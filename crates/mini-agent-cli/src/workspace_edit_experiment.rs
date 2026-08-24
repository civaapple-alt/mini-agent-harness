use super::*;
use mini_agent_core::Event;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::Observer;
use mini_agent_core::StopReason;
use mini_agent_core::ToolCall;
use mini_agent_core::ToolRegistry;

const PROMPT: &str = "Change mode from slow to fast; preserve everything else.";
const INITIAL: &str = "# preserve this comment\nmode = slow\nretries = 3\n# owner = runtime-team\n";
const EXPECTED: &str =
    "# preserve this comment\nmode = fast\nretries = 3\n# owner = runtime-team\n";
const LOSSY_REWRITE: &str = "mode = fast\nretries = 3\n";

#[derive(Debug, PartialEq)]
struct ExperimentResult {
    treatment: &'static str,
    completed: bool,
    model_steps: usize,
    tool_calls: usize,
    tool_errors: usize,
    target_changed: bool,
    collateral_preserved: bool,
}

impl ExperimentResult {
    fn as_json(&self) -> Value {
        json!({
            "treatment": self.treatment,
            "completed": self.completed,
            "model_steps": self.model_steps,
            "tool_calls": self.tool_calls,
            "tool_errors": self.tool_errors,
            "target_changed": self.target_changed,
            "collateral_preserved": self.collateral_preserved,
        })
    }
}

#[derive(Clone, Copy)]
enum EditSurface {
    ExactReplacement,
    WholeFileRewrite,
}

struct EditingModel;

impl Model for EditingModel {
    type Error = std::convert::Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let tool_results = request
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .count();
        if tool_results == 0 {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "Reading the file first.".to_string(),
                tool_calls: vec![ToolCall {
                    id: "read-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({"path": "settings.conf"}),
                }],
                usage: None,
            });
        }
        if tool_results == 1 {
            let exact_edit_available = request.tools.iter().any(|tool| tool.name == "edit_file");
            let (name, arguments) = if exact_edit_available {
                (
                    "edit_file",
                    json!({
                        "path": "settings.conf",
                        "old_text": "mode = slow",
                        "new_text": "mode = fast"
                    }),
                )
            } else {
                (
                    "write_file",
                    json!({"path": "settings.conf", "content": LOSSY_REWRITE}),
                )
            };
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "Applying the requested change.".to_string(),
                tool_calls: vec![ToolCall {
                    id: "edit-1".to_string(),
                    name: name.to_string(),
                    arguments,
                }],
                usage: None,
            });
        }

        Ok(ModelResponse {
            reasoning: String::new(),
            text: "Done.".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

struct RewriteFile(Arc<Workspace>);

impl Tool for RewriteFile {
    fn spec(&self) -> ToolSpec {
        file_tool_spec("write_file", "Replace an entire UTF-8 workspace file", true)
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.0.read_path(arguments)?;
        let content = string_arg(arguments, "content")?;
        self.0.approve(&format!("rewrite {}", path.display()))?;
        fs::write(&path, content).map_err(io_error)?;
        Ok(format!("rewrote {}", path.display()))
    }
}

#[derive(Default)]
struct TraceCounts {
    tool_calls: usize,
    tool_errors: usize,
}

impl Observer for TraceCounts {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::ToolStarted { .. } => self.tool_calls += 1,
            Event::ToolFinished { is_error: true, .. } => self.tool_errors += 1,
            _ => {}
        }
    }
}

async fn run_treatment(surface: EditSurface) -> ExperimentResult {
    let root = tests::test_root();
    fs::write(root.join("settings.conf"), INITIAL).unwrap();
    let workspace = Arc::new(
        Workspace::new(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
        )
        .unwrap(),
    );
    let mutation: Box<dyn Tool> = match surface {
        EditSurface::ExactReplacement => Box::new(EditFile(Arc::clone(&workspace))),
        EditSurface::WholeFileRewrite => Box::new(RewriteFile(Arc::clone(&workspace))),
    };
    let tools = ToolRegistry::new(vec![Box::new(ReadFile(workspace)), mutation]);
    let config = HarnessConfig {
        max_steps: 3,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(EditingModel, tools, config);
    let mut trace = TraceCounts::default();

    let outcome = harness.run(PROMPT, &mut trace).await.unwrap();
    let content = fs::read_to_string(root.join("settings.conf")).unwrap();
    fs::remove_dir_all(root).unwrap();

    ExperimentResult {
        treatment: match surface {
            EditSurface::ExactReplacement => "exact_unique_replacement",
            EditSurface::WholeFileRewrite => "whole_file_rewrite",
        },
        completed: outcome.stop_reason == StopReason::Completed,
        model_steps: outcome.steps,
        tool_calls: trace.tool_calls,
        tool_errors: trace.tool_errors,
        target_changed: content.contains("mode = fast"),
        collateral_preserved: content == EXPECTED,
    }
}

#[tokio::test]
async fn compares_exact_edit_with_whole_file_rewrite() {
    let exact = run_treatment(EditSurface::ExactReplacement).await;
    let rewrite = run_treatment(EditSurface::WholeFileRewrite).await;

    println!(
        "{}",
        serde_json::to_string_pretty(&[exact.as_json(), rewrite.as_json()]).unwrap()
    );
    assert_eq!(
        exact,
        ExperimentResult {
            treatment: "exact_unique_replacement",
            completed: true,
            model_steps: 3,
            tool_calls: 2,
            tool_errors: 0,
            target_changed: true,
            collateral_preserved: true,
        }
    );
    assert_eq!(
        rewrite,
        ExperimentResult {
            treatment: "whole_file_rewrite",
            completed: true,
            model_steps: 3,
            tool_calls: 2,
            tool_errors: 0,
            target_changed: true,
            collateral_preserved: false,
        }
    );
}
