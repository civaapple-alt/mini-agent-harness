use crate::result_store::ResultStore;
use crate::sandbox::ProcessSandbox;
use crate::sandbox::SandboxKind;
use crate::workspace::ApprovalController;
use crate::workspace::shell_command;
use crate::workspace::terminate_process_tree;
use mini_agent_core::Tool;
use mini_agent_core::ToolError;
use mini_agent_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::process::Child;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

const MAX_PROCESSES: usize = 8;
const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_LOG_BYTES_PER_STREAM: usize = 256 * 1024;
const INLINE_LOG_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub struct ProcessManager(Arc<ProcessManagerInner>);

struct ProcessManagerInner {
    root: PathBuf,
    approval: ApprovalController,
    results: ResultStore,
    sandbox_kind: SandboxKind,
    state: Mutex<ProcessState>,
}

#[derive(Default)]
struct ProcessState {
    next_id: u64,
    jobs: VecDeque<ProcessJob>,
}

struct ProcessJob {
    id: u64,
    command: String,
    child: Child,
    stdout: SharedLog,
    stderr: SharedLog,
    exit: Option<String>,
}

type SharedLog = Arc<Mutex<BoundedLog>>;

struct BoundedLog {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

impl BoundedLog {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(MAX_LOG_BYTES_PER_STREAM / 2),
            tail: VecDeque::with_capacity(MAX_LOG_BYTES_PER_STREAM / 2),
            total_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let head_limit = MAX_LOG_BYTES_PER_STREAM / 2;
        let tail_limit = MAX_LOG_BYTES_PER_STREAM - head_limit;
        let retained = bytes.len().min(head_limit.saturating_sub(self.head.len()));
        self.head.extend_from_slice(&bytes[..retained]);
        for byte in &bytes[retained..] {
            if self.tail.len() == tail_limit {
                self.tail.pop_front();
            }
            self.tail.push_back(*byte);
        }
    }

    fn snapshot(&self) -> (String, usize, bool) {
        let text = String::from_utf8_lossy(&self.head).to_string()
            + &String::from_utf8_lossy(&self.tail.iter().copied().collect::<Vec<_>>());
        let truncated = self.total_bytes > MAX_LOG_BYTES_PER_STREAM;
        (text, self.total_bytes, truncated)
    }
}

impl ProcessManager {
    pub fn new(
        root: PathBuf,
        approval: ApprovalController,
        results: ResultStore,
        sandbox_kind: SandboxKind,
    ) -> Self {
        Self(Arc::new(ProcessManagerInner {
            root,
            approval,
            results,
            sandbox_kind,
            state: Mutex::new(ProcessState::default()),
        }))
    }

    fn start(&self, command: &str) -> Result<String, ToolError> {
        if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
            return Err(ToolError(format!(
                "command must contain 1..={MAX_COMMAND_BYTES} bytes"
            )));
        }
        self.0
            .approval
            .approve(&format!("start managed process `{command}`"))?;

        let mut state = self.0.state.lock().unwrap();
        refresh_jobs(&mut state.jobs)?;
        while state.jobs.len() >= MAX_PROCESSES {
            let Some(index) = state.jobs.iter().position(|job| job.exit.is_some()) else {
                return Err(ToolError(format!(
                    "managed process limit reached: {MAX_PROCESSES}"
                )));
            };
            state.jobs.remove(index);
        }

        let mut child = shell_command(command)
            .current_dir(&self.0.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(io_error)?;
        let sandbox = ProcessSandbox::new(self.0.sandbox_kind);
        sandbox.attach_child(&child);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError("cannot capture process stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError("cannot capture process stderr".to_string()))?;
        let stdout_log = Arc::new(Mutex::new(BoundedLog::new()));
        let stderr_log = Arc::new(Mutex::new(BoundedLog::new()));
        spawn_log_reader(stdout, Arc::clone(&stdout_log));
        spawn_log_reader(stderr, Arc::clone(&stderr_log));

        state.next_id = state.next_id.saturating_add(1);
        let id = state.next_id;
        state.jobs.push_back(ProcessJob {
            id,
            command: command.to_string(),
            child,
            stdout: stdout_log,
            stderr: stderr_log,
            exit: None,
        });
        Ok(format!(
            "process_id={id}\nstatus=running\ncommand={command}"
        ))
    }

    fn read(&self, id: u64) -> Result<String, ToolError> {
        let mut state = self.0.state.lock().unwrap();
        refresh_jobs(&mut state.jobs)?;
        let job = state
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| ToolError(format!("unknown process_id: {id}")))?;
        let (stdout, stdout_bytes, stdout_truncated) = job.stdout.lock().unwrap().snapshot();
        let (stderr, stderr_bytes, stderr_truncated) = job.stderr.lock().unwrap().snapshot();
        let output = format!(
            "process_id={}\nstatus={}\ncommand={}\nstdout_bytes={}\nstderr_bytes={}\nstdout_truncated={}\nstderr_truncated={}\nstdout:\n{}\nstderr:\n{}",
            job.id,
            job.exit.as_deref().unwrap_or("running"),
            job.command,
            stdout_bytes,
            stderr_bytes,
            stdout_truncated,
            stderr_truncated,
            stdout,
            stderr
        );
        if output.len() <= INLINE_LOG_BYTES {
            return Ok(output);
        }
        let source_bytes = stdout_bytes.saturating_add(stderr_bytes);
        let stored =
            self.0
                .results
                .store(output, source_bytes, stdout_truncated || stderr_truncated);
        Ok(format!(
            "process_id={id}\nstatus={}\n<tool_result_preview handle=\"{}\" stored_bytes=\"{}\" source_bytes=\"{}\" source_truncated=\"{}\">\n{}\n</tool_result_preview>\nUse read_tool_result to inspect more output.",
            job.exit.as_deref().unwrap_or("running"),
            stored.handle,
            stored.stored_bytes,
            stored.source_bytes,
            stored.source_truncated,
            stored.preview
        ))
    }

    fn stop(&self, id: u64) -> Result<String, ToolError> {
        let mut state = self.0.state.lock().unwrap();
        refresh_jobs(&mut state.jobs)?;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| ToolError(format!("unknown process_id: {id}")))?;
        if let Some(exit) = &job.exit {
            return Ok(format!("process_id={id}\nstatus={exit}"));
        }
        let status = terminate_process_tree(&mut job.child).map_err(io_error)?;
        let exit = status_text(status.code(), "stopped");
        job.exit = Some(exit.clone());
        Ok(format!("process_id={id}\nstatus={exit}"))
    }

    fn list(&self) -> Result<String, ToolError> {
        let mut state = self.0.state.lock().unwrap();
        refresh_jobs(&mut state.jobs)?;
        if state.jobs.is_empty() {
            return Ok("no managed processes".to_string());
        }
        Ok(state
            .jobs
            .iter()
            .map(|job| {
                format!(
                    "process_id={} status={} command={}",
                    job.id,
                    job.exit.as_deref().unwrap_or("running"),
                    job.command
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

impl Drop for ProcessManagerInner {
    fn drop(&mut self) {
        let Ok(state) = self.state.get_mut() else {
            return;
        };
        for job in &mut state.jobs {
            if job.exit.is_none() {
                let _ = terminate_process_tree(&mut job.child);
            }
        }
    }
}

pub fn process_tools(manager: ProcessManager) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ProcessStart(manager.clone())),
        Box::new(ProcessRead(manager.clone())),
        Box::new(ProcessStop(manager.clone())),
        Box::new(ProcessList(manager)),
    ]
}

struct ProcessStart(ProcessManager);
struct ProcessRead(ProcessManager);
struct ProcessStop(ProcessManager);
struct ProcessList(ProcessManager);

impl Tool for ProcessStart {
    fn spec(&self) -> ToolSpec {
        process_spec(
            "process_start",
            "Start a managed long-running shell process and return immediately",
            true,
        )
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        self.0.start(string_arg(arguments, "command")?)
    }
}

impl Tool for ProcessRead {
    fn spec(&self) -> ToolSpec {
        process_spec(
            "process_read",
            "Read status and bounded logs for a managed process",
            false,
        )
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        self.0.read(process_id(arguments)?)
    }
}

impl Tool for ProcessStop {
    fn spec(&self) -> ToolSpec {
        process_spec(
            "process_stop",
            "Stop one managed process and its process tree",
            false,
        )
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        self.0.stop(process_id(arguments)?)
    }
}

impl Tool for ProcessList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "process_list".to_string(),
            description: "List managed processes in this mini-agent session".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        self.0.list()
    }
}

fn process_spec(name: &str, description: &str, command: bool) -> ToolSpec {
    let (properties, required) = if command {
        (json!({"command": {"type": "string"}}), json!(["command"]))
    } else {
        (
            json!({"process_id": {"type": "integer", "minimum": 1}}),
            json!(["process_id"]),
        )
    };
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }),
    }
}

fn refresh_jobs(jobs: &mut VecDeque<ProcessJob>) -> Result<(), ToolError> {
    for job in jobs {
        if job.exit.is_none()
            && let Some(status) = job.child.try_wait().map_err(io_error)?
        {
            job.exit = Some(status_text(status.code(), "exited"));
        }
    }
    Ok(())
}

fn spawn_log_reader(mut reader: impl Read + Send + 'static, log: SharedLog) {
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            log.lock().unwrap().push(&buffer[..count]);
        }
    });
}

fn status_text(code: Option<i32>, fallback: &str) -> String {
    code.map_or_else(|| fallback.to_string(), |code| format!("exited({code})"))
}

fn string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError(format!("{name} must be a string")))
}

fn process_id(arguments: &Value) -> Result<u64, ToolError> {
    arguments
        .get("process_id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| ToolError("process_id must be a positive integer".to_string()))
}

fn io_error(error: std::io::Error) -> ToolError {
    ToolError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::ApprovalMode;
    use std::fs;
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    #[test]
    fn managed_process_returns_immediately_and_exposes_output() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mini-agent-process-{nonce}"));
        fs::create_dir(&root).unwrap();
        let manager = ProcessManager::new(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
            ResultStore::default(),
            SandboxKind::Native,
        );
        let command = if cfg!(windows) {
            "Write-Output managed-ready"
        } else {
            "printf managed-ready"
        };

        let started = manager.start(command).unwrap();
        assert!(started.contains("status=running"));
        let mut output = String::new();
        for _ in 0..100 {
            output = manager.read(1).unwrap();
            if output.contains("managed-ready") && output.contains("exited(0)") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(output.contains("managed-ready"), "{output}");
        assert!(output.contains("exited(0)"), "{output}");

        drop(manager);
        fs::remove_dir_all(root).unwrap();
    }
}
