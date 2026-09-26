use crate::SessionOperation;
use mini_agent_protocol::{Tool, ToolError, ToolHandler, ToolRuntime, ToolSpec};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CHILD_ID_BYTES: usize = 64;
const MAX_CHILD_PROMPT_BYTES: usize = 32 * 1024;
const MAX_OPERATION_GROUP_ID_BYTES: usize = 128;
const MAX_CHILD_REPORT_BYTES: usize = 4 * 1024;
const MAX_REPORT_PAGE: usize = 32;
const MAX_REPORT_PAGE_BYTES: usize = 10 * 1024;
const MAX_TASK_LIST_PAGE: usize = 32;
const MAX_THREAD_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHILD_TASK_RESULT_CHARS: usize = 512;

/// Host-side request for WebStudio to create an independent child runtime.
/// The tool only returns a bounded request; the Gateway observes the event and
/// calls the normal Session fork/control seam.
struct DelegateTaskTool;

impl ToolHandler for DelegateTaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delegate_task".to_string(),
            description: "Queue one bounded task for an independent child Session. Choose parallel for independent work or sequential with a group_id and zero-based sequence for dependent work. The child runs with the same Host permissions and can be queried with task_read.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["child_thread_id", "prompt", "execution_mode"],
                "properties": {
                    "child_thread_id": {"type": "string"},
                    "prompt": {"type": "string"},
                    "title": {"type": "string"},
                    "group_id": {"type": "string"},
                    "execution_mode": {"type": "string", "enum": ["parallel", "sequential"]},
                    "sequence": {"type": "integer", "minimum": 0}
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
        let group_id = arguments.get("group_id").and_then(Value::as_str);
        if let Some(group_id) = group_id
            && (group_id.trim().is_empty() || group_id.len() > MAX_OPERATION_GROUP_ID_BYTES)
        {
            return Err(ToolError(
                "delegate_task group_id must be non-empty and bounded".to_string(),
            ));
        }
        let execution_mode = arguments
            .get("execution_mode")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("delegate_task requires execution_mode".to_string()))?;
        if !matches!(execution_mode, "parallel" | "sequential") {
            return Err(ToolError(
                "delegate_task execution_mode must be parallel or sequential".to_string(),
            ));
        }
        if execution_mode == "sequential" && group_id.is_none() {
            return Err(ToolError(
                "delegate_task sequential mode requires group_id".to_string(),
            ));
        }
        let sequence = arguments.get("sequence").and_then(Value::as_u64);
        if execution_mode == "sequential" && sequence.is_none() {
            return Err(ToolError(
                "delegate_task sequential mode requires sequence".to_string(),
            ));
        }
        if arguments.get("sequence").is_some() && sequence.is_none() {
            return Err(ToolError(
                "delegate_task sequence must be a non-negative integer".to_string(),
            ));
        }
        validate_child_id(child_thread_id).map_err(ToolError)?;
        if prompt.trim().is_empty() || prompt.len() > MAX_CHILD_PROMPT_BYTES {
            return Err(ToolError(
                "delegate_task prompt must be non-empty and bounded".to_string(),
            ));
        }
        let mut result = json!({
            "status": "queued",
            "operation_id": format!("child:{child_thread_id}"),
            "child_thread_id": child_thread_id,
            "prompt": prompt,
            "execution_mode": execution_mode,
        });
        if let Some(group_id) = group_id {
            result["group_id"] = json!(group_id);
        }
        if let Some(sequence) = sequence {
            result["sequence"] = json!(sequence);
        }
        serde_json::to_string(&result).map_err(|error| ToolError(error.to_string()))
    }
}

struct TaskReadTool {
    parent_session_dir: PathBuf,
}

struct TaskListTool {
    parent_session_dir: PathBuf,
}

impl ToolHandler for TaskListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_list".to_string(),
            description: "List bounded summaries of child tasks owned by this parent Session. Pages are ordered by child_thread_id; use after_child_thread_id from next_cursor to continue, and task_read for details and reports.".to_string(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "after_child_thread_id":{"type":"string"},
                    "limit":{"type":"integer","minimum":1,"maximum":32}
                },
                "additionalProperties":false
            }),
        }
    }
}

impl ToolRuntime for TaskListTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let parent_session_id = self
            .parent_session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ToolError("parent Session identity is unavailable".to_string()))?;
        let after = arguments
            .get("after_child_thread_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if arguments.get("after_child_thread_id").is_some()
            && arguments["after_child_thread_id"].as_str().is_none()
        {
            return Err(ToolError(
                "task_list after_child_thread_id must be a string".to_string(),
            ));
        }
        if !after.is_empty() {
            validate_child_id(after).map_err(ToolError)?;
        }
        let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(16);
        if arguments.get("limit").is_some() && arguments["limit"].as_u64().is_none() {
            return Err(ToolError("task_list limit must be an integer".to_string()));
        }
        if !(1..=MAX_TASK_LIST_PAGE as u64).contains(&limit) {
            return Err(ToolError(format!(
                "task_list limit must be between 1 and {MAX_TASK_LIST_PAGE}"
            )));
        }
        let base = self
            .parent_session_dir
            .parent()
            .ok_or_else(|| ToolError("parent Session directory is invalid".to_string()))?;
        let index_path = base.join("thread_index.json");
        let index_bytes = fs::metadata(&index_path)
            .map_err(|error| ToolError(format!("cannot inspect child task index: {error}")))?
            .len();
        if index_bytes > MAX_THREAD_INDEX_BYTES {
            return Err(ToolError(
                "child task index exceeds its bounded read limit".to_string(),
            ));
        }
        let index = fs::read_to_string(index_path)
            .map_err(|error| ToolError(format!("cannot read child task index: {error}")))?;
        let index: Value = serde_json::from_str(&index)
            .map_err(|error| ToolError(format!("invalid child task index: {error}")))?;
        let threads = index
            .get("threads")
            .and_then(Value::as_object)
            .ok_or_else(|| ToolError("child task index has no thread map".to_string()))?;
        let canonical_base = base
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve child task index: {error}")))?;
        let mut child_ids = threads
            .keys()
            .filter(|child_id| child_id.as_str() > after)
            .cloned()
            .collect::<Vec<_>>();
        child_ids.sort();
        let mut children = Vec::new();
        let mut next_cursor = None;
        for child_thread_id in child_ids {
            let Some(session_id) = threads
                .get(&child_thread_id)
                .and_then(|entry| entry.get("session_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if validate_child_id(&child_thread_id).is_err()
                || validate_child_id(session_id).is_err()
            {
                continue;
            }
            let path = base.join(session_id).join("session.jsonl");
            let Ok(canonical_path) = path.canonicalize() else {
                continue;
            };
            if !canonical_path.starts_with(&canonical_base) {
                return Err(ToolError(
                    "child Session path escaped the Session root".to_string(),
                ));
            }
            let (summary, _, _) =
                read_child_task(&canonical_path, parent_session_id, 0, false).map_err(ToolError)?;
            let Some(mut summary) = summary else {
                continue;
            };
            summary.as_object_mut().unwrap().remove("result");
            if children.len() == limit as usize {
                next_cursor = children
                    .last()
                    .and_then(|value: &Value| value.get("child_thread_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                break;
            }
            children.push(json!({"child_thread_id":child_thread_id,"operation":summary}));
        }
        serde_json::to_string(&json!({
            "children": children,
            "next_cursor": next_cursor,
            "limit": limit,
        }))
        .map_err(|error| ToolError(error.to_string()))
    }
}

impl ToolHandler for TaskReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_read".to_string(),
            description: "Read bounded status, result, and reports for a child Session. Use the child_thread_id returned by delegate_task and read each task once after dispatch to distinguish running from queued. Poll running children for progress; queued tasks start automatically when a slot frees, so do not repeatedly read them. If a child Session is not materialized yet, skip it and report the missing child ID instead of retrying in a loop.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["child_thread_id"],
                "properties": {
                    "child_thread_id": {"type": "string"},
                    "after_cursor": {"type": "integer", "minimum": 0}
                },
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
        let after_cursor = arguments
            .get("after_cursor")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if arguments.get("after_cursor").is_some() && arguments["after_cursor"].as_u64().is_none() {
            return Err(ToolError(
                "task_read after_cursor must be a non-negative integer".to_string(),
            ));
        }
        let (operation, reports, next_cursor) =
            read_child_task(&canonical_path, parent_session_id, after_cursor, true)
                .map_err(ToolError)?;
        serde_json::to_string(&json!({
            "child_thread_id": child_thread_id,
            "status": operation
                .as_ref()
                .and_then(|value| value.get("status"))
                .cloned()
                .unwrap_or_else(|| json!("idle")),
            "operation": operation,
            "reports": reports,
            "next_cursor": next_cursor,
        }))
        .map_err(|error| ToolError(error.to_string()))
    }
}

struct TaskReportTool {
    session_dir: PathBuf,
    context: crate::ChildTaskContext,
}

impl ToolHandler for TaskReportTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_report".to_string(),
            description: "Send a bounded progress update to the parent agent. Reports are persisted with the child task and can be read by the parent.".to_string(),
            parameters: json!({"type":"object","required":["report"],"properties":{"report":{"type":"string","maxLength":4096}},"additionalProperties":false}),
        }
    }
}

impl ToolRuntime for TaskReportTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let report = arguments
            .get("report")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("task_report requires report".to_string()))?;
        if report.trim().is_empty()
            || report.len() > MAX_CHILD_REPORT_BYTES
            || report
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(ToolError(
                "task_report must be non-empty and at most 4 KiB".to_string(),
            ));
        }
        let attempt = current_child_attempt(
            &self.session_dir.join("session.jsonl"),
            &self.context.operation_id,
        )
        .map_err(ToolError)?;
        serde_json::to_string(&json!({"status":"requested","action":"report","operation_id":self.context.operation_id,"parent_thread_id":self.context.parent_thread_id,"attempt":attempt,"report":report}))
            .map_err(|error| ToolError(error.to_string()))
    }
}

struct TaskControlTool;

impl ToolHandler for TaskControlTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task_control".to_string(),
            description: "Request Gateway-mediated control of a delegated child task. For every per-child action, use the current child_thread_id, operation_id, and positive attempt from task_list or task_read. This tool returns an intent, not completion; the Gateway performs it and sends a bounded outcome to the parent at a safe Turn boundary or idle continuation. Re-read task state before deciding what to do next.".to_string(),
            parameters: json!({
                "type":"object", "required":["action"],
                "properties":{
                    "action":{"type":"string","enum":["update_queued","steer","assign","queue_follow_up","pause","resume","cancel","retry","cancel_group"]},
                    "child_thread_id":{"type":"string"}, "operation_id":{"type":"string"},
                    "attempt":{"type":"integer","minimum":1,"description":"Expected current child operation attempt; required for every per-child action."},
                    "group_id":{"type":"string"}, "prompt":{"type":"string","maxLength":32768},
                    "text":{"type":"string","maxLength":4096}
                }, "additionalProperties":false
            }),
        }
    }
}

impl ToolRuntime for TaskControlTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("task_control requires action".to_string()))?;
        if !matches!(
            action,
            "update_queued"
                | "steer"
                | "assign"
                | "queue_follow_up"
                | "pause"
                | "resume"
                | "cancel"
                | "retry"
                | "cancel_group"
        ) {
            return Err(ToolError("task_control action is invalid".to_string()));
        }
        let attempt = if action == "cancel_group" {
            None
        } else {
            let child_thread_id = arguments
                .get("child_thread_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolError(format!("task_control {action} requires child_thread_id"))
                })?;
            validate_child_id(child_thread_id).map_err(ToolError)?;
            let operation_id = arguments
                .get("operation_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError(format!("task_control {action} requires operation_id")))?;
            if operation_id.trim().is_empty() || operation_id.len() > 128 {
                return Err(ToolError(format!(
                    "task_control {action} operation_id must be non-empty and bounded"
                )));
            }
            let attempt = arguments
                .get("attempt")
                .and_then(Value::as_u64)
                .and_then(|attempt| u32::try_from(attempt).ok())
                .filter(|attempt| *attempt > 0)
                .ok_or_else(|| {
                    ToolError(format!("task_control {action} requires a positive attempt"))
                })?;
            Some(attempt)
        };
        if matches!(action, "update_queued" | "assign" | "queue_follow_up") {
            let prompt = arguments
                .get("prompt")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError(format!("task_control {action} requires prompt")))?;
            if prompt.trim().is_empty() || prompt.len() > MAX_CHILD_PROMPT_BYTES {
                return Err(ToolError(format!(
                    "task_control {action} prompt must be non-empty and at most 32 KiB"
                )));
            }
        }
        if action == "steer" {
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError("task_control steer requires text".to_string()))?;
            if text.trim().is_empty() || text.len() > 4 * 1024 {
                return Err(ToolError(
                    "task_control steer text must be non-empty and at most 4 KiB".to_string(),
                ));
            }
        }
        if action == "cancel_group" {
            let group_id = arguments
                .get("group_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ToolError("task_control cancel_group requires group_id".to_string())
                })?;
            if group_id.trim().is_empty() || group_id.len() > MAX_OPERATION_GROUP_ID_BYTES {
                return Err(ToolError(
                    "task_control cancel_group group_id must be non-empty and bounded".to_string(),
                ));
            }
        }
        let mut intent = json!({"status":"requested","action":action});
        if let Some(attempt) = attempt {
            intent["attempt"] = json!(attempt);
        }
        for key in [
            "child_thread_id",
            "operation_id",
            "group_id",
            "prompt",
            "text",
        ] {
            if let Some(value) = arguments.get(key) {
                intent[key] = value.clone();
            }
        }
        serde_json::to_string(&intent).map_err(|error| ToolError(error.to_string()))
    }
}

pub fn child_task_tools(
    session_dir: PathBuf,
    child: Option<crate::ChildTaskContext>,
) -> Vec<Box<dyn Tool>> {
    if let Some(context) = child {
        vec![Box::new(TaskReportTool {
            session_dir,
            context,
        })]
    } else {
        vec![
            Box::new(DelegateTaskTool),
            Box::new(TaskReadTool {
                parent_session_dir: session_dir.clone(),
            }),
            Box::new(TaskListTool {
                parent_session_dir: session_dir,
            }),
            Box::new(TaskControlTool),
        ]
    }
}

fn current_child_attempt(path: &Path, operation_id: &str) -> Result<u32, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read child Session: {error}"))?;
    bytes
        .split(|byte| *byte == b'\n')
        .rev()
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find(|record| {
            record.get("kind").and_then(Value::as_str) == Some("operation")
                && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
                && record.get("operation_id").and_then(Value::as_str) == Some(operation_id)
        })
        .and_then(|record| record.get("attempt")?.as_u64()?.try_into().ok())
        .ok_or_else(|| "child task attempt was not found".to_string())
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

fn read_child_task(
    path: &Path,
    parent_session_id: &str,
    after_cursor: u64,
    include_reports: bool,
) -> Result<(Option<Value>, Vec<Value>, u64), String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read child Session: {error}"))?;
    let mut valid_lineage = false;
    let mut operation_id: Option<String> = None;
    let mut latest = None;
    let mut follow_up = None;
    let mut control = None;
    let mut reports = Vec::new();
    let mut next_cursor = after_cursor;
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
        if valid_lineage
            && record.get("kind").and_then(Value::as_str) == Some("operation")
            && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
        {
            operation_id = record
                .get("operation_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let Ok(operation_record) = serde_json::from_value::<SessionOperation>(record.clone())
            else {
                continue;
            };
            let mut bounded =
                serde_json::to_value(operation_record).expect("SessionOperation is serializable");
            for key in ["prompt", "result", "error"] {
                if let Some(value) = record.get(key).and_then(Value::as_str) {
                    bounded[key] = json!(
                        value
                            .chars()
                            .take(MAX_CHILD_TASK_RESULT_CHARS)
                            .collect::<String>()
                    );
                }
            }
            control = match record.get("status").and_then(Value::as_str) {
                Some("pausing" | "cancelling") => Some(json!({
                    "action": if record["status"] == "pausing" { "pause" } else { "cancel_active" },
                    "status": if record["status"] == "pausing" { "pending" } else { "accepted" },
                    "request_id": record.get("control_request_id").and_then(Value::as_str),
                })),
                _ => None,
            };
            latest = Some(bounded);
        }
        if valid_lineage
            && record.get("kind").and_then(Value::as_str) == Some("child_control_request")
            && record.get("operation_id").and_then(Value::as_str) == operation_id.as_deref()
        {
            let action = record.get("action").and_then(Value::as_str);
            if action == Some("queue_follow_up") {
                follow_up = Some(json!({
                    "status": record.get("request_status").and_then(Value::as_str).unwrap_or("accepted"),
                    "prompt": record.get("prompt").and_then(Value::as_str)
                        .map(|prompt| prompt.chars().take(256).collect::<String>()),
                    "request_id": record.get("request_id").and_then(Value::as_str),
                }));
            } else if matches!(action, Some("pause" | "cancel_active")) {
                control = Some(json!({
                    "action": action,
                    "status": record.get("request_status").and_then(Value::as_str),
                    "request_id": record.get("request_id").and_then(Value::as_str),
                }));
            }
        }
        if !include_reports
            || !valid_lineage
            || record.get("kind").and_then(Value::as_str) != Some("child_report")
            || record.get("operation_id").and_then(Value::as_str) != operation_id.as_deref()
        {
            continue;
        }
        let cursor = record
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if cursor <= after_cursor {
            continue;
        }
        let report = json!({
            "cursor": cursor,
            "report_id": record.get("report_id").and_then(Value::as_str).unwrap_or_default(),
            "attempt": record.get("attempt").and_then(Value::as_u64).unwrap_or_default(),
            "timestamp_ms": record.get("timestamp_ms").and_then(Value::as_u64).unwrap_or_default(),
            "report": record.get("report").and_then(Value::as_str).unwrap_or_default(),
        });
        let report_bytes = serde_json::to_vec(&report)
            .map_err(|error| format!("cannot encode child report: {error}"))?
            .len();
        let existing_bytes = serde_json::to_vec(&reports)
            .map_err(|error| format!("cannot encode child report page: {error}"))?
            .len();
        if reports.len() == MAX_REPORT_PAGE
            || existing_bytes + report_bytes + 1 > MAX_REPORT_PAGE_BYTES
        {
            break;
        }
        next_cursor = cursor;
        reports.push(report);
    }
    if let Some(latest) = latest.as_mut() {
        if let Some(follow_up) = follow_up
            .filter(|request| matches!(request["status"].as_str(), Some("accepted" | "blocked")))
        {
            latest["follow_up"] = follow_up;
        }
        if let Some(control) = control
            .filter(|request| matches!(request["status"].as_str(), Some("pending" | "accepted")))
        {
            latest["control"] = control;
        }
    }
    Ok((latest, reports, next_cursor))
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
                "prompt": "inspect the module",
                "execution_mode": "parallel"
            }))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["status"], "queued");
        assert_eq!(value["operation_id"], "child:child-1");
        assert_eq!(value["execution_mode"], "parallel");
    }

    #[test]
    fn delegate_task_preserves_main_thread_scheduling_intent() {
        let tool = DelegateTaskTool;
        let result = tool
            .execute(&json!({
                "child_thread_id": "child-2",
                "prompt": "apply the reviewed change",
                "group_id": "refactor",
                "execution_mode": "sequential",
                "sequence": 2
            }))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["group_id"], "refactor");
        assert_eq!(value["execution_mode"], "sequential");
        assert_eq!(value["sequence"], 2);
    }

    #[test]
    fn delegate_task_rejects_missing_execution_mode() {
        let error = DelegateTaskTool
            .execute(&json!({
                "child_thread_id": "child-3",
                "prompt": "inspect the module"
            }))
            .expect_err("delegation must declare its scheduling mode");
        assert_eq!(error.0, "delegate_task requires execution_mode");
    }

    #[test]
    fn delegate_task_rejects_sequential_group_without_sequence() {
        let error = DelegateTaskTool
            .execute(&json!({
                "child_thread_id": "child-4",
                "prompt": "inspect the module",
                "group_id": "ordered-review",
                "execution_mode": "sequential"
            }))
            .expect_err("sequential tasks need their explicit queue position");
        assert_eq!(error.0, "delegate_task sequential mode requires sequence");
    }

    #[test]
    fn task_control_returns_gateway_intents_without_executing_them() {
        assert!(
            TaskControlTool
                .spec()
                .description
                .contains("sends a bounded outcome to the parent")
        );
        let result = TaskControlTool.execute(&json!({
            "action":"steer","child_thread_id":"child-1","operation_id":"child:child-1","attempt":2,"text":"inspect the failing test"
        })).unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["status"], "requested");
        assert_eq!(value["action"], "steer");
        assert_eq!(value["attempt"], 2);
        assert_eq!(value["text"], "inspect the failing test");
        let pause = TaskControlTool.execute(&json!({
            "action":"pause","child_thread_id":"child-1","operation_id":"child:child-1","attempt":2
        })).unwrap();
        assert!(!pause.contains("turn_id"));
        assert!(
            TaskControlTool
                .execute(&json!({
                    "action":"cancel","child_thread_id":"child-1","operation_id":"child:child-1"
                }))
                .is_err()
        );
        assert!(
            TaskControlTool
                .execute(&json!({"action":"cancel_group","group_id":"review"}))
                .is_ok()
        );
        assert!(
            TaskControlTool
                .execute(&json!({"action":"destroy"}))
                .is_err()
        );
    }

    #[test]
    fn task_list_paginates_child_summaries_without_reports() {
        let root = crate::test_support::test_root();
        let parent_session_dir = root.join("parent-session");
        fs::create_dir_all(&parent_session_dir).unwrap();
        let mut threads = serde_json::Map::new();
        for (child_id, session_id, parent_id) in [
            ("child-c", "session-c", "parent-session"),
            ("child-a", "session-a", "parent-session"),
            ("child-b", "session-b", "parent-session"),
            ("other-child", "session-other", "other-parent"),
        ] {
            threads.insert(
                child_id.to_string(),
                json!({"session_id":session_id,"updated_at_ms":1}),
            );
            let session_dir = root.join(session_id);
            fs::create_dir_all(&session_dir).unwrap();
            fs::write(
                session_dir.join("session.jsonl"),
                [
                    json!({"kind":"session_created","forked_from":{"parent_session_id":parent_id}}),
                    json!({"kind":"operation","operation_kind":"child_task","operation_id":format!("child:{child_id}"),"parent_thread_id":"parent-thread","status":"running","attempt":1,"prompt":"short prompt","timestamp_ms":1}),
                    json!({"kind":"child_report","operation_id":format!("child:{child_id}"),"report":"private detail"}),
                ]
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            )
            .unwrap();
        }
        fs::write(
            root.join("thread_index.json"),
            json!({"threads":threads}).to_string(),
        )
        .unwrap();

        let tool = TaskListTool { parent_session_dir };
        let first: Value =
            serde_json::from_str(&tool.execute(&json!({"limit":2})).unwrap()).unwrap();
        assert_eq!(first["children"].as_array().unwrap().len(), 2);
        assert_eq!(first["children"][0]["child_thread_id"], "child-a");
        assert_eq!(first["children"][1]["child_thread_id"], "child-b");
        assert_eq!(first["next_cursor"], "child-b");
        assert_eq!(first["children"][0]["operation"]["status"], "running");
        assert!(first["children"][0]["operation"].get("reports").is_none());

        let second: Value = serde_json::from_str(
            &tool
                .execute(&json!({"limit":2,"after_child_thread_id":"child-b"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(second["children"].as_array().unwrap().len(), 1);
        assert_eq!(second["children"][0]["child_thread_id"], "child-c");
        assert!(second["next_cursor"].is_null());
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn task_report_uses_the_current_attempt_after_a_retry() {
        let root = crate::test_support::test_root();
        fs::write(
            root.join("session.jsonl"),
            json!({"kind":"operation","operation_kind":"child_task","operation_id":"child:one","attempt":2}).to_string(),
        )
        .unwrap();
        let tool = TaskReportTool {
            session_dir: root.clone(),
            context: crate::ChildTaskContext {
                parent_thread_id: "parent".to_string(),
                operation_id: "child:one".to_string(),
                attempt: 1,
            },
        };
        let result = tool
            .execute(&json!({"report":"retry is progressing"}))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["attempt"], 2);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn task_read_report_pages_keep_unreturned_records_reachable() {
        let root = crate::test_support::test_root();
        let path = root.join("child.jsonl");
        let mut records = vec![
            json!({"seq":1,"kind":"session_created","forked_from":{"parent_session_id":"parent"}}),
            json!({"seq":2,"kind":"operation","operation_kind":"child_task","operation_id":"child:one"}),
        ];
        for seq in 3..38 {
            records.push(json!({"seq":seq,"kind":"child_report","operation_id":"child:one","report_id":format!("r{seq}"),"attempt":1,"timestamp_ms":seq,"report":format!("report {seq}")}));
        }
        fs::write(
            &path,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let (_, first, cursor) = read_child_task(&path, "parent", 2, true).unwrap();
        assert_eq!(first.len(), MAX_REPORT_PAGE);
        assert_eq!(cursor, 34);
        let (_, second, next) = read_child_task(&path, "parent", cursor, true).unwrap();
        assert_eq!(second.len(), 3);
        assert_eq!(second[0]["report"], "report 35");
        assert_eq!(next, 37);
        crate::test_support::remove_test_root(&root);
    }
}
