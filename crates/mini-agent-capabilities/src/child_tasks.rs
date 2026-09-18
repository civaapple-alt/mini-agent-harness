use mini_agent_protocol::{Tool, ToolError, ToolHandler, ToolRuntime, ToolSpec};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CHILD_ID_BYTES: usize = 64;
const MAX_CHILD_PROMPT_BYTES: usize = 32 * 1024;
const MAX_CHILD_RESULT_BYTES: usize = 16 * 1024;

/// Host-side request for WebStudio to create an independent child runtime.
/// The tool only returns a bounded request; the Gateway observes the event and
/// calls the normal Session fork/control seam.
struct DelegateTaskTool;

impl ToolHandler for DelegateTaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delegate_task".to_string(),
            description: "Queue one bounded task for an independent child Session. The child runs with the same Host permissions and can be queried with task_read.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["child_thread_id", "prompt"],
                "properties": {
                    "child_thread_id": {"type": "string"},
                    "prompt": {"type": "string"},
                    "title": {"type": "string"}
                },
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for DelegateTaskTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let child_thread_id = arguments
            .get("child_thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("delegate_task requires child_thread_id".to_string()))?;
        let prompt = arguments
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("delegate_task requires prompt".to_string()))?;
        validate_child_id(child_thread_id).map_err(ToolError)?;
        if prompt.trim().is_empty() || prompt.len() > MAX_CHILD_PROMPT_BYTES {
            return Err(ToolError(
                "delegate_task prompt must be non-empty and bounded".to_string(),
            ));
        }
        serde_json::to_string(&json!({
            "status": "queued",
            "operation_id": format!("child:{child_thread_id}"),
            "child_thread_id": child_thread_id,
            "prompt": prompt,
        }))
        .map_err(|error| ToolError(error.to_string()))
    }
}

struct TaskReadTool {
    parent_session_dir: PathBuf,
}

impl ToolHandler for TaskReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_read".to_string(),
            description: "Read the bounded status and result of a child Session task. Use the child_thread_id returned by delegate_task.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["child_thread_id"],
                "properties": {"child_thread_id": {"type": "string"}},
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for TaskReadTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let child_thread_id = arguments
            .get("child_thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("task_read requires child_thread_id".to_string()))?;
        validate_child_id(child_thread_id).map_err(ToolError)?;
        let parent_session_id = self
            .parent_session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ToolError("parent Session identity is unavailable".to_string()))?;
        let base = self
            .parent_session_dir
            .parent()
            .ok_or_else(|| ToolError("parent Session directory is invalid".to_string()))?;
        let index = fs::read_to_string(base.join("thread_index.json"))
            .map_err(|error| ToolError(format!("cannot read child task index: {error}")))?;
        let index: Value = serde_json::from_str(&index)
            .map_err(|error| ToolError(format!("invalid child task index: {error}")))?;
        let session_id = index
            .get("threads")
            .and_then(|threads| threads.get(child_thread_id))
            .and_then(|entry| entry.get("session_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("child task was not found".to_string()))?;
        validate_child_id(session_id).map_err(ToolError)?;
        let canonical_base = base
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve child task index: {error}")))?;
        let path = base.join(session_id).join("session.jsonl");
        let canonical_path = path
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve child Session: {error}")))?;
        if !canonical_path.starts_with(&canonical_base) {
            return Err(ToolError(
                "child Session path escaped the Session root".to_string(),
            ));
        }
        let operation =
            read_child_operation(&canonical_path, parent_session_id).map_err(ToolError)?;
        serde_json::to_string(&json!({
            "child_thread_id": child_thread_id,
            "status": operation
                .as_ref()
                .and_then(|value| value.get("status"))
                .cloned()
                .unwrap_or_else(|| json!("idle")),
            "operation": operation,
        }))
        .map_err(|error| ToolError(error.to_string()))
    }
}

pub fn child_task_tools(session_dir: PathBuf) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(DelegateTaskTool),
        Box::new(TaskReadTool {
            parent_session_dir: session_dir,
        }),
    ]
}

fn validate_child_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_CHILD_ID_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("child_thread_id must contain only bounded ASCII id characters".to_string());
    }
    Ok(())
}

fn read_child_operation(path: &Path, parent_session_id: &str) -> Result<Option<Value>, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read child Session: {error}"))?;
    let mut valid_lineage = false;
    let mut latest = None;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.len() > 64 * 1024 {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if record.get("kind").and_then(Value::as_str) == Some("session_created") {
            valid_lineage = record
                .get("forked_from")
                .and_then(|value| value.get("parent_session_id"))
                .and_then(Value::as_str)
                == Some(parent_session_id);
        }
        if valid_lineage && record.get("kind").and_then(Value::as_str) == Some("operation") {
            let mut bounded = serde_json::Map::new();
            for key in [
                "operation_id",
                "operation_kind",
                "status",
                "turn_id",
                "operation_group_id",
                "execution_mode",
                "prompt",
            ] {
                if let Some(value) = record.get(key).and_then(Value::as_str) {
                    let value = if key == "prompt" {
                        value
                            .chars()
                            .take(MAX_CHILD_PROMPT_BYTES)
                            .collect::<String>()
                    } else {
                        value.to_string()
                    };
                    bounded.insert(key.to_string(), json!(value));
                }
            }
            if let Some(sequence) = record.get("group_sequence").and_then(Value::as_u64) {
                bounded.insert("group_sequence".to_string(), json!(sequence));
            }
            if let Some(attempt) = record.get("attempt").and_then(Value::as_u64) {
                bounded.insert("attempt".to_string(), json!(attempt));
            }
            for key in ["result", "error"] {
                if let Some(value) = record.get(key).and_then(Value::as_str) {
                    bounded.insert(
                        key.to_string(),
                        json!(
                            value
                                .chars()
                                .take(MAX_CHILD_RESULT_BYTES)
                                .collect::<String>()
                        ),
                    );
                }
            }
            latest = Some(Value::Object(bounded));
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_task_returns_a_bounded_queue_request() {
        let tool = DelegateTaskTool;
        let result = tool
            .execute(&json!({
                "child_thread_id": "child-1",
                "prompt": "inspect the module"
            }))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["status"], "queued");
        assert_eq!(value["operation_id"], "child:child-1");
    }
}
