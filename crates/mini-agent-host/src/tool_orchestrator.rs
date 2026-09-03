use mini_agent_capabilities::ApprovalController;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolAdmission;
use mini_agent_protocol::ToolApprovalRequest;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolExecutionStatus;

/// Typed tools perform bounded validation and describe their admission need;
/// this orchestrator owns approval and the post-admission execution boundary.
/// Tools that still return `Legacy` retain their existing lifecycle until a
/// later migration slice.
pub struct ToolOrchestrator {
    approval: ApprovalController,
}

impl ToolOrchestrator {
    pub fn new(approval: ApprovalController) -> Self {
        Self { approval }
    }
}

impl ToolExecutionDelegate for ToolOrchestrator {
    fn execute(&self, tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        let outcome = match tool.admission(request) {
            Ok(ToolAdmission::Legacy) => tool.execute_outcome(&request.arguments),
            Ok(ToolAdmission::ApprovalRequired { action }) => {
                let approval_request = ToolApprovalRequest::from_execution(action, request);
                match self.approval.approve_request(&approval_request) {
                    Ok(()) => tool.execute_after_admission(request),
                    Err(error) => ToolExecutionOutcome::failed(error.to_string()),
                }
            }
            Err(error) => ToolExecutionOutcome::failed(error.to_string()),
        };
        classify_outcome(outcome)
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
