use crate::Harness;
use crate::HarnessConfig;
use crate::HarnessError;
use crate::ToolRegistry;
use mini_agent_protocol::Event;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEvent;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::Observer;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

#[derive(Debug)]
struct FaultInjectionError(&'static str);

impl fmt::Display for FaultInjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for FaultInjectionError {}

enum FaultStep {
    Response(ModelResponse),
    PartialStream,
}

/// Test-only provider double for dirty model boundaries and recovery paths.
struct FaultInjectionModel {
    steps: VecDeque<FaultStep>,
}

impl FaultInjectionModel {
    fn new(steps: impl IntoIterator<Item = FaultStep>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
        }
    }
}

impl Model for FaultInjectionModel {
    type Error = FaultInjectionError;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        match self
            .steps
            .pop_front()
            .expect("missing fault-injection step")
        {
            FaultStep::Response(response) => Ok(response),
            FaultStep::PartialStream => {
                events.emit(ModelEvent::TextDelta("partial".to_string()));
                Err(FaultInjectionError("partial model stream"))
            }
        }
    }
}

#[derive(Default)]
struct EventRecorder(Vec<Event>);

impl Observer for EventRecorder {
    fn observe(&mut self, event: &Event) {
        self.0.push(event.clone());
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        reasoning: String::new(),
        text: text.to_string(),
        tool_calls: Vec::new(),
        usage: None,
    }
}

fn tool_response(name: &str, arguments: Value) -> ModelResponse {
    ModelResponse {
        reasoning: String::new(),
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "fault-call".to_string(),
            name: name.to_string(),
            arguments,
        }],
        usage: None,
    }
}

struct RequiredStringTool;

impl ToolHandler for RequiredStringTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "required_string".to_string(),
            description: "Requires a string value".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for RequiredStringTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        arguments
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_uppercase)
            .ok_or_else(|| ToolError("missing required string argument: value".to_string()))
    }
}

struct RetryableTool;

impl ToolHandler for RetryableTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "retryable".to_string(),
            description: "Returns a temporary failure".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for RetryableTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("temporary failure".to_string()))
    }

    fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
        ToolExecutionOutcome {
            status: ToolExecutionStatus::Retryable,
            content: "temporary failure".to_string(),
        }
    }
}

#[tokio::test]
async fn missing_required_tool_argument_is_projected_for_model_recovery() {
    let model = FaultInjectionModel::new([
        FaultStep::Response(tool_response("required_string", json!({}))),
        FaultStep::Response(text_response("recovered")),
    ]);
    let mut harness = Harness::new(
        model,
        ToolRegistry::new(vec![Box::new(RequiredStringTool)]),
        HarnessConfig::default(),
    );
    let mut events = EventRecorder::default();

    let outcome = harness
        .run("use the required tool", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.final_text, "recovered");
    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "fault-call".to_string(),
            name: "required_string".to_string(),
            content: "missing required string argument: value".to_string(),
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
        }
    );
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
            ..
        }
    )));
}

#[tokio::test]
async fn partial_model_stream_is_failed_without_fabricating_completion() {
    let model = FaultInjectionModel::new([FaultStep::PartialStream]);
    let mut harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());
    let mut events = EventRecorder::default();

    let error = harness
        .run("stream a response", &mut events)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Model(FaultInjectionError("partial model stream"))
    ));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::AssistantTextDelta { delta } if delta == "partial"
    )));
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ModelResponded { .. }))
    );
    assert_eq!(
        events.0.last(),
        Some(&Event::RunFailed {
            reason: mini_agent_protocol::RunFailure::Model,
        })
    );
}

#[tokio::test]
async fn retryable_tool_result_is_preserved_until_model_recovers() {
    let model = FaultInjectionModel::new([
        FaultStep::Response(tool_response("retryable", json!({}))),
        FaultStep::Response(text_response("retry recovered")),
    ]);
    let mut harness = Harness::new(
        model,
        ToolRegistry::new(vec![Box::new(RetryableTool)]),
        HarnessConfig::default(),
    );
    let mut events = EventRecorder::default();

    let outcome = harness
        .run("retry the temporary failure", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.final_text, "retry recovered");
    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "fault-call".to_string(),
            name: "retryable".to_string(),
            content: "temporary failure".to_string(),
            is_error: true,
            outcome: Some(ToolExecutionStatus::Retryable),
        }
    );
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            outcome: Some(ToolExecutionStatus::Retryable),
            is_error: true,
            ..
        }
    )));
}
