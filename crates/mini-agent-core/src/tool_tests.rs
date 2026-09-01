use super::ToolRouter;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use std::sync::Arc;

struct EchoTool;

impl ToolHandler for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for EchoTool {
    fn execute(&self, _arguments: &Value) -> Result<String, mini_agent_protocol::ToolError> {
        Ok("legacy".to_string())
    }
}

struct DelegatingExecution;

impl ToolExecutionDelegate for DelegatingExecution {
    fn execute(&self, _tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        ToolExecutionOutcome::completed(format!("{}:{}", request.call_id, request.name))
    }
}

#[test]
fn router_resolves_before_delegating_execution() {
    let router = ToolRouter::with_executor(vec![Box::new(EchoTool)], Arc::new(DelegatingExecution));

    assert_eq!(
        router.execute_outcome(&ToolExecutionRequest::new(
            "call-1",
            "echo",
            serde_json::json!({}),
        )),
        ToolExecutionOutcome::completed("call-1:echo")
    );
}

#[test]
fn hidden_tools_are_not_visible_or_resolvable_and_can_be_restored() {
    let mut router = ToolRouter::new(vec![Box::new(EchoTool)]);
    router.set_hidden_tools(vec!["echo".to_string()]);

    assert!(router.specs().is_empty());
    assert_eq!(
        router.execute("echo", &serde_json::json!({})),
        Err(mini_agent_protocol::ToolError(
            "unknown tool: echo".to_string()
        ))
    );

    router.set_hidden_tools(Vec::new());
    assert_eq!(router.specs().len(), 1);
}
