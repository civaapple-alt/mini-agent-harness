use super::*;
use crate::context_controller::COMPACTION_PREFIX;
use crate::context_controller::COMPACTION_PROMPT;
use crate::context_controller::assemble_compacted;
use crate::context_controller::split_prefix_tail;
use crate::context_controller::trim_prefix_to_fit;
use crate::tool_batch_executor::truncate_utf8;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelUsage;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolSpec;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;

struct ScriptedModel {
    responses: VecDeque<ModelResponse>,
}

impl Model for ScriptedModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        Ok(self
            .responses
            .pop_front()
            .expect("missing scripted response"))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RecordedRequest {
    system_prompt: String,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
}

struct RecordingModel {
    responses: VecDeque<ModelResponse>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl Model for RecordingModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.requests.lock().unwrap().push(RecordedRequest {
            system_prompt: request.system_prompt.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.to_vec(),
        });
        Ok(self
            .responses
            .pop_front()
            .expect("missing recorded response"))
    }
}

struct Uppercase;

impl Tool for Uppercase {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "uppercase".to_string(),
            description: "Convert text to uppercase".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("text must be a string".to_string()))?;
        Ok(text.to_uppercase())
    }
}

struct ApprovalTool;

impl Tool for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "needs_approval".to_string(),
            description: "A tool whose host policy requires approval".to_string(),
            parameters: json!({"type": "object"}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("approval required".to_string()))
    }

    fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
        ToolExecutionOutcome {
            status: ToolExecutionStatus::NeedsApproval,
            content: "approval required".to_string(),
        }
    }
}

#[tokio::test]
async fn runs_model_tool_model_path() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": "quiet"}),
                }],
                usage: Some(ModelUsage {
                    input_tokens: 10,
                    cached_input_tokens: 2,
                    output_tokens: 3,
                }),
            },
            ModelResponse {
                reasoning: String::new(),
                text: "The result is QUIET.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let tools = ToolRegistry::new(vec![Box::new(Uppercase)]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    let outcome = harness
        .run("make it loud", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "The result is QUIET.");
    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "uppercase".to_string(),
            content: "QUIET".to_string(),
            is_error: false,
            outcome: Some(ToolExecutionStatus::Completed),
        }
    );
    assert_eq!(
        events.last(),
        Some(&Event::RunFinished {
            stop_reason: StopReason::Completed,
            steps: 2,
        })
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ModelResponded {
            usage: Some(ModelUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                output_tokens: 3,
            }),
            ..
        }
    )));
}

#[tokio::test]
async fn steering_stops_after_a_complete_tool_batch() {
    let control = RunControl::new();
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "request_steer".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        }]),
    };
    let tools = ToolRegistry::new(vec![Box::new(RequestSteer(control.clone()))]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());

    let outcome = harness
        .run_with_control("correct me", &mut (), &control)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Steered);
    assert_eq!(outcome.steps, 1);
    assert!(matches!(
        outcome.messages.last(),
        Some(Message::Tool { .. })
    ));
}

struct SubmitSteerDuringSampling {
    control: RunControl,
    calls: usize,
}

impl Model for SubmitSteerDuringSampling {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let call = self.calls;
        self.calls = self.calls.saturating_add(1);
        if call == 0 {
            self.control
                .submit(TurnInput::new(
                    TurnInputMode::Steer,
                    "focus on the actual bug",
                ))
                .unwrap();
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "the first answer drifted".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        assert!(request.messages.iter().any(|message| matches!(
            message,
            Message::User { text } if text == "focus on the actual bug"
        )));
        Ok(ModelResponse {
            reasoning: String::new(),
            text: "the corrected answer".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

#[tokio::test]
async fn same_turn_steering_consumes_input_after_sampling() {
    let control = RunControl::new();
    let model = SubmitSteerDuringSampling {
        control: control.clone(),
        calls: 0,
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());

    let outcome = harness
        .run_with_control_mode(
            "initial request",
            &mut (),
            &control,
            SteeringMode::ContinueSameTurn,
        )
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "the corrected answer");
    assert_eq!(
        outcome
            .messages
            .iter()
            .filter(|message| matches!(message, Message::User { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn returns_unknown_tool_failure_to_model() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "missing".to_string(),
                    arguments: json!({}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "I could not run that tool.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());

    let outcome = harness.run("try it", &mut ()).await.unwrap();

    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "missing".to_string(),
            content: "unknown tool: missing".to_string(),
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
        }
    );
}

#[tokio::test]
async fn preserves_structured_tool_policy_outcome_in_events() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-approval".to_string(),
                    name: "needs_approval".to_string(),
                    arguments: json!({}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "waiting for approval".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let mut harness = Harness::new(
        model,
        ToolRegistry::new(vec![Box::new(ApprovalTool)]),
        HarnessConfig::default(),
    );
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    harness
        .run("use the protected tool", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            is_error: true,
            outcome: Some(ToolExecutionStatus::NeedsApproval),
            ..
        }
    )));
    assert!(harness.messages().iter().any(|message| matches!(
        message,
        Message::Tool {
            outcome: Some(ToolExecutionStatus::NeedsApproval),
            is_error: true,
            ..
        }
    )));
}

#[tokio::test]
async fn records_tool_output_truncation_explicitly() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": "abcdefghij"}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "done".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let config = HarnessConfig {
        max_tool_output_bytes: 5,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(Uppercase)]), config);
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    harness
        .run("produce long output", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            content,
            truncated: true,
            ..
        } if content == "ABCDE"
    )));
}

#[tokio::test]
async fn stops_at_step_limit() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "still working".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "missing".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_steps: 1,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config);

    let outcome = harness.run("continue forever", &mut ()).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::StepLimit);
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.final_text, "still working");
}

#[tokio::test]
async fn preserves_history_across_runs_and_can_clear_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "second answer".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());

    harness.run("first question", &mut ()).await.unwrap();
    let outcome = harness.run("second question", &mut ()).await.unwrap();

    assert_eq!(outcome.messages.len(), 4);
    assert_eq!(harness.messages(), outcome.messages);
    assert_eq!(
        outcome.messages,
        vec![
            Message::User {
                text: "first question".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
            },
            Message::User {
                text: "second question".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "second answer".to_string(),
                tool_calls: Vec::new(),
            },
        ]
    );

    harness.clear_history();
    assert!(harness.messages().is_empty());
}

#[test]
fn context_items_have_an_independent_hard_limit() {
    let config = HarnessConfig {
        max_context_item_bytes: 4,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRegistry::default(),
        config,
    );

    let error = harness.append_context("12345").unwrap_err();

    assert_eq!(
        error,
        LimitExceeded {
            kind: LimitKind::ContextItemBytes,
            limit: 4,
            actual: 5,
        }
    );
    assert!(harness.messages().is_empty());
}

#[test]
fn restores_only_history_that_fits_the_current_harness() {
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRegistry::default(),
        HarnessConfig::default(),
    );
    let messages = vec![Message::Context {
        text: "persisted world".to_string(),
    }];

    harness.restore_history(messages.clone()).unwrap();

    assert_eq!(harness.messages(), messages);
    let error = harness
        .restore_history(vec![Message::Context {
            text: "x".repeat(HarnessConfig::default().max_context_item_bytes + 1),
        }])
        .unwrap_err();
    assert_eq!(error.kind, LimitKind::ContextItemBytes);
    assert_eq!(harness.messages(), messages);

    let user_err = harness
        .restore_history(vec![Message::User {
            text: "x".repeat(HarnessConfig::default().max_user_input_bytes + 1),
        }])
        .unwrap_err();
    assert_eq!(user_err.kind, LimitKind::UserInputBytes);

    let tool_err = harness
        .restore_history(vec![Message::Tool {
            call_id: "call-1".to_string(),
            name: "uppercase".to_string(),
            content: "x".repeat(HarnessConfig::default().max_tool_output_bytes + 1),
            is_error: false,
            outcome: None,
        }])
        .unwrap_err();
    assert_eq!(tool_err.kind, LimitKind::ToolOutputBytes);

    let assistant_err = harness
        .restore_history(vec![Message::Assistant {
            reasoning: String::new(),
            text: "x".repeat(HarnessConfig::default().max_model_response_bytes + 1),
            tool_calls: vec![],
        }])
        .unwrap_err();
    assert_eq!(assistant_err.kind, LimitKind::ModelResponseBytes);
}

struct RequestSteer(RunControl);

impl Tool for RequestSteer {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "request_steer".to_string(),
            description: "Request a cooperative turn stop".to_string(),
            parameters: json!({"type": "object", "additionalProperties": false}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        self.0.request_steer();
        Ok("steer requested".to_string())
    }
}

#[test]
fn verifier_can_restore_tool_history_before_disabling_new_tool_calls() {
    let history = vec![
        Message::User {
            text: "inspect the release".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: json!({"key": "release"}),
            }],
        },
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "lookup".to_string(),
            content: "release is ready".to_string(),
            is_error: false,
            outcome: None,
        },
    ];
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRegistry::default(),
        HarnessConfig::default(),
    );

    harness.restore_history(history.clone()).unwrap();
    harness.replace_config(HarnessConfig {
        max_tool_calls_per_step: 0,
        ..HarnessConfig::default()
    });

    assert_eq!(harness.messages(), history);
}

#[tokio::test]
async fn compacts_context_and_continues_the_tool_loop() {
    let long_tool_value = "x".repeat(300);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": long_tool_value}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "The user asked for a long operation. The uppercase tool completed successfully. Continue by reporting completion.".to_string(),
                tool_calls: Vec::new(),
                usage: Some(ModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: 0,
                    output_tokens: 20,
                }),
            },
            ModelResponse {
                reasoning: String::new(),
                text: "Long operation completed.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
        requests: Arc::clone(&requests),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 1024,
        max_tool_output_bytes: 512,
        max_context_bytes: 2000,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(Uppercase)]), config);
    harness
        .append_context("<world_state>rust,cargo</world_state>")
        .unwrap();
    let mut events = RecordingObserver::default();
    let prompt = format!("perform the long operation {}", "n".repeat(400));

    let outcome = harness.run(&prompt, &mut events).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "Long operation completed.");
    assert!(matches!(
        outcome.messages.as_slice(),
        [
            Message::User { text },
            Message::Context { text: context },
            Message::Assistant { tool_calls, .. },
            Message::Tool { name, content, .. },
            Message::Assistant {
                text: answer,
                tool_calls: final_calls,
                ..
            }
        ] if text.starts_with(COMPACTION_PREFIX)
            && context == "<world_state>rust,cargo</world_state>"
            && !tool_calls.is_empty()
            && name == "uppercase"
            && content.contains('X')
            && answer == "Long operation completed."
            && final_calls.is_empty()
    ));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            usage: Some(ModelUsage { input_tokens: 100, .. }),
        } if after_bytes < before_bytes
    )));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let first = &requests[0];
    let compaction = &requests[1];
    let continuation = &requests[2];
    assert_eq!(compaction.system_prompt, first.system_prompt);
    assert!(compaction.tools.is_empty());
    assert_eq!(continuation.system_prompt, first.system_prompt);
    assert_eq!(continuation.tools, first.tools);
    assert!(matches!(
        compaction.messages.as_slice(),
        [
            Message::User { text: compacted_prompt },
            Message::User { text: instruction },
        ] if compacted_prompt == prompt.as_str() && instruction == COMPACTION_PROMPT
    ));
    assert!(matches!(
        continuation.messages.as_slice(),
        [
            Message::User { text },
            Message::Context { .. },
            Message::Assistant { tool_calls, .. },
            Message::Tool { name, content, .. },
        ] if text.starts_with(COMPACTION_PREFIX)
            && !tool_calls.is_empty()
            && name == "uppercase"
            && content.contains('X')
    ));
}

#[tokio::test]
async fn empty_summary_falls_back_to_mechanical_trim() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": "x".repeat(300)}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "   ".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "Long operation completed.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 1024,
        max_tool_output_bytes: 512,
        max_context_bytes: 2000,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(Uppercase)]), config);
    let mut events = RecordingObserver::default();

    let outcome = harness
        .run("perform the long operation", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.final_text, "Long operation completed.");
    assert!(outcome.messages.iter().any(|message| matches!(
        message,
        Message::Tool { name, content, .. } if name == "uppercase" && content.contains('X')
    )));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            ..
        } if after_bytes < before_bytes
    )));
    assert!(!events.0.iter().any(|event| matches!(
        event,
        Event::RunFailed {
            reason: mini_agent_protocol::RunFailure::Compaction,
        }
    )));
}

#[tokio::test]
async fn trims_over_budget_compaction_prefix_and_continues() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: "Older turns covered padding and the latest user asked to continue."
                    .to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "Continued.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
        requests: Arc::clone(&requests),
    };
    let padding = "p".repeat(300);
    let history = vec![
        Message::User {
            text: format!("old:{padding}"),
        },
        Message::User {
            text: format!("mid:{padding}"),
        },
        Message::User {
            text: format!("recent:{padding}"),
        },
    ];
    let system_prompt = HarnessConfig::default().system_prompt;
    let history_bytes = context_bytes_for(&system_prompt, &history, &[]);
    let config = HarnessConfig {
        max_context_bytes: history_bytes,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config.clone());
    harness.restore_history(history).unwrap();
    let mut events = RecordingObserver::default();

    let outcome = harness.run("continue", &mut events).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.final_text, "Continued.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let compaction = &requests[0];
    assert!(matches!(
        compaction.messages.last(),
        Some(Message::User { text }) if text == COMPACTION_PROMPT
    ));
    assert!(
        !compaction
            .messages
            .iter()
            .any(|message| matches!(message, Message::User { text } if text.starts_with("old:")))
    );
    assert!(
        context_bytes_for(
            &compaction.system_prompt,
            &compaction.messages,
            &compaction.tools
        ) <= config.max_context_bytes
    );
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            ..
        } if after_bytes < before_bytes
    )));
}

#[test]
fn split_prefix_tail_keeps_last_two_assistant_groups() {
    let assistant = |text: &str| Message::Assistant {
        reasoning: String::new(),
        text: text.to_string(),
        tool_calls: Vec::new(),
    };
    let messages = vec![
        Message::User {
            text: "one".to_string(),
        },
        assistant("a1"),
        Message::User {
            text: "two".to_string(),
        },
        assistant("a2"),
        Message::User {
            text: "three".to_string(),
        },
        assistant("a3"),
    ];

    let (prefix, tail) = split_prefix_tail(&messages);

    assert_eq!(
        prefix,
        vec![
            Message::User {
                text: "one".to_string(),
            },
            assistant("a1"),
            Message::User {
                text: "two".to_string(),
            },
        ]
    );
    assert_eq!(
        tail,
        vec![
            assistant("a2"),
            Message::User {
                text: "three".to_string(),
            },
            assistant("a3"),
        ]
    );
}

#[test]
fn trim_prefix_to_fit_drops_oldest_until_request_fits() {
    let system = "sys";
    let tools: &[ToolSpec] = &[];
    let prompt = "SUMMARIZE";
    let mut prefix = vec![
        Message::User {
            text: format!("old-{}", "a".repeat(400)),
        },
        Message::User {
            text: format!("keep-{}", "b".repeat(40)),
        },
    ];
    let fitting = vec![
        prefix[1].clone(),
        Message::User {
            text: prompt.to_string(),
        },
    ];
    let max_bytes = context_bytes_for(system, &fitting, tools);

    trim_prefix_to_fit(&mut prefix, prompt, system, tools, max_bytes);

    assert_eq!(prefix.len(), 1);
    assert!(matches!(
        &prefix[0],
        Message::User { text } if text.starts_with("keep-")
    ));
}

#[test]
fn truncates_utf8_within_hard_byte_limit() {
    let output = truncate_utf8("一二三四五六七八九十".to_string(), 20);

    assert!(output.len() <= 20);
    assert!(output.is_char_boundary(output.len()));
    assert!(output.starts_with('一'));
    assert!(output.ends_with('十'));
}

#[tokio::test]
async fn rejects_oversized_user_input_without_retaining_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "unused".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_user_input_bytes: 4,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config);
    let mut events = RecordingObserver::default();

    let error = harness.run("12345", &mut events).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::UserInputBytes,
            limit: 4,
            actual: 5,
        })
    ));
    assert!(harness.messages().is_empty());
    assert!(matches!(
        events.0.as_slice(),
        [Event::RunFailed {
            reason: mini_agent_protocol::RunFailure::LimitExceeded(_)
        }]
    ));
    assert_eq!(
        serde_json::to_value(&events.0[0]).unwrap(),
        json!({
            "type": "run_failed",
            "reason": {
                "type": "limit_exceeded",
                "detail": {
                    "kind": "user_input_bytes",
                    "limit": 4,
                    "actual": 5
                }
            }
        })
    );
}

#[test]
fn compaction_summary_includes_prefix_within_user_limit() {
    let compacted = assemble_compacted(
        Some("这是一个足够长的压缩摘要，用于验证 UTF-8 截断"),
        None,
        Vec::new(),
        32,
    );

    let Some(Message::User { text }) = compacted.first() else {
        panic!("compaction should produce a summary user message");
    };
    assert!(text.len() <= 32);
    assert!(text.is_char_boundary(text.len()));
}

#[tokio::test]
async fn rejects_context_before_calling_the_model() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "unused".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_context_bytes: 1,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config);

    let error = harness.run("a", &mut ()).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ContextBytes,
            limit: 1,
            ..
        })
    ));
    assert!(harness.messages().is_empty());
}

#[tokio::test]
async fn rejects_excess_tool_calls_before_executing_any() {
    let call = |id: &str| ToolCall {
        id: id.to_string(),
        name: "uppercase".to_string(),
        arguments: json!({"text": "quiet"}),
    };
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![call("call-1"), call("call-2")],
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_tool_calls_per_step: 1,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::new(vec![Box::new(Uppercase)]), config);
    let mut events = RecordingObserver::default();

    let error = harness.run("do both", &mut events).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ToolCallsPerStep,
            limit: 1,
            actual: 2,
        })
    ));
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ToolStarted { .. }))
    );
}

#[tokio::test]
async fn rejects_oversized_model_response_before_retaining_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: "why".to_string(),
            text: "x".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 5,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRegistry::default(), config);

    let error = harness.run("answer", &mut ()).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ModelResponseBytes,
            limit: 5,
            ..
        })
    ));
    assert_eq!(
        harness.messages(),
        &[Message::User {
            text: "answer".to_string(),
        }]
    );
}

#[test]
fn default_ceilings_remain_bounded_under_deepseek_v4_windows() {
    let config = HarnessConfig::default();
    assert_eq!(config.max_context_bytes, 1024 * 1024);
    assert_eq!(config.max_model_response_bytes, 64 * 1024);
}

#[tokio::test]
async fn repetitive_tool_calls_trigger_loop_warning() {
    struct EchoTool;
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".to_string(),
                description: "echo input".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"],
                    "additionalProperties": false
                }),
            }
        }
        fn execute(&self, _args: &Value) -> Result<String, ToolError> {
            Ok("same output".to_string())
        }
    }

    let tools = ToolRegistry::new(vec![Box::new(EchoTool)]);

    let model = ScriptedModel {
        responses: VecDeque::from(vec![
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"msg": "hello"}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call2".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"msg": "hello"}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call3".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({"msg": "hello"}),
                }],
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "done after warning".to_string(),
                tool_calls: vec![],
                usage: None,
            },
        ]),
    };

    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    let outcome = harness.run("start", &mut ()).await.unwrap();

    assert_eq!(outcome.final_text, "done after warning");
    // Verify that loop warning was injected into messages
    let has_loop_warning = harness.messages().iter().any(|msg| match msg {
        Message::Context { text } => text.contains("Loop warning"),
        _ => false,
    });
    assert!(has_loop_warning, "Expected loop warning in harness context");
}

#[test]
fn turn_atomic_trimming_drops_assistant_and_tool_groups_together() {
    let system = "sys";
    let tools: &[ToolSpec] = &[];
    let prompt = "SUMMARIZE";
    let mut prefix = vec![
        Message::User {
            text: "first question".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "calling tool".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "test_tool".to_string(),
                arguments: json!({"arg": "val"}),
            }],
        },
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "test_tool".to_string(),
            content: "x".repeat(300),
            is_error: false,
            outcome: None,
        },
        Message::User {
            text: "second question".to_string(),
        },
    ];

    let fitting = vec![
        prefix[3].clone(),
        Message::User {
            text: prompt.to_string(),
        },
    ];
    let max_bytes = context_bytes_for(system, &fitting, tools);

    trim_prefix_to_fit(&mut prefix, prompt, system, tools, max_bytes);

    let mut pending_tool_calls: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for msg in &prefix {
        match msg {
            Message::Assistant { tool_calls, .. } => {
                for call in tool_calls {
                    pending_tool_calls.insert(call.id.clone());
                }
            }
            Message::Tool { call_id, .. } => {
                assert!(
                    pending_tool_calls.contains(call_id.as_str()),
                    "found orphan tool output without matching assistant call: {call_id}"
                );
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct RecordingObserver(Vec<Event>);

impl Observer for RecordingObserver {
    fn observe(&mut self, event: &Event) {
        self.0.push(event.clone());
    }
}
