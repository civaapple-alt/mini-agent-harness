use mini_agent_capabilities::{ApprovalController, ApprovalFailure};
use mini_agent_protocol::{
    Tool, ToolAdmission, ToolApprovalRequest, ToolExecutionDelegate, ToolExecutionOutcome,
    ToolExecutionRequest,
};

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
        match tool.admission(request) {
            Ok(ToolAdmission::Legacy) => tool.execute_outcome(&request.arguments),
            Ok(ToolAdmission::Allowed) => tool.execute_after_admission(request),
            Ok(ToolAdmission::Deferred { reason }) => ToolExecutionOutcome::deferred(reason),
            Ok(ToolAdmission::ApprovalRequired {
                action,
                target_paths,
                action_summary,
            }) => {
                let approval_request = ToolApprovalRequest::from_execution_with_summary(
                    action,
                    action_summary,
                    target_paths,
                    request,
                );
                match self
                    .approval
                    .approve_request_with_classification(&approval_request)
                {
                    Ok(()) => tool.execute_after_admission(request),
                    Err(ApprovalFailure::UserDenied(error)) => {
                        ToolExecutionOutcome::needs_approval(error)
                    }
                    Err(error) => ToolExecutionOutcome::failed(error.to_string()),
                }
            }
            Err(error) => ToolExecutionOutcome::failed(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_capabilities::ApprovalPolicy;
    use mini_agent_protocol::{ApprovalOutcome, ToolError, ToolHandler, ToolRuntime, ToolSpec};
    use serde_json::{Value, json};

    struct FixtureTool {
        admission: ToolAdmission,
        outcome: ToolExecutionOutcome,
    }

    impl ToolHandler for FixtureTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "fixture".to_string(),
                description: "fixture".to_string(),
                parameters: json!({"type": "object"}),
            }
        }

        fn admission(&self, _request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
            Ok(self.admission.clone())
        }
    }

    impl ToolRuntime for FixtureTool {
        fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
            Ok(self.outcome.content.clone())
        }

        fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
            self.outcome.clone()
        }
    }

    fn request() -> ToolExecutionRequest {
        ToolExecutionRequest::new("call-1", "fixture", json!({}))
    }

    #[test]
    fn preserves_explicit_deferred_admission() {
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic));
        let outcome = orchestrator.execute(
            &FixtureTool {
                admission: ToolAdmission::Deferred {
                    reason: "plan lock".to_string(),
                },
                outcome: ToolExecutionOutcome::completed("must not run"),
            },
            &request(),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::Deferred
        );
        assert_eq!(outcome.content, "plan lock");
    }

    #[test]
    fn maps_typed_approval_denial_to_needs_approval() {
        let approval = ApprovalController::with_callback(ApprovalPolicy::Interactive, |_| {
            Ok(mini_agent_protocol::ToolApprovalResolution::once(
                ApprovalOutcome::Denied,
            ))
        });
        let orchestrator = ToolOrchestrator::new(approval);
        let outcome = orchestrator.execute(
            &FixtureTool {
                admission: ToolAdmission::ApprovalRequired {
                    action: "run fixture".to_string(),
                    target_paths: Vec::new(),
                    action_summary: None,
                },
                outcome: ToolExecutionOutcome::completed("must not run"),
            },
            &request(),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::NeedsApproval
        );
        assert_eq!(outcome.content, "user denied: run fixture");
    }

    #[test]
    fn does_not_reclassify_legacy_failure_text() {
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic));
        let outcome = orchestrator.execute(
            &FixtureTool {
                admission: ToolAdmission::Legacy,
                outcome: ToolExecutionOutcome::failed("MCP tool call timed out"),
            },
            &request(),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::Failed
        );
    }
}
