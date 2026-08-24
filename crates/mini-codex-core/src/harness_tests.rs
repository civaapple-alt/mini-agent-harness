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
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "missing".to_string(),
                    arguments: json!({}),
                }],
                usage: None,
            },
            ModelResponse {
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
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": "abcdefghij"}),
                }],
                usage: None,
            },
            ModelResponse {
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
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
            ModelResponse {
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
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
            },
            Message::User {
                text: "second question".to_string(),
            },
            Message::Assistant {
                text: "second answer".to_string(),
                tool_calls: Vec::new(),
            },
        ]
    );

    harness.clear_history();
    assert!(harness.messages().is_empty());
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
            text: "too long".to_string(),
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
