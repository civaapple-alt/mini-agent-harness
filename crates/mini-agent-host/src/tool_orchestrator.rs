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
    project_instructions: Option<crate::project_context::ProjectInstructionLoader>,
}

impl ToolOrchestrator {
    pub fn new(approval: ApprovalController) -> Self {
        Self {
            approval,
            project_instructions: None,
        }
    }

    pub(crate) fn with_project_instructions(
        mut self,
        loader: crate::project_context::ProjectInstructionLoader,
    ) -> Self {
        self.project_instructions = Some(loader);
        self
    }

    fn instruction_outcome(
        &self,
        request: &ToolExecutionRequest,
        admission: &ToolAdmission,
    ) -> Option<ToolExecutionOutcome> {
        if !matches!(
            request.name.as_str(),
            "read_file" | "read_image" | "apply_patch"
        ) {
            return None;
        }
        let target_paths = admission.target_paths()?;
        let loader = self.project_instructions.as_ref()?;
        match loader.applicable_injections(target_paths, &request.known_context_injections) {
            Ok(injections) if injections.is_empty() => None,
            Ok(injections) => {
                let records = injections
                    .iter()
                    .map(|injection| injection.record.clone())
                    .collect::<Vec<_>>();
                let messages = injections
                    .iter()
                    .map(|injection| injection.message.clone())
                    .collect::<Vec<_>>();
                let sources = injections
                    .iter()
                    .map(|injection| {
                        let path = injection.record.path.as_deref().unwrap_or("AGENTS.md");
                        format!(
                            "{}:{path}",
                            injection.record.workspace.as_deref().unwrap_or("工作区")
                        )
                    })
                    .collect::<Vec<_>>();
                for injection in &injections {
                    if let Some(warning) = &injection.warning {
                        eprintln!("warning: {warning}");
                    }
                }
                Some(
                    ToolExecutionOutcome::deferred(format!(
                        "Host injected applicable workspace instructions from {}. Review them, then retry the operation.",
                        sources.join("、")
                    ))
                    .with_context_injection(messages, records),
                )
            }
            Err(error) => Some(ToolExecutionOutcome::failed(error)),
        }
    }
}

impl ToolExecutionDelegate for ToolOrchestrator {
    fn execute(&self, tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        match tool.admission(request) {
            Ok(ToolAdmission::Legacy) => tool.execute_outcome(&request.arguments),
            Ok(admission @ ToolAdmission::Allowed { .. }) => self
                .instruction_outcome(request, &admission)
                .unwrap_or_else(|| tool.execute_after_admission(request, &admission)),
            Ok(ToolAdmission::Deferred { reason }) => ToolExecutionOutcome::deferred(reason),
            Ok(admission @ ToolAdmission::ApprovalRequired { .. }) => {
                if let Some(outcome) = self.instruction_outcome(request, &admission) {
                    return outcome;
                }
                let ToolAdmission::ApprovalRequired {
                    action,
                    target_paths,
                    action_summary,
                } = &admission
                else {
                    unreachable!("matched ApprovalRequired")
                };
                let approval_request = ToolApprovalRequest::from_execution_with_summary(
                    action.clone(),
                    action_summary.clone(),
                    target_paths.clone(),
                    request,
                );
                match self
                    .approval
                    .approve_request_with_classification(&approval_request)
                {
                    Ok(()) => tool.execute_after_admission(request, &admission),
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
    use crate::test_support::test_root;
    use mini_agent_capabilities::ApprovalPolicy;
    use mini_agent_protocol::{ApprovalOutcome, ToolError, ToolHandler, ToolRuntime, ToolSpec};
    use serde_json::{Value, json};

    struct FixtureTool {
        name: String,
        admission: ToolAdmission,
        outcome: ToolExecutionOutcome,
    }

    impl ToolHandler for FixtureTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.clone(),
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

    fn request(name: &str) -> ToolExecutionRequest {
        ToolExecutionRequest::new("call-1", name, json!({}))
    }

    #[test]
    fn preserves_explicit_deferred_admission() {
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic));
        let outcome = orchestrator.execute(
            &FixtureTool {
                name: "fixture".to_string(),
                admission: ToolAdmission::Deferred {
                    reason: "plan lock".to_string(),
                },
                outcome: ToolExecutionOutcome::completed("must not run"),
            },
            &request("fixture"),
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
                name: "fixture".to_string(),
                admission: ToolAdmission::ApprovalRequired {
                    action: "run fixture".to_string(),
                    target_paths: Vec::new(),
                    action_summary: None,
                },
                outcome: ToolExecutionOutcome::completed("must not run"),
            },
            &request("fixture"),
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
                name: "fixture".to_string(),
                admission: ToolAdmission::Legacy,
                outcome: ToolExecutionOutcome::failed("MCP tool call timed out"),
            },
            &request("fixture"),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::Failed
        );
    }

    #[test]
    fn shell_does_not_scan_workspace_instruction_paths() {
        let root = test_root();
        std::fs::write(root.join("AGENTS.md"), "must not scan for shell").unwrap();
        let loader =
            crate::project_context::ProjectInstructionLoader::from_configured_roots(&root, &[]);
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic))
                .with_project_instructions(loader);

        let outcome = orchestrator.execute(
            &FixtureTool {
                name: "shell".to_string(),
                admission: ToolAdmission::Allowed {
                    target_paths: vec![root.join("AGENTS.md").display().to_string()],
                },
                outcome: ToolExecutionOutcome::completed("shell ran"),
            },
            &request("shell"),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::Completed
        );
        assert!(outcome.context_injections.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
