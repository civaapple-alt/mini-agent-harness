use super::classify_outcome;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;

struct LegacyTool(&'static str);

impl Tool for LegacyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError(self.0.to_string()))
    }
}

#[test]
fn classifies_host_policy_messages_without_changing_content() {
    let cases = [
        (
            "user denied: shell command `pwd`",
            ToolExecutionStatus::NeedsApproval,
        ),
        (
            "workspace mutations locked in Plan Mode; living plan is plan.md",
            ToolExecutionStatus::Deferred,
        ),
        ("MCP tool call timed out", ToolExecutionStatus::Retryable),
    ];
    for (content, status) in cases {
        let outcome = classify_outcome(ToolExecutionOutcome::failed(content));
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.content, content);
    }
}

#[test]
fn leaves_regular_failures_as_failed() {
    let outcome = classify_outcome(ToolExecutionOutcome::failed("invalid arguments"));
    assert_eq!(outcome.status, ToolExecutionStatus::Failed);
}

#[test]
fn classifies_legacy_tool_errors_through_the_host_wrapper() {
    let tools = super::classify_tools(vec![
        Box::new(LegacyTool("user denied: write file")),
        Box::new(LegacyTool("MCP tool call timed out")),
    ]);

    assert_eq!(
        tools[0].execute_outcome(&serde_json::json!({})).status,
        ToolExecutionStatus::NeedsApproval
    );
    assert_eq!(
        tools[1].execute_outcome(&serde_json::json!({})).status,
        ToolExecutionStatus::Retryable
    );
}
