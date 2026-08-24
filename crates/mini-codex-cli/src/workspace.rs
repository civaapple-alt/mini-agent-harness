use mini_codex_core::Tool;
use mini_codex_core::ToolError;
use mini_codex_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::io;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const MAX_READ_BYTES: u64 = 64 * 1024;
const MAX_WRITE_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    Interactive,
    Automatic,
}

#[derive(Clone)]
pub struct ApprovalController(Arc<AtomicBool>);

impl ApprovalController {
    pub fn new(mode: ApprovalMode) -> Self {
        Self(Arc::new(AtomicBool::new(matches!(
            mode,
            ApprovalMode::Automatic
        ))))
    }

    pub fn mode(&self) -> ApprovalMode {
        if self.0.load(Ordering::Relaxed) {
            ApprovalMode::Automatic
        } else {
            ApprovalMode::Interactive
        }
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        self.0
            .store(matches!(mode, ApprovalMode::Automatic), Ordering::Relaxed);
    }
}

pub fn workspace_tools(
    root: PathBuf,
    approval: ApprovalController,
) -> Result<Vec<Box<dyn Tool>>, ToolError> {
    let workspace = Arc::new(Workspace::new(root, approval)?);
    Ok(vec![
        Box::new(ReadFile(Arc::clone(&workspace))),
        Box::new(EditFile(Arc::clone(&workspace))),
        Box::new(WriteFile(Arc::clone(&workspace))),
        Box::new(Shell(workspace)),
    ])
}

struct Workspace {
    root: PathBuf,
    approval: ApprovalController,
}

impl Workspace {
    fn new(root: PathBuf, approval: ApprovalController) -> Result<Self, ToolError> {
        let root = root
            .canonicalize()
            .map_err(|error| ToolError(format!("invalid workspace: {error}")))?;
        Ok(Self { root, approval })
    }

    fn read_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        self.ensure_inside(resolved)
    }

    fn create_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        if candidate.exists() {
            return Err(ToolError(
                "file already exists; use edit_file for existing files".to_string(),
            ));
        }
        let parent = candidate
            .parent()
            .ok_or_else(|| ToolError("path has no parent".to_string()))?
            .canonicalize()
            .map_err(|error| ToolError(format!("parent directory must exist: {error}")))?;
        if !parent.starts_with(&self.root) {
            return Err(ToolError("path escapes the workspace".to_string()));
        }
        let file_name = candidate
            .file_name()
            .ok_or_else(|| ToolError("path has no file name".to_string()))?;
        Ok(parent.join(file_name))
    }

    fn candidate(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let raw = string_arg(value, "path")?;
        let relative = Path::new(raw);
        if relative.as_os_str().is_empty()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                ) || matches!(
                    component,
                    Component::Normal(name)
                        if name.to_string_lossy().eq_ignore_ascii_case(".git")
                )
            })
        {
            return Err(ToolError(
                "path must be relative, remain in the workspace, and avoid .git".to_string(),
            ));
        }
        Ok(self.root.join(relative))
    }

    fn ensure_inside(&self, path: PathBuf) -> Result<PathBuf, ToolError> {
        if path.starts_with(&self.root) && path != self.root {
            Ok(path)
        } else {
            Err(ToolError("path escapes the workspace".to_string()))
        }
    }

    fn approve(&self, action: &str) -> Result<(), ToolError> {
        match self.approval.mode() {
            ApprovalMode::Automatic => return Ok(()),
            ApprovalMode::Interactive => {}
        }
        if !io::stdin().is_terminal() {
            return Err(ToolError(format!(
                "denied non-interactive action: {action}"
            )));
        }
        eprint!("approve {action}? [y/N] ");
        io::stderr()
            .flush()
            .map_err(|error| ToolError(error.to_string()))?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| ToolError(error.to_string()))?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            Ok(())
        } else {
            Err(ToolError(format!("user denied: {action}")))
        }
    }
}

struct ReadFile(Arc<Workspace>);

impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        file_tool_spec("read_file", "Read a UTF-8 file in the workspace", false)
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.0.read_path(arguments)?;
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(io_error)?
            .take(MAX_READ_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > MAX_READ_BYTES {
            return Err(ToolError(format!(
                "file exceeds {MAX_READ_BYTES} byte read limit"
            )));
        }
        String::from_utf8(bytes).map_err(|_| ToolError("file is not UTF-8".to_string()))
    }
}

struct EditFile(Arc<Workspace>);

impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace one exact, unique text occurrence in a workspace file"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"}
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.0.read_path(arguments)?;
        let old_text = string_arg(arguments, "old_text")?;
        let new_text = string_arg(arguments, "new_text")?;
        if old_text.is_empty() {
            return Err(ToolError("old_text must not be empty".to_string()));
        }
        let content = fs::read_to_string(&path).map_err(io_error)?;
        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError("file exceeds edit limit".to_string()));
        }
        let matches = content.match_indices(old_text).count();
        if matches != 1 {
            return Err(ToolError(format!(
                "old_text must match exactly once; found {matches}"
            )));
        }
        let updated = content.replacen(old_text, new_text, 1);
        if updated.len() > MAX_WRITE_BYTES {
            return Err(ToolError("edited file exceeds write limit".to_string()));
        }
        self.0.approve(&format!("edit {}", path.display()))?;
        fs::write(&path, updated).map_err(io_error)?;
        Ok(format!("edited {}", path.display()))
    }
}

struct WriteFile(Arc<Workspace>);

impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        file_tool_spec(
            "write_file",
            "Create a new UTF-8 file in an existing workspace directory",
            true,
        )
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.0.create_path(arguments)?;
        let content = string_arg(arguments, "content")?;
        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError(format!(
                "content exceeds {MAX_WRITE_BYTES} byte write limit"
            )));
        }
        self.0.approve(&format!(
            "write {} ({} bytes)",
            path.display(),
            content.len()
        ))?;
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error)?;
        file.write_all(content.as_bytes()).map_err(io_error)?;
        Ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

struct Shell(Arc<Workspace>);

impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: shell_description(self.0.approval.mode()),
            parameters: json!({
                "type": "object",
                "properties": { "command": {"type": "string"} },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let command = string_arg(arguments, "command")?;
        if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
            return Err(ToolError(format!(
                "command must contain 1..={MAX_COMMAND_BYTES} bytes"
            )));
        }
        self.0.approve(&format!("shell command `{command}`"))?;
        run_shell(command, &self.0.root, COMMAND_TIMEOUT)
    }
}

fn shell_description(approval: ApprovalMode) -> String {
    let approval = match approval {
        ApprovalMode::Interactive => "after user approval",
        ApprovalMode::Automatic => "without per-command approval",
    };
    if cfg!(windows) {
        format!(
            "Run one PowerShell 7 command via pwsh in the Windows workspace {approval}, with a 120-second deadline. Use PowerShell syntax and cmdlets; do not use Unix-only commands or options such as `ls -la`, `find -maxdepth`, or `head`."
        )
    } else {
        format!("Run one POSIX sh command in the workspace {approval}, with a 120-second deadline")
    }
}

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut process = Command::new("pwsh");
        process.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            command,
        ]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        process
    }
}

fn terminate_process_tree(child: &mut std::process::Child) -> io::Result<ExitStatus> {
    let process_id = child.id().to_string();
    let killed = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &process_id, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    } else {
        Command::new("kill")
            .args(["-KILL", "--", &format!("-{process_id}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if !killed && child.try_wait()?.is_none() {
        child.kill()?;
    }
    child.wait()
}

fn run_shell(command: &str, root: &Path, timeout: Duration) -> Result<String, ToolError> {
    let mut child = shell_command(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError("cannot capture command stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError("cannot capture command stderr".to_string()))?;
    let stream_limit = MAX_COMMAND_OUTPUT_BYTES / 2;
    let stdout = thread::spawn(move || capture_bounded(stdout, stream_limit));
    let stderr = thread::spawn(move || capture_bounded(stderr, stream_limit));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            break (terminate_process_tree(&mut child).map_err(io_error)?, true);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout
        .join()
        .map_err(|_| ToolError("stdout reader panicked".to_string()))?
        .map_err(io_error)?;
    let stderr = stderr
        .join()
        .map_err(|_| ToolError("stderr reader panicked".to_string()))?
        .map_err(io_error)?;
    let status = if timed_out {
        format!("timed out after {} seconds", timeout.as_secs_f64())
    } else {
        status.code().map_or_else(
            || "terminated by signal".to_string(),
            |code| code.to_string(),
        )
    };
    Ok(format!(
        "exit: {status}\nstdout:\n{}\nstderr:\n{}",
        stdout.render(),
        stderr.render()
    ))
}

struct CapturedOutput {
    bytes: Vec<u8>,
    total_bytes: usize,
    truncated: bool,
}

impl CapturedOutput {
    fn render(&self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        if self.truncated {
            format!(
                "[retained head+tail from {} bytes]\n{text}",
                self.total_bytes
            )
        } else {
            text.into_owned()
        }
    }
}

fn capture_bounded(mut reader: impl Read, max_bytes: usize) -> io::Result<CapturedOutput> {
    let head_limit = max_bytes.div_ceil(2);
    let tail_limit = max_bytes - head_limit;
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = VecDeque::with_capacity(tail_limit);
    let mut total_bytes = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(count);
        let mut remaining = &buffer[..count];
        if head.len() < head_limit {
            let retained = remaining.len().min(head_limit - head.len());
            head.extend_from_slice(&remaining[..retained]);
            remaining = &remaining[retained..];
        }
        for byte in remaining {
            if tail_limit == 0 {
                break;
            }
            if tail.len() == tail_limit {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
    head.extend(tail);
    Ok(CapturedOutput {
        bytes: head,
        total_bytes,
        truncated: total_bytes > max_bytes,
    })
}

fn file_tool_spec(name: &str, description: &str, content: bool) -> ToolSpec {
    let mut properties = json!({"path": {"type": "string"}});
    let mut required = vec!["path"];
    if content {
        properties["content"] = json!({"type": "string"});
        required.push("content");
    }
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

fn string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError(format!("{name} must be a string")))
}

fn io_error(error: io::Error) -> ToolError {
    ToolError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    pub(super) fn test_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mini-codex-{nonce}"));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn reads_and_edits_inside_workspace() {
        let root = test_root();
        fs::write(root.join("note.txt"), "hello world").unwrap();
        let workspace = Arc::new(
            Workspace::new(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
            )
            .unwrap(),
        );
        let read = ReadFile(Arc::clone(&workspace));
        let edit = EditFile(workspace);

        assert_eq!(
            read.execute(&json!({"path": "note.txt"})).unwrap(),
            "hello world"
        );
        edit.execute(&json!({
            "path": "note.txt",
            "old_text": "world",
            "new_text": "agent"
        }))
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("note.txt")).unwrap(),
            "hello agent"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_escape_and_git_paths() {
        let root = test_root();
        let workspace = Workspace::new(
            root.clone(),
            ApprovalController::new(ApprovalMode::Automatic),
        )
        .unwrap();

        assert!(workspace.candidate(&json!({"path": "../secret"})).is_err());
        assert!(
            workspace
                .candidate(&json!({"path": ".git/config"}))
                .is_err()
        );
        assert!(
            workspace
                .candidate(&json!({"path": ".GIT/config"}))
                .is_err()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_file_creates_but_does_not_replace() {
        let root = test_root();
        fs::write(root.join("existing.txt"), "keep me").unwrap();
        let workspace = Arc::new(
            Workspace::new(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
            )
            .unwrap(),
        );
        let write = WriteFile(workspace);

        write
            .execute(&json!({"path": "new.txt", "content": "new file"}))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).unwrap(),
            "new file"
        );
        assert!(
            write
                .execute(&json!({"path": "existing.txt", "content": "replaced"}))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("existing.txt")).unwrap(),
            "keep me"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounded_capture_keeps_head_and_tail() {
        let captured = capture_bounded(Cursor::new(b"0123456789abcdef"), 8).unwrap();

        assert_eq!(captured.bytes, b"0123cdef");
        assert_eq!(captured.total_bytes, 16);
        assert!(captured.truncated);
    }

    #[test]
    fn shell_process_has_a_timeout() {
        let root = test_root();
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 5"
        } else {
            "sleep 5"
        };

        let output = run_shell(command, &root, Duration::from_millis(50)).unwrap();

        assert!(output.contains("timed out"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shell_matches_the_host_environment() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::new(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
            )
            .unwrap(),
        );
        let spec = Shell(workspace).spec();
        let command = shell_command("echo ready");

        if cfg!(windows) {
            assert_eq!(command.get_program(), "pwsh");
            assert!(spec.description.contains("PowerShell 7"));
            assert!(spec.description.contains("Windows"));
        } else {
            assert_eq!(command.get_program(), "sh");
            assert!(spec.description.contains("POSIX sh"));
        }
        assert!(spec.description.contains("without per-command approval"));

        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "workspace_edit_experiment.rs"]
mod edit_experiment;
