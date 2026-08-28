use super::ToolExecutionOutcome;
use super::ToolExecutionRequest;
use super::ToolExecutionStatus;
use crate::ToolCall;
use serde_json::json;

#[test]
fn execution_request_converts_from_tool_call() {
    let request = ToolExecutionRequest::from(ToolCall {
        id: "call-1".to_string(),
        name: "read_file".to_string(),
        arguments: json!({"path": "README.md"}),
    });

    assert_eq!(
        request,
        ToolExecutionRequest::new("call-1", "read_file", json!({"path": "README.md"}))
    );
}

#[test]
fn execution_statuses_round_trip_without_collapsing_policy() {
    for status in [
        ToolExecutionStatus::Completed,
        ToolExecutionStatus::Failed,
        ToolExecutionStatus::NeedsApproval,
        ToolExecutionStatus::Deferred,
        ToolExecutionStatus::Retryable,
    ] {
        let outcome = ToolExecutionOutcome {
            status,
            content: "detail".to_string(),
        };
        let encoded = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            serde_json::from_value::<ToolExecutionOutcome>(encoded).unwrap(),
            outcome
        );
    }
}
