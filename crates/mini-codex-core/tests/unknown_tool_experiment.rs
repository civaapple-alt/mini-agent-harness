use mini_codex_core::Event;
use mini_codex_core::Harness;
use mini_codex_core::HarnessConfig;
use mini_codex_core::Message;
use mini_codex_core::Model;
use mini_codex_core::ModelEventSink;
use mini_codex_core::ModelRequest;
use mini_codex_core::ModelResponse;
use mini_codex_core::Observer;
use mini_codex_core::ToolCall;
use mini_codex_core::ToolRegistry;
use serde::Serialize;
use serde_json::json;
use std::convert::Infallible;

const PROMPT: &str = "Find the answer, recovering from a missing tool if needed.";
const RECOVERY_EVIDENCE: &str = "RECOVERED_FROM_TOOL_ERROR";

#[derive(Debug, PartialEq, Serialize)]
struct ExperimentResult {
    treatment: &'static str,
    completed: bool,
    model_steps: usize,
    tool_errors: usize,
    verifier_passed: bool,
    final_text: String,
}

struct RecoveringModel;

impl Model for RecoveringModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let saw_tool_error = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::Tool { is_error: true, .. }));
        if saw_tool_error {
            return Ok(ModelResponse {
                text: RECOVERY_EVIDENCE.to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }

        Ok(ModelResponse {
            text: "Trying the workspace_search tool.".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "workspace_search".to_string(),
                arguments: json!({ "query": "answer" }),
            }],
            usage: None,
        })
    }
}

#[derive(Default)]
struct ErrorCounter {
    tool_errors: usize,
}

impl Observer for ErrorCounter {
    fn observe(&mut self, event: &Event) {
        if matches!(event, Event::ToolFinished { is_error: true, .. }) {
            self.tool_errors += 1;
        }
    }
}

struct IgnoreModelEvents;

impl ModelEventSink for IgnoreModelEvents {
    fn emit(&mut self, _event: mini_codex_core::ModelEvent) {}
}

async fn project_error_to_model() -> ExperimentResult {
    let config = HarnessConfig {
        max_steps: 2,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(RecoveringModel, ToolRegistry::default(), config);
    let mut observer = ErrorCounter::default();

    let outcome = harness.run(PROMPT, &mut observer).await.unwrap();
    let verifier_passed = outcome.final_text == RECOVERY_EVIDENCE;
    ExperimentResult {
        treatment: "project_error_to_model",
        completed: true,
        model_steps: outcome.steps,
        tool_errors: observer.tool_errors,
        verifier_passed,
        final_text: outcome.final_text,
    }
}

async fn stop_on_unknown_tool() -> ExperimentResult {
    let messages = vec![Message::User {
        text: PROMPT.to_string(),
    }];
    let tools = ToolRegistry::default();
    let tool_specs = tools.specs();
    let mut model = RecoveringModel;
    let mut events = IgnoreModelEvents;
    let response = model
        .respond(
            ModelRequest {
                system_prompt: &HarnessConfig::default().system_prompt,
                messages: &messages,
                tools: &tool_specs,
                max_response_bytes: HarnessConfig::default().max_model_response_bytes,
            },
            &mut events,
        )
        .await
        .unwrap();
    let tool_errors = response
        .tool_calls
        .iter()
        .filter(|call| tools.execute(&call.name, &call.arguments).is_err())
        .count();
    let final_text = response.text;
    let verifier_passed = final_text == RECOVERY_EVIDENCE;

    ExperimentResult {
        treatment: "stop_on_unknown_tool",
        completed: false,
        model_steps: 1,
        tool_errors,
        verifier_passed,
        final_text,
    }
}

#[tokio::test]
async fn compares_unknown_tool_treatments() {
    let stop = stop_on_unknown_tool().await;
    let project = project_error_to_model().await;

    println!(
        "{}",
        serde_json::to_string_pretty(&[&stop, &project]).unwrap()
    );
    assert_eq!(
        stop,
        ExperimentResult {
            treatment: "stop_on_unknown_tool",
            completed: false,
            model_steps: 1,
            tool_errors: 1,
            verifier_passed: false,
            final_text: "Trying the workspace_search tool.".to_string(),
        }
    );
    assert_eq!(
        project,
        ExperimentResult {
            treatment: "project_error_to_model",
            completed: true,
            model_steps: 2,
            tool_errors: 1,
            verifier_passed: true,
            final_text: RECOVERY_EVIDENCE.to_string(),
        }
    );
}
