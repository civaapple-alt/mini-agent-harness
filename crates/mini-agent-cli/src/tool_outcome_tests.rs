use super::classify_outcome;
use mini_agent_core::ToolExecutionOutcome;
use mini_agent_core::ToolExecutionStatus;

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
