use mini_agent_capabilities::{ApprovalController, ApprovalFailure, ResultStore};
use mini_agent_protocol::{
    Tool, ToolAdmission, ToolApprovalRequest, ToolExecutionDelegate, ToolExecutionOutcome,
    ToolExecutionRequest,
};
use serde_json::json;

const CORE_TOOL_OUTPUT_BYTES: usize = 16 * 1024;
const OUTPUT_FAILURE_PREVIEW_BYTES: usize = 6 * 1024;

/// Typed tools perform bounded validation and describe their admission need;
/// this orchestrator owns approval and the post-admission execution boundary.
/// Tools that still return `Legacy` retain their existing lifecycle until a
/// later migration slice.
pub struct ToolOrchestrator {
    approval: ApprovalController,
    project_instructions: Option<crate::project_context::ProjectInstructionLoader>,
    results: Option<ResultStore>,
}

impl ToolOrchestrator {
    pub fn new(approval: ApprovalController) -> Self {
        Self {
            approval,
            project_instructions: None,
            results: None,
        }
    }

    pub(crate) fn with_project_instructions(
        mut self,
        loader: crate::project_context::ProjectInstructionLoader,
    ) -> Self {
        self.project_instructions = Some(loader);
        self
    }

    pub(crate) fn with_result_store(mut self, results: ResultStore) -> Self {
        self.results = Some(results);
        self
    }

    fn persist_large_output(&self, mut outcome: ToolExecutionOutcome) -> ToolExecutionOutcome {
        if outcome.content.len() <= CORE_TOOL_OUTPUT_BYTES {
            return outcome;
        }
        let source_bytes = outcome.content.len();
        let metadata = Some(json!({"kind": "tool_output"}));
        let stored = self
            .results
            .as_ref()
            .ok_or_else(|| {
                mini_agent_protocol::ToolError(
                    "Session artifact storage is unavailable".to_string(),
                )
            })
            .and_then(|results| {
                results.store_with_metadata(outcome.content.clone(), source_bytes, false, metadata)
            });
        match stored {
            Ok(stored) => {
                let retained_notice = if stored.source_truncated {
                    " The source exceeded the 8 MiB artifact limit; the retained artifact contains a head-and-tail excerpt."
                } else {
                    ""
                };
                outcome.content = format!(
                    "Tool output exceeded the inline limit and was truncated for context. Full output handle: {}. Read it with read_tool_output(handle: \"{}\", cursor: 0); continue with each next_cursor.{}\n\n{}",
                    stored.handle, stored.handle, retained_notice, stored.preview,
                );
                outcome.output_truncated = true;
            }
            Err(error) => {
                let reason = ResultStore::bounded_preview(&error.to_string(), 512);
                let preview =
                    ResultStore::bounded_preview(&outcome.content, OUTPUT_FAILURE_PREVIEW_BYTES);
                outcome.content = format!(
                    "Tool output exceeded the inline limit. The complete output was not retained because Session artifact storage failed: {reason}\nSource bytes: {source_bytes}. Only this head-and-tail preview remains available:\n\n{preview}"
                );
                outcome.output_truncated = true;
            }
        }
        outcome
    }

    fn enforce_plan_selection(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<(), ToolExecutionOutcome> {
        let Some(session_dir) = self.approval.session_dir() else {
            return Ok(());
        };
        let selection = crate::goal::plan_mode_tool_selection(
            Some(&session_dir),
            std::slice::from_ref(&request.name),
        )
        .map_err(|_| {
            ToolExecutionOutcome::deferred(
                "Plan Mode state could not be verified; this tool call was not executed. Retry after restoring the Session state.",
            )
        })?;
        let Some(selection) = selection else {
            return Ok(());
        };
        if selection.review_pending {
            return Err(ToolExecutionOutcome::deferred(
                "Plan review is pending; this tool call was not executed. Review the plan, then continue.",
            ));
        }
        if !selection
            .allowed_tools
            .iter()
            .any(|name| name == &request.name)
        {
            return Err(ToolExecutionOutcome::deferred(
                "Plan Mode deferred this tool call without execution because it is outside the active tool selection. Choose an allowed planning tool or exit Plan Mode.",
            ));
        }
        Ok(())
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
        if let Err(outcome) = self.enforce_plan_selection(request) {
            return outcome;
        }
        let outcome = match tool.admission(request) {
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
        };
        self.persist_large_output(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_root;
    use mini_agent_capabilities::{ApprovalPolicy, SessionRequest, SessionStore};
    use mini_agent_protocol::{ApprovalOutcome, ToolError, ToolHandler, ToolRuntime, ToolSpec};
    use serde_json::{Value, json};
    use std::fs;

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
    fn plan_mode_defers_tools_outside_the_session_selection() {
        let session_dir = test_root();
        let session_file = session_dir.join("session.jsonl");
        fs::write(&session_file, "").unwrap();
        crate::goal::init_plan_mode_with_prompt(&session_dir, None).unwrap();
        let approval = ApprovalController::new(ApprovalPolicy::Automatic);
        approval.bind_session_file(&session_file);
        let orchestrator = ToolOrchestrator::new(approval);

        let outcome = orchestrator.execute(
            &FixtureTool {
                name: "delegate_task".to_string(),
                admission: ToolAdmission::Allowed {
                    target_paths: Vec::new(),
                },
                outcome: ToolExecutionOutcome::completed("must not run"),
            },
            &request("delegate_task"),
        );

        assert_eq!(
            outcome.status,
            mini_agent_protocol::ToolExecutionStatus::Deferred
        );
        assert!(
            outcome
                .content
                .contains("deferred this tool call without execution")
        );
        fs::remove_dir_all(session_dir).unwrap();
    }

    #[test]
    fn oversized_tool_output_is_stored_before_core_truncation() {
        let results = ResultStore::default();
        let full_output = format!("head:{}:tail", "x".repeat(40 * 1024));
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic))
                .with_result_store(results.clone());

        let outcome = orchestrator.execute(
            &FixtureTool {
                name: "shell".to_string(),
                admission: ToolAdmission::Allowed {
                    target_paths: Vec::new(),
                },
                outcome: ToolExecutionOutcome::completed(full_output.clone()),
            },
            &request("shell"),
        );

        assert!(outcome.output_truncated);
        assert!(outcome.content.len() < CORE_TOOL_OUTPUT_BYTES);
        let handle = outcome
            .content
            .split("Full output handle: ")
            .nth(1)
            .unwrap()
            .split('.')
            .next()
            .unwrap();
        let page = results.read_page(handle, 0, 16 * 1024).unwrap();
        assert_eq!(page.content, full_output[..16 * 1024]);
        assert_eq!(page.metadata.unwrap()["kind"], "tool_output");
    }

    #[test]
    fn failed_artifact_write_keeps_a_bounded_head_tail_notice() {
        let root = test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_path = opened.store.path().to_path_buf();
        let results = opened.store.result_store();
        drop(opened);
        fs::remove_file(session_path).unwrap();
        let orchestrator =
            ToolOrchestrator::new(ApprovalController::new(ApprovalPolicy::Automatic))
                .with_result_store(results.clone());
        let full_output = format!("head:{}:tail", "x".repeat(40 * 1024));

        let outcome = orchestrator.execute(
            &FixtureTool {
                name: "shell".to_string(),
                admission: ToolAdmission::Allowed {
                    target_paths: Vec::new(),
                },
                outcome: ToolExecutionOutcome::completed(full_output),
            },
            &request("shell"),
        );

        assert!(outcome.output_truncated);
        assert!(outcome.content.len() <= CORE_TOOL_OUTPUT_BYTES);
        assert!(outcome.content.contains("complete output was not retained"));
        assert!(results.read_page("result-1", 0, 1024).is_err());
        fs::remove_dir_all(root).unwrap();
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
