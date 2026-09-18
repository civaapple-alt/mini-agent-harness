use crate::sandbox::{ProcessSandbox, SandboxKind};
use crate::workspace::shell_command;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_TASK_ID_BYTES: usize = 128;
const MAX_COMMAND_SUMMARY_BYTES: usize = 256;
const MAX_LOG_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct BackgroundShellTask {
    pub task_id: String,
    pub owner_thread_id: String,
    pub state: String,
    pub command_summary: String,
    pub command_hash: String,
    pub working_directory: String,
    pub process_id: Option<u32>,
    pub started_at: u64,
    pub stopped_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub log_bytes: usize,
    pub log_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct BackgroundShellLogs {
    pub task_id: String,
    pub text: String,
    pub bytes: usize,
    pub truncated: bool,
}

#[derive(Clone)]
pub struct BackgroundShellManager {
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    owner_thread_id: Option<String>,
    allow_tasks: bool,
    tasks: HashMap<String, Arc<TaskHandle>>,
}

struct TaskHandle {
    task: Mutex<BackgroundShellTask>,
    command: String,
    working_directory: PathBuf,
    sandbox: ProcessSandbox,
    child: Mutex<Option<Child>>,
    logs: Arc<Mutex<BoundedLog>>,
    stop_requested: AtomicBool,
}

#[derive(Default)]
struct BoundedLog {
    bytes: VecDeque<u8>,
    total_bytes: usize,
    truncated: bool,
}

impl BackgroundShellManager {
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

    pub fn list(&self, owner_thread_id: &str) -> Result<Vec<BackgroundShellTask>, String> {
        self.ensure_owner(owner_thread_id)?;
        let state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        let mut tasks = state
            .tasks
            .values()
            .map(|task| task.snapshot())
            .collect::<Result<Vec<_>, _>>()?;
        tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        Ok(tasks)
    }

    pub fn start(
        &self,
        owner_thread_id: &str,
        task_id: &str,
        command: &str,
        working_directory: &Path,
        sandbox_kind: SandboxKind,
    ) -> Result<BackgroundShellTask, String> {
        self.ensure_enabled()?;
        self.ensure_owner(owner_thread_id)?;
        validate_task_id(task_id)?;
        validate_command(command)?;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        if let Some(existing) = state.tasks.get(task_id) {
            return existing.snapshot();
        }
        let task = spawn_task(
            owner_thread_id,
            task_id,
            command,
            working_directory,
            sandbox_kind,
        )?;
        state.tasks.insert(task_id.to_string(), task.clone());
        task.snapshot()
    }

    pub fn read(
        &self,
        owner_thread_id: &str,
        task_id: &str,
    ) -> Result<BackgroundShellTask, String> {
        self.ensure_owner(owner_thread_id)?;
        self.task(task_id)?.snapshot()
    }

    pub fn logs(
        &self,
        owner_thread_id: &str,
        task_id: &str,
    ) -> Result<BackgroundShellLogs, String> {
        self.ensure_owner(owner_thread_id)?;
        let task = self.task(task_id)?;
        let snapshot = task.snapshot()?;
        let log = task
            .logs
            .lock()
            .map_err(|_| "background shell logs are unavailable".to_string())?;
        Ok(BackgroundShellLogs {
            task_id: snapshot.task_id,
            text: String::from_utf8_lossy(&log.bytes.iter().copied().collect::<Vec<_>>())
                .into_owned(),
            bytes: log.bytes.len(),
            truncated: log.truncated,
        })
    }

    pub fn stop(
        &self,
        owner_thread_id: &str,
        task_id: &str,
    ) -> Result<BackgroundShellTask, String> {
        self.ensure_owner(owner_thread_id)?;
        let task = self.task(task_id)?;
        task.stop("stopped")?;
        task.snapshot()
    }

    pub fn restart(
        &self,
        owner_thread_id: &str,
        task_id: &str,
        sandbox_kind: SandboxKind,
    ) -> Result<BackgroundShellTask, String> {
        self.ensure_owner(owner_thread_id)?;
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        let old = state
            .tasks
            .remove(task_id)
            .ok_or_else(|| format!("background task `{task_id}` was not found"))?;
        old.stop("stopped")?;
        let task = spawn_task(
            owner_thread_id,
            task_id,
            &old.command,
            &old.working_directory,
            sandbox_kind,
        )?;
        state.tasks.insert(task_id.to_string(), task.clone());
        task.snapshot()
    }

    pub fn close_all(&self) -> Result<(), String> {
        let tasks = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?
            .tasks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for task in tasks {
            task.stop("stopped")?;
        }
        if let Ok(mut state) = self.inner.lock() {
            state.tasks.clear();
        }
        Ok(())
    }

    fn ensure_owner(&self, owner_thread_id: &str) -> Result<(), String> {
        let state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        if state
            .owner_thread_id
            .as_deref()
            .is_some_and(|owner| owner != owner_thread_id)
        {
            return Err("background task belongs to another Thread runtime".to_string());
        }
        Ok(())
    }

    fn ensure_enabled(&self) -> Result<(), String> {
        let state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        if !state.allow_tasks {
            return Err("background Shell tasks are disabled for this runtime".to_string());
        }
        Ok(())
    }

    fn task(&self, task_id: &str) -> Result<Arc<TaskHandle>, String> {
        let state = self
            .inner
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        state
            .tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| format!("background task `{task_id}` was not found"))
    }
}

impl Default for BackgroundShellManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundShellManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.close_all();
        }
    }
}

impl TaskHandle {
    fn snapshot(&self) -> Result<BackgroundShellTask, String> {
        let mut task = self
            .task
            .lock()
            .map(|task| task.clone())
            .map_err(|_| "background shell state is unavailable".to_string())?;
        let log = self
            .logs
            .lock()
            .map_err(|_| "background shell logs are unavailable".to_string())?;
        task.log_bytes = log.total_bytes;
        task.log_truncated = log.truncated;
        Ok(task)
    }

    fn stop(&self, final_state: &str) -> Result<(), String> {
        self.stop_requested.store(true, Ordering::Release);
        let mut child = self
            .child
            .lock()
            .map_err(|_| "background shell process is unavailable".to_string())?;
        if let Some(child) = child.as_mut()
            && child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
        {
            let _ = self
                .sandbox
                .terminate(child)
                .map_err(|error| error.to_string())?;
        }
        *child = None;
        let mut task = self
            .task
            .lock()
            .map_err(|_| "background shell state is unavailable".to_string())?;
        task.state = final_state.to_string();
        task.stopped_at = Some(timestamp_ms());
        Ok(())
    }
}

fn spawn_task(
    owner_thread_id: &str,
    task_id: &str,
    command: &str,
    working_directory: &Path,
    sandbox_kind: SandboxKind,
) -> Result<Arc<TaskHandle>, String> {
    let mut process = shell_command(command);
    process
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let sandbox = ProcessSandbox::new(sandbox_kind);
    let mut child = process.spawn().map_err(|error| error.to_string())?;
    sandbox.attach_child(&child);
    let process_id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "background shell stdout is unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "background shell stderr is unavailable".to_string())?;
    let logs = Arc::new(Mutex::new(BoundedLog::default()));
    spawn_log_reader(stdout, Arc::clone(&logs));
    spawn_log_reader(stderr, Arc::clone(&logs));
    let task = Arc::new(TaskHandle {
        task: Mutex::new(BackgroundShellTask {
            task_id: task_id.to_string(),
            owner_thread_id: owner_thread_id.to_string(),
            state: "starting".to_string(),
            command_summary: command_summary(command),
            command_hash: command_hash(command),
            working_directory: working_directory.display().to_string(),
            process_id: Some(process_id),
            started_at: timestamp_ms(),
            stopped_at: None,
            exit_code: None,
            log_bytes: 0,
            log_truncated: false,
        }),
        command: command.to_string(),
        working_directory: working_directory.to_path_buf(),
        sandbox,
        child: Mutex::new(Some(child)),
        logs: Arc::clone(&logs),
        stop_requested: AtomicBool::new(false),
    });
    let monitor = Arc::clone(&task);
    thread::spawn(move || monitor_process(monitor));
    if let Ok(mut record) = task.task.lock() {
        record.state = "running".to_string();
    }
    Ok(task)
}

fn monitor_process(task: Arc<TaskHandle>) {
    loop {
        let status = match task.child.lock() {
            Ok(mut child) => {
                let Some(child) = child.as_mut() else {
                    break;
                };
                child.try_wait().ok().flatten()
            }
            Err(_) => break,
        };
        if let Some(status) = status {
            if let Ok(mut child) = task.child.lock() {
                *child = None;
            }
            if let Ok(mut record) = task.task.lock() {
                record.state = if task.stop_requested.load(Ordering::Acquire) || status.success() {
                    "stopped".to_string()
                } else {
                    "failed".to_string()
                };
                record.exit_code = status.code();
                record.stopped_at = Some(timestamp_ms());
            }
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_log_reader(mut reader: impl Read + Send + 'static, logs: Arc<Mutex<BoundedLog>>) {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if let Ok(mut log) = logs.lock() {
                log.append(&buffer[..count]);
            }
        }
    });
}

impl BoundedLog {
    fn append(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        for byte in bytes {
            self.bytes.push_back(*byte);
        }
        while self.bytes.len() > MAX_LOG_BYTES {
            self.bytes.pop_front();
            self.truncated = true;
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

fn validate_command(command: &str) -> Result<(), String> {
    if command.is_empty() || command.len() > 16 * 1024 {
        Err("command must contain 1..=16384 bytes".to_string())
    } else {
        Ok(())
    }
}

fn command_summary(command: &str) -> String {
    let summary = command.split_whitespace().collect::<Vec<_>>().join(" ");
    summary.chars().take(MAX_COMMAND_SUMMARY_BYTES).collect()
}

fn command_hash(command: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in command.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

    #[test]
    fn background_task_ids_are_bounded_and_duplicate_start_is_idempotent() {
        let manager = BackgroundShellManager::new();
        manager.bind_owner("thread-1");
        assert!(
            manager
                .start(
                    "thread-1",
                    "bad id",
                    "sleep 1",
                    Path::new("."),
                    SandboxKind::Native
                )
                .is_err()
        );
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let first = manager
            .start(
                "thread-1",
                "dev-server",
                command,
                Path::new("."),
                SandboxKind::Native,
            )
            .unwrap();
        let second = manager
            .start(
                "thread-1",
                "dev-server",
                "another command",
                Path::new("."),
                SandboxKind::Native,
            )
            .unwrap();
        assert_eq!(first.task_id, second.task_id);
        assert_eq!(first.command_hash, second.command_hash);
        let stopped = manager.stop("thread-1", "dev-server").unwrap();
        assert_eq!(stopped.state, "stopped");
    }

    #[test]
    fn background_task_logs_are_bounded_and_readable() {
        let manager = BackgroundShellManager::new();
        manager.bind_owner("thread-1");
        let command = if cfg!(windows) {
            "Write-Output background-log"
        } else {
            "printf background-log"
        };
        manager
            .start(
                "thread-1",
                "log-task",
                command,
                Path::new("."),
                SandboxKind::Native,
            )
            .unwrap();
        for _ in 0..20 {
            let logs = manager.logs("thread-1", "log-task").unwrap();
            if logs.text.contains("background-log") {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("background task output was not captured");
    }

    #[test]
    fn disabled_runtime_cannot_create_background_tasks() {
        let manager = BackgroundShellManager::new();
        manager.bind_owner("child-thread");
        manager.disable();
        assert!(
            manager
                .start(
                    "child-thread",
                    "child-task",
                    "sleep 1",
                    Path::new("."),
                    SandboxKind::Native,
                )
                .is_err()
        );
    }

    #[test]
    fn closing_runtime_terminates_and_forgets_background_tasks() {
        let manager = BackgroundShellManager::new();
        manager.bind_owner("thread-1");
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        manager
            .start(
                "thread-1",
                "runtime-task",
                command,
                Path::new("."),
                SandboxKind::Native,
            )
            .unwrap();
        manager.close_all().unwrap();
        assert!(manager.list("thread-1").unwrap().is_empty());
    }
}
