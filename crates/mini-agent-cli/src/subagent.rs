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

pub struct SpawnAgent(pub Arc<Workspace>);

impl SpawnAgent {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self(workspace)
    }
}

impl Tool for SpawnAgent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_agent".to_string(),
            description: "Spawn an isolated subagent child process to perform a dedicated subtask in the workspace. Returns the subagent's structured result and execution status.".to_string(),
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
        let message = string_arg(args, "message")?;
        if task_name.trim().is_empty() {
            return Err(ToolError("task_name cannot be empty".to_string()));
        }
        if message.trim().is_empty() {
            return Err(ToolError("message cannot be empty".to_string()));
        }

        self.0.approve(&format!("spawn subagent `{task_name}`"))?;

        let model = args.get("model").and_then(|v| v.as_str());
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
            .arg("--json")
            .arg("--auto")
            .arg("--security-preset")
            .arg(self.0.approval.preset().as_str())
            .arg("--sandbox")
            .arg(self.0.sandbox.as_str());

        if let Some(m) = model {
            cmd.env("OPENAI_MODEL", m);
        }

        let output = run_sandboxed_command(
            cmd,
            &self.0.root,
            self.0.sandbox,
            Duration::from_secs(timeout_secs),
        )?;

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

            if exit_code == 0 && error.is_empty() {
                Ok(format!(
                    "Subagent '{task_name}' completed (in {steps} steps):\n\n{final_output}"
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
                    "Subagent '{task_name}' failed (exit code {exit_code}):\n{err_msg}\n\nPartial output:\n{final_output}"
                ))
            }
        } else {
            // Fallback for raw text output
            if output.exit_code == Some(0) {
                Ok(format!(
                    "Subagent '{task_name}' completed:\n\n{}",
                    output.raw_stdout.trim()
                ))
            } else {
                Ok(format!(
                    "Subagent '{task_name}' exited with error:\n{}\n{}",
                    output.raw_stdout.trim(),
                    output.raw_stderr.trim()
                ))
            }
        }
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
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

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
        assert!(spec.parameters["properties"]["model"].is_object());
        assert!(spec.parameters["properties"]["timeout_seconds"].is_object());
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
}
