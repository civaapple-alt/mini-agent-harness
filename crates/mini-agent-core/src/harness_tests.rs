use super::*;
use crate::ModelResponse;
use crate::ModelUsage;
use crate::Tool;
use crate::ToolCall;
use crate::ToolError;
use crate::ToolSpec;
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
        }
    );
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

    let outcome = harness
        .run("perform the long operation", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "Long operation completed.");
    assert!(matches!(
        outcome.messages.as_slice(),
        [
            Message::User { text },
            Message::Context { text: context },
            Message::Assistant {
                text: answer,
                tool_calls,
                ..
            }
        ] if text.starts_with(COMPACTION_PREFIX)
            && context == "<world_state>rust,cargo</world_state>"
            && answer == "Long operation completed."
            && tool_calls.is_empty()
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
    assert_eq!(compaction.tools, first.tools);
    assert_eq!(continuation.system_prompt, first.system_prompt);
    assert_eq!(continuation.tools, first.tools);
    assert_eq!(
        &compaction.messages[..first.messages.len()],
        first.messages.as_slice()
    );
    assert!(matches!(
        compaction.messages.last(),
        Some(Message::User { text }) if text == COMPACTION_PROMPT
    ));
}

#[tokio::test]
async fn failed_compaction_preserves_existing_history() {
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

    let error = harness
        .run("perform the long operation", &mut events)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Compaction(reason) if reason == "model returned an empty summary"
    ));
    assert_eq!(harness.messages().len(), 3);
    assert!(matches!(
        harness.messages().first(),
        Some(Message::User { text }) if text == "perform the long operation"
    ));
    assert_eq!(
        events.0.last(),
        Some(&Event::RunFailed {
            reason: crate::RunFailure::Compaction,
        })
    );
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
            reason: crate::RunFailure::LimitExceeded(_)
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

#[derive(Default)]
struct RecordingObserver(Vec<Event>);

impl Observer for RecordingObserver {
    fn observe(&mut self, event: &Event) {
        self.0.push(event.clone());
    }
}
