use crate::workspace::Workspace;
use crate::workspace::run_sandboxed_command;
use crate::workspace::string_arg;
use mini_agent_core::Tool;
use mini_agent_core::ToolError;
use mini_agent_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub struct SpawnAgent(pub Arc<Workspace>);

impl SpawnAgent {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self(workspace)
    }
}

pub struct SendSubagentMessage(pub Arc<Workspace>);

impl SendSubagentMessage {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self(workspace)
    }
}

pub struct ListSubagents(pub Arc<Workspace>);

impl ListSubagents {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self(workspace)
    }
}

pub fn subagent_tools(workspace: Arc<Workspace>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(SpawnAgent::new(Arc::clone(&workspace))),
        Box::new(SendSubagentMessage::new(Arc::clone(&workspace))),
        Box::new(ListSubagents::new(workspace)),
    ]
}

fn sanitize_task_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed.chars().take(32).collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn record_subagent_tree_meta(
    root: &std::path::Path,
    child_id: &str,
    task_name: &str,
    agent_type: &str,
    persona: Option<&str>,
    started_at_ms: u64,
    completed_at_ms: u64,
    steps: u64,
    exit_code: i64,
    output: &str,
    error: &str,
    review_stats: Option<crate::persona::ReviewStats>,
) {
    let subagents_dir = root.join(".agents/sessions").join(child_id);
    let _ = std::fs::create_dir_all(&subagents_dir);
    let meta = json!({
        "subagent_id": child_id,
        "task_name": task_name,
        "agent_type": agent_type,
        "persona": persona,
        "started_at_ms": started_at_ms,
        "completed_at_ms": completed_at_ms,
        "duration_ms": completed_at_ms.saturating_sub(started_at_ms),
        "steps": steps,
        "exit_code": exit_code,
        "status": if exit_code == 0 && error.is_empty() { "completed" } else { "failed" },
        "review_stats": review_stats.map(|s| json!({
            "open": s.open,
            "fixed": s.fixed,
            "wontfix": s.wontfix,
            "addressed": s.addressed,
        })),
    });
    let _ = std::fs::write(
        subagents_dir.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap_or_default(),
    );
    let out = json!({
        "output": output,
        "error": error,
    });
    let _ = std::fs::write(
        subagents_dir.join("output.json"),
        serde_json::to_vec_pretty(&out).unwrap_or_default(),
    );
}

impl Tool for SpawnAgent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_agent".to_string(),
            description: "Spawn an isolated subagent child process to perform a dedicated subtask in the workspace. Returns the subagent's structured result, execution status, and session_id for multi-turn follow-ups.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_name": {
                        "type": "string",
                        "description": "Descriptive task identifier (e.g. 'breaking_changes_review', 'unit_tester')"
                    },
                    "message": {
                        "type": "string",
                        "description": "Initial task prompt and instructions for the child agent"
                    },
                    "agent_type": {
                        "type": "string",
                        "enum": ["explore", "plan", "general"],
                        "description": "Subagent preset role: 'explore' (fast read-only search), 'plan' (plan-only, no deliverable), or 'general' (full execution). Default: 'general'"
                    },
                    "persona": {
                        "type": "string",
                        "enum": [
                            "reviewer",
                            "implementer",
                            "security-auditor",
                            "test-writer",
                            "researcher",
                            "design-doc-writer",
                            "design-doc-reviewer"
                        ],
                        "description": "Specialized persona behavior and contract preset (e.g. 'reviewer', 'implementer', 'security-auditor', 'test-writer')"
                    },
                    "review_file": {
                        "type": "string",
                        "description": "Optional file path for structured review notes or issues (e.g. '.agents/scratch/review-123.md')"
                    },
                    "summary_file": {
                        "type": "string",
                        "description": "Optional file path for implementation deliverables and summaries"
                    },
                    "fork_context": {
                        "type": "boolean",
                        "description": "Whether to inherit parent settled context snapshot (default: true for general/plan, false for explore)"
                    },
                    "persist": {
                        "type": "boolean",
                        "description": "Whether to persist session checkpoint for multi-turn follow-ups (default: true)"
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model override for the subagent"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Maximum execution time in seconds (default: 120, min: 10, max: 600)"
                    }
                },
                "required": ["task_name", "message"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let task_name = string_arg(args, "task_name")?;
        let raw_message = string_arg(args, "message")?;
        if task_name.trim().is_empty() {
            return Err(ToolError("task_name cannot be empty".to_string()));
        }
        if raw_message.trim().is_empty() {
            return Err(ToolError("message cannot be empty".to_string()));
        }

        self.0.approve(&format!("spawn subagent `{task_name}`"))?;

        let agent_type = args
            .get("agent_type")
            .and_then(Value::as_str)
            .unwrap_or("general");
        let persona = args.get("persona").and_then(Value::as_str);
        let review_file = args.get("review_file").and_then(Value::as_str);
        let summary_file = args.get("summary_file").and_then(Value::as_str);
        let persist = args.get("persist").and_then(Value::as_bool).unwrap_or(true);
        let model = args.get("model").and_then(|v| v.as_str());
        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(120)
            .clamp(10, 600);

        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let message = crate::persona::render_subagent_prompt(
            Some(agent_type),
            persona,
            raw_message,
            review_file,
            summary_file,
        );

        let current_exe = std::env::current_exe()
            .map_err(|e| ToolError(format!("cannot locate mini-agent executable: {e}")))?;

        let session_id = if persist {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let safe_name = sanitize_task_name(task_name);
            Some(format!("sub-{timestamp}-{safe_name}"))
        } else {
            None
        };

        let mut cmd = Command::new(current_exe);
        cmd.arg("ask")
            .arg(&message)
            .arg("--json")
            .arg("--auto")
            .arg("--security-preset")
            .arg(self.0.approval.preset().as_str())
            .arg("--sandbox")
            .arg(self.0.sandbox.as_str());

        if let Some(ref sid) = session_id {
            cmd.arg("--session-id").arg(sid);
        }

        if let Some(m) = model {
            cmd.env("OPENAI_MODEL", m);
        }

        let output = run_sandboxed_command(
            cmd,
            &self.0.root,
            self.0.sandbox,
            Duration::from_secs(timeout_secs),
        )?;

        let completed_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        if output.timed_out {
            return Err(ToolError(format!(
                "subagent '{task_name}' timed out after {timeout_secs} seconds"
            )));
        }

        // Attempt to parse structured JSON emitted by mini-agent ask --json
        if let Ok(json_val) = serde_json::from_str::<Value>(output.raw_stdout.trim()) {
            let exit_code = json_val
                .get("exit_code")
                .and_then(|c| c.as_i64())
                .unwrap_or(0);
            let final_output = json_val
                .get("output")
                .and_then(|o| o.as_str())
                .unwrap_or("");
            let steps = json_val.get("steps").and_then(|s| s.as_u64()).unwrap_or(1);
            let error = json_val.get("error").and_then(|e| e.as_str()).unwrap_or("");
            let returned_sid = json_val
                .get("session_id")
                .and_then(|s| s.as_str())
                .or(session_id.as_deref());

            let review_stats = if let Some(rf) = review_file {
                let candidate = self.0.root.join(rf);
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    Some(crate::persona::parse_review_stats(&content))
                } else {
                    Some(crate::persona::parse_review_stats(final_output))
                }
            } else {
                None
            };

            if let Some(sid) = returned_sid {
                record_subagent_tree_meta(
                    &self.0.root,
                    sid,
                    task_name,
                    agent_type,
                    persona,
                    started_at_ms,
                    completed_at_ms,
                    steps,
                    exit_code,
                    final_output,
                    error,
                    review_stats,
                );
            }

            let session_info = match returned_sid {
                Some(sid) => format!(" [session_id: {sid}]"),
                None => String::new(),
            };

            let review_info = match review_stats {
                Some(stats)
                    if stats.open > 0
                        || stats.fixed > 0
                        || stats.wontfix > 0
                        || stats.addressed > 0 =>
                {
                    format!(" [{stats}]")
                }
                _ => String::new(),
            };

            if exit_code == 0 && error.is_empty() {
                Ok(format!(
                    "Subagent '{task_name}'{session_info}{review_info} completed (in {steps} steps):\n\n{final_output}"
                ))
            } else {
                let err_msg = if !error.is_empty() {
                    error
                } else if !output.raw_stderr.trim().is_empty() {
                    output.raw_stderr.trim()
                } else {
                    "subagent failed without error details"
                };
                Ok(format!(
                    "Subagent '{task_name}'{session_info}{review_info} failed (exit code {exit_code}):\n{err_msg}\n\nPartial output:\n{final_output}"
                ))
            }
        } else {
            // Fallback for raw text output
            let session_info = match session_id {
                Some(ref sid) => {
                    record_subagent_tree_meta(
                        &self.0.root,
                        sid,
                        task_name,
                        agent_type,
                        persona,
                        started_at_ms,
                        completed_at_ms,
                        1,
                        output.exit_code.unwrap_or(0) as i64,
                        output.raw_stdout.trim(),
                        output.raw_stderr.trim(),
                        None,
                    );
                    format!(" [session_id: {sid}]")
                }
                None => String::new(),
            };
            if output.exit_code == Some(0) {
                Ok(format!(
                    "Subagent '{task_name}'{session_info} completed:\n\n{}",
                    output.raw_stdout.trim()
                ))
            } else {
                Ok(format!(
                    "Subagent '{task_name}'{session_info} exited with error:\n{}\n{}",
                    output.raw_stdout.trim(),
                    output.raw_stderr.trim()
                ))
            }
        }
    }
}

impl Tool for SendSubagentMessage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_subagent_message".to_string(),
            description: "Send a follow-up message or instruction to an existing subagent session. Resumes the subagent's conversation history and returns the updated result.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Durable session ID of the subagent (e.g. 'sub-1724750000-reviewer')"
                    },
                    "message": {
                        "type": "string",
                        "description": "Follow-up message, critique, or instruction"
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Maximum execution time in seconds (default: 120, min: 10, max: 600)"
                    }
                },
                "required": ["session_id", "message"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let session_id = string_arg(args, "session_id")?;
        let message = string_arg(args, "message")?;
        if session_id.trim().is_empty() {
            return Err(ToolError("session_id cannot be empty".to_string()));
        }
        if message.trim().is_empty() {
            return Err(ToolError("message cannot be empty".to_string()));
        }

        self.0
            .approve(&format!("send message to subagent `{session_id}`"))?;

        let timeout_secs = args
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(120)
            .clamp(10, 600);

        let current_exe = std::env::current_exe()
            .map_err(|e| ToolError(format!("cannot locate mini-agent executable: {e}")))?;

        let mut cmd = Command::new(current_exe);
        cmd.arg("ask")
            .arg(message)
            .arg("--session-id")
            .arg(session_id)
            .arg("--json")
            .arg("--auto")
            .arg("--security-preset")
            .arg(self.0.approval.preset().as_str())
            .arg("--sandbox")
            .arg(self.0.sandbox.as_str());

        let output = run_sandboxed_command(
            cmd,
            &self.0.root,
            self.0.sandbox,
            Duration::from_secs(timeout_secs),
        )?;

        if output.timed_out {
            return Err(ToolError(format!(
                "subagent session '{session_id}' timed out after {timeout_secs} seconds"
            )));
        }

        // Parse structured JSON emitted by mini-agent ask --json
        if let Ok(json_val) = serde_json::from_str::<Value>(output.raw_stdout.trim()) {
            let exit_code = json_val
                .get("exit_code")
                .and_then(|c| c.as_i64())
                .unwrap_or(0);
            let final_output = json_val
                .get("output")
                .and_then(|o| o.as_str())
                .unwrap_or("");
            let steps = json_val.get("steps").and_then(|s| s.as_u64()).unwrap_or(1);
            let error = json_val.get("error").and_then(|e| e.as_str()).unwrap_or("");

            if exit_code == 0 && error.is_empty() {
                Ok(format!(
                    "Subagent [{session_id}] completed follow-up turn (in {steps} steps):\n\n{final_output}"
                ))
            } else {
                let err_msg = if !error.is_empty() {
                    error
                } else if !output.raw_stderr.trim().is_empty() {
                    output.raw_stderr.trim()
                } else {
                    "subagent follow-up failed without error details"
                };
                Ok(format!(
                    "Subagent [{session_id}] failed follow-up turn (exit code {exit_code}):\n{err_msg}\n\nPartial output:\n{final_output}"
                ))
            }
        } else {
            // Fallback
            if output.exit_code == Some(0) {
                Ok(format!(
                    "Subagent [{session_id}] completed follow-up turn:\n\n{}",
                    output.raw_stdout.trim()
                ))
            } else {
                Ok(format!(
                    "Subagent [{session_id}] exited with error:\n{}\n{}",
                    output.raw_stdout.trim(),
                    output.raw_stderr.trim()
                ))
            }
        }
    }
}

impl Tool for ListSubagents {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_subagents".to_string(),
            description: "List all active and recent subagent sessions in the workspace."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, _args: &Value) -> Result<String, ToolError> {
        let sessions = crate::session::list(&self.0.root)
            .map_err(|e| ToolError(format!("cannot list subagent sessions: {e}")))?;
        let subagent_sessions: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.id.starts_with("sub-"))
            .collect();

        if subagent_sessions.is_empty() {
            return Ok("No subagent sessions found in this workspace.".to_string());
        }

        let mut output = String::from("| Session ID | Size (bytes) |\n|---|---|\n");
        for session in subagent_sessions {
            output.push_str(&format!("| `{}` | {} |\n", session.id, session.bytes));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxKind;
    use crate::security::SecurityPreset;
    use crate::workspace::ApprovalController;
    use crate::workspace::ApprovalMode;
    use std::fs;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    fn test_root() -> std::path::PathBuf {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mini-agent-subagent-{nonce}-{sequence}"));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn spawn_agent_spec_has_expected_fields() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = SpawnAgent::new(workspace);
        let spec = tool.spec();
        assert_eq!(spec.name, "spawn_agent");
        assert!(spec.description.contains("subagent"));
        assert!(spec.parameters["properties"]["task_name"].is_object());
        assert!(spec.parameters["properties"]["message"].is_object());
        assert!(spec.parameters["properties"]["agent_type"].is_object());
        assert!(spec.parameters["properties"]["persona"].is_object());
        assert!(spec.parameters["properties"]["review_file"].is_object());
        assert!(spec.parameters["properties"]["summary_file"].is_object());
        assert!(spec.parameters["properties"]["fork_context"].is_object());
        assert!(spec.parameters["properties"]["persist"].is_object());
        assert!(spec.parameters["properties"]["model"].is_object());
        assert!(spec.parameters["properties"]["timeout_seconds"].is_object());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_subagent_tree_meta_creates_files() {
        let root = test_root();
        let stats = crate::persona::ReviewStats {
            open: 0,
            fixed: 2,
            wontfix: 0,
            addressed: 0,
        };
        record_subagent_tree_meta(
            &root,
            "sub-123-reviewer",
            "reviewer",
            "general",
            Some("reviewer"),
            1000,
            2000,
            4,
            0,
            "all tests pass",
            "",
            Some(stats),
        );
        let subagent_dir = root.join(".agents/sessions/sub-123-reviewer");
        assert!(subagent_dir.join("meta.json").is_file());
        assert!(subagent_dir.join("output.json").is_file());
        let meta: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(subagent_dir.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["agent_type"], "general");
        assert_eq!(meta["persona"], "reviewer");
        assert_eq!(meta["duration_ms"], 1000);
        assert_eq!(meta["status"], "completed");
        assert_eq!(meta["review_stats"]["fixed"], 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn send_subagent_message_spec_has_expected_fields() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = SendSubagentMessage::new(workspace);
        let spec = tool.spec();
        assert_eq!(spec.name, "send_subagent_message");
        assert!(spec.parameters["properties"]["session_id"].is_object());
        assert!(spec.parameters["properties"]["message"].is_object());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_subagents_spec_has_expected_fields() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = ListSubagents::new(workspace);
        let spec = tool.spec();
        assert_eq!(spec.name, "list_subagents");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn spawn_agent_rejects_empty_task_name_or_message() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = SpawnAgent::new(workspace);
        assert!(
            tool.execute(&json!({"task_name": "", "message": "hello"}))
                .is_err()
        );
        assert!(
            tool.execute(&json!({"task_name": "test", "message": "   "}))
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn send_subagent_message_rejects_empty_session_id() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = SendSubagentMessage::new(workspace);
        assert!(
            tool.execute(&json!({"session_id": "", "message": "hello"}))
                .is_err()
        );
        assert!(
            tool.execute(&json!({"session_id": "sub-123", "message": "   "}))
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
