use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolExecutionStatus;

/// Owns the host-side execution boundary while legacy tools are migrated.
///
/// This first slice delegates the actual call to the existing tool and keeps
/// outcome classification in one place. Approval and sandbox admission move
/// here in later slices; the tool remains the source of tool-specific parsing
/// and side-effect behavior until then.
#[derive(Default)]
pub(crate) struct ToolOrchestrator;

impl ToolExecutionDelegate for ToolOrchestrator {
    fn execute(&self, tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        classify_outcome(tool.execute_outcome(&request.arguments))
    }
}

fn classify_outcome(outcome: ToolExecutionOutcome) -> ToolExecutionOutcome {
    if outcome.status != ToolExecutionStatus::Failed {
        return outcome;
    }
    let status = if outcome.content.starts_with("user denied:")
        || outcome
            .content
            .starts_with("denied non-interactive action:")
    {
        ToolExecutionStatus::NeedsApproval
    } else if outcome
        .content
        .starts_with("workspace mutations locked in Plan Mode")
    {
        ToolExecutionStatus::Deferred
    } else if outcome.content.contains("circuit breaker is open")
        || outcome.content.contains("timed out")
    {
        ToolExecutionStatus::Retryable
    } else {
        return outcome;
    };
    ToolExecutionOutcome { status, ..outcome }
}
