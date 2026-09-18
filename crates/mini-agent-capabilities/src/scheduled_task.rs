use mini_agent_protocol::{
    Tool, ToolAdmission, ToolError, ToolExecutionOutcome, ToolExecutionRequest, ToolHandler,
    ToolRuntime, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_TASK_ID_BYTES: usize = 128;
const MAX_SUMMARY_BYTES: usize = 512;
const MAX_DELAY_SECONDS: u64 = 24 * 60 * 60;

/// A bounded, runtime-scoped marker that tells the model when it may perform
/// the next check. It never executes Shell commands or model prompts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledTask {
    pub task_id: String,
    pub owner_thread_id: String,
    pub state: String,
    pub trigger_type: String,
    pub summary: String,
    pub created_at: u64,
    pub due_at: u64,
    pub ready_at: Option<u64>,
    pub cancelled_at: Option<u64>,
}

#[derive(Clone)]
pub struct ScheduledTaskManager {
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    owner_thread_id: Option<String>,
    allow_tasks: bool,
    tasks: HashMap<String, ScheduledTask>,
}

impl ScheduledTaskManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerState {
                owner_thread_id: None,
                allow_tasks: true,
                tasks: HashMap::new(),
            })),
        }
    }

    pub fn bind_owner(&self, thread_id: impl Into<String>) {
        if let Ok(mut state) = self.inner.lock() {
            state.owner_thread_id = Some(thread_id.into());
        }
    }

    pub fn disable(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.allow_tasks = false;
        }
    }

    pub fn list(&self, owner_thread_id: &str) -> Result<Vec<ScheduledTask>, String> {
        self.ensure_owner(owner_thread_id)?;
        let mut state = self.lock_state()?;
        refresh_due(&mut state.tasks);
        let mut tasks = state.tasks.values().cloned().collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        Ok(tasks)
    }

    pub fn read(&self, owner_thread_id: &str, task_id: &str) -> Result<ScheduledTask, String> {
        self.ensure_owner(owner_thread_id)?;
        let mut state = self.lock_state()?;
        refresh_due(&mut state.tasks);
        state
            .tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| format!("scheduled task `{task_id}` was not found"))
    }

    pub fn create_delay(
        &self,
        owner_thread_id: &str,
        task_id: &str,
        delay_seconds: u64,
        summary: &str,
    ) -> Result<ScheduledTask, String> {
        self.ensure_enabled()?;
        self.ensure_owner(owner_thread_id)?;
        validate_task_id(task_id)?;
        if !(1..=MAX_DELAY_SECONDS).contains(&delay_seconds) {
            return Err(format!(
                "delay_seconds must be between 1 and {MAX_DELAY_SECONDS}"
            ));
        }
        let summary = validate_summary(summary)?;
        let mut state = self.lock_state()?;
        refresh_due(&mut state.tasks);
        if let Some(existing) = state.tasks.get(task_id) {
            return Ok(existing.clone());
        }
        let now = timestamp_ms();
        let task = ScheduledTask {
            task_id: task_id.to_string(),
            owner_thread_id: owner_thread_id.to_string(),
            state: "scheduled".to_string(),
            trigger_type: "delay".to_string(),
            summary,
            created_at: now,
            due_at: now.saturating_add(Duration::from_secs(delay_seconds).as_millis() as u64),
            ready_at: None,
            cancelled_at: None,
        };
        state.tasks.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    pub fn cancel(&self, owner_thread_id: &str, task_id: &str) -> Result<ScheduledTask, String> {
        self.ensure_owner(owner_thread_id)?;
        let mut state = self.lock_state()?;
        refresh_due(&mut state.tasks);
        let task = state
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| format!("scheduled task `{task_id}` was not found"))?;
        if task.state == "scheduled" {
            task.state = "cancelled".to_string();
            task.cancelled_at = Some(timestamp_ms());
        }
        Ok(task.clone())
    }

    pub fn close_all(&self) -> Result<(), String> {
        let mut state = self.lock_state()?;
        state.tasks.clear();
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, ManagerState>, String> {
        self.inner
            .lock()
            .map_err(|_| "scheduled task state is unavailable".to_string())
    }

    fn ensure_owner(&self, owner_thread_id: &str) -> Result<(), String> {
        let state = self.lock_state()?;
        if state
            .owner_thread_id
            .as_deref()
            .is_some_and(|owner| owner != owner_thread_id)
        {
            return Err("scheduled task belongs to another Thread runtime".to_string());
        }
        Ok(())
    }

    fn ensure_enabled(&self) -> Result<(), String> {
        let state = self.lock_state()?;
        if !state.allow_tasks {
            return Err("scheduled tasks are disabled for this runtime".to_string());
        }
        Ok(())
    }
}

impl Default for ScheduledTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Model-facing control for bounded wake-up markers. A later model turn can
/// read a task and perform one remote status query without holding a Shell.
pub fn scheduled_task_tools(manager: ScheduledTaskManager) -> Vec<Box<dyn Tool>> {
    vec![Box::new(ScheduledTaskTool { manager })]
}

struct ScheduledTaskTool {
    manager: ScheduledTaskManager,
}

impl ToolHandler for ScheduledTaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "scheduled_task".to_string(),
            description: "Schedule a bounded wake-up marker without running Shell or a model prompt. Use it instead of sleeping in a foreground Shell while waiting to check a remote task; on a later turn read the task and perform one status check.".to_string(),
            parameters: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {"type": "string", "enum": ["create", "read", "list", "cancel"]},
                    "task_id": {"type": "string"},
                    "delay_seconds": {"type": "integer", "minimum": 1, "maximum": MAX_DELAY_SECONDS},
                    "summary": {"type": "string"}
                },
                "additionalProperties": false
            }),
        }
    }

    fn admission(&self, _request: &ToolExecutionRequest) -> Result<ToolAdmission, ToolError> {
        Ok(ToolAdmission::Allowed)
    }
}

impl ToolRuntime for ScheduledTaskTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        self.execute_for_owner(arguments, "default")
    }

    fn execute_after_admission(&self, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        let owner = request
            .context
            .as_ref()
            .map(|context| context.thread_id.as_str())
            .unwrap_or("default");
        self.execute_for_owner(&request.arguments, owner)
            .map_or_else(
                |error| ToolExecutionOutcome::failed(error.to_string()),
                ToolExecutionOutcome::completed,
            )
    }
}

impl ScheduledTaskTool {
    fn execute_for_owner(&self, arguments: &Value, owner: &str) -> Result<String, ToolError> {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("scheduled_task requires action".to_string()))?;
        let task_id = arguments.get("task_id").and_then(Value::as_str);
        let task = match action {
            "create" => {
                let task_id = task_id.ok_or_else(|| {
                    ToolError("scheduled_task create requires task_id".to_string())
                })?;
                let delay_seconds = arguments
                    .get("delay_seconds")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        ToolError("scheduled_task create requires delay_seconds".to_string())
                    })?;
                let summary = arguments
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("scheduled status check");
                self.manager
                    .create_delay(owner, task_id, delay_seconds, summary)
                    .map_err(ToolError)?
            }
            "read" => self
                .manager
                .read(
                    owner,
                    task_id.ok_or_else(|| {
                        ToolError("scheduled_task read requires task_id".to_string())
                    })?,
                )
                .map_err(ToolError)?,
            "cancel" => self
                .manager
                .cancel(
                    owner,
                    task_id.ok_or_else(|| {
                        ToolError("scheduled_task cancel requires task_id".to_string())
                    })?,
                )
                .map_err(ToolError)?,
            "list" => {
                let tasks = self.manager.list(owner).map_err(ToolError)?;
                return serde_json::to_string(&json!({"status": "ok", "data": tasks}))
                    .map_err(|error| ToolError(error.to_string()));
            }
            _ => return Err(ToolError("unknown scheduled_task action".to_string())),
        };
        serde_json::to_string(&json!({"status": task.state, "task": task}))
            .map_err(|error| ToolError(error.to_string()))
    }
}

fn refresh_due(tasks: &mut HashMap<String, ScheduledTask>) {
    let now = timestamp_ms();
    for task in tasks.values_mut() {
        if task.state == "scheduled" && task.due_at <= now {
            task.state = "ready".to_string();
            task.ready_at = Some(now);
        }
    }
}

fn validate_task_id(task_id: &str) -> Result<(), String> {
    if task_id.is_empty()
        || task_id.len() > MAX_TASK_ID_BYTES
        || !task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "task_id must contain only letters, numbers, '.', '_' or '-' and be at most {MAX_TASK_ID_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_summary(summary: &str) -> Result<String, String> {
    if summary.trim().is_empty() || summary.len() > MAX_SUMMARY_BYTES {
        return Err(format!(
            "summary must contain 1..={MAX_SUMMARY_BYTES} bytes"
        ));
    }
    Ok(summary.to_string())
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn delay_task_becomes_ready_without_blocking_a_turn() {
        let manager = ScheduledTaskManager::new();
        manager.bind_owner("thread-1");
        let task = manager
            .create_delay("thread-1", "check-run", 1, "check GitHub Action")
            .unwrap();
        assert_eq!(task.state, "scheduled");
        thread::sleep(Duration::from_millis(1_050));
        assert_eq!(
            manager.read("thread-1", "check-run").unwrap().state,
            "ready"
        );
    }

    #[test]
    fn duplicate_task_id_is_idempotent_and_cancel_is_local() {
        let manager = ScheduledTaskManager::new();
        manager.bind_owner("thread-1");
        let first = manager
            .create_delay("thread-1", "check-run", 10, "first")
            .unwrap();
        let second = manager
            .create_delay("thread-1", "check-run", 20, "second")
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            manager.cancel("thread-1", "check-run").unwrap().state,
            "cancelled"
        );
    }

    #[test]
    fn child_or_disabled_runtime_cannot_create_task() {
        let manager = ScheduledTaskManager::new();
        manager.bind_owner("child-thread");
        manager.disable();
        assert!(
            manager
                .create_delay("child-thread", "child-task", 1, "not allowed")
                .is_err()
        );
    }
}
