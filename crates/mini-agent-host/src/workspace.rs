use crate::processes::ProcessManager;
use crate::processes::process_tools;
use crate::result_store::ReadToolResult;
use crate::result_store::ResultStore;
use crate::sandbox::ProcessSandbox;
use crate::sandbox::SandboxKind;
use crate::security::ApprovalStore;
use crate::security::SecurityDecision;
use crate::security::SecurityPolicy;
use crate::security::SecurityPreset;
use mini_agent_core::Tool;
use mini_agent_core::ToolError;
use mini_agent_core::ToolSpec;
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
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const MAX_READ_BYTES: u64 = 128 * 1024;
const MAX_WRITE_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_COMMAND_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const INLINE_COMMAND_OUTPUT_BYTES: usize = 16 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    Interactive,
    Automatic,
}

type ApprovalCallback = dyn Fn(&str) -> Result<bool, ToolError> + Send + Sync;

#[derive(Clone)]
pub struct ApprovalController {
    automatic: Arc<AtomicBool>,
    policy: Arc<SecurityPolicy>,
    store: ApprovalStore,
    callback: Arc<ApprovalCallback>,
    living_plan: Arc<Mutex<Option<PathBuf>>>,
    goal_dir: Arc<Mutex<Option<PathBuf>>>,
    session_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl ApprovalController {
    #[allow(dead_code)]
    pub fn new(mode: ApprovalMode) -> Self {
        Self::with_policy_and_callback(
            mode,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            terminal_approval,
        )
    }

    pub fn with_preset(mode: ApprovalMode, preset: SecurityPreset) -> Self {
        Self::with_policy_and_callback(mode, SecurityPolicy::for_preset(preset), terminal_approval)
    }

    #[allow(dead_code)]
    pub fn with_callback(
        mode: ApprovalMode,
        callback: impl Fn(&str) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self::with_policy_and_callback(
            mode,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            callback,
        )
    }

    pub fn with_policy_and_callback(
        mode: ApprovalMode,
        policy: SecurityPolicy,
        callback: impl Fn(&str) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            automatic: Arc::new(AtomicBool::new(matches!(mode, ApprovalMode::Automatic))),
            policy: Arc::new(policy),
            store: ApprovalStore::new(),
            callback: Arc::new(callback),
            living_plan: Arc::new(Mutex::new(None)),
            goal_dir: Arc::new(Mutex::new(None)),
            session_dir: Arc::new(Mutex::new(None)),
        }
    }

    pub fn preset(&self) -> SecurityPreset {
        self.policy.preset
    }

    pub fn mode(&self) -> ApprovalMode {
        if self.automatic.load(Ordering::Relaxed) {
            ApprovalMode::Automatic
        } else {
            ApprovalMode::Interactive
        }
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        self.automatic
            .store(matches!(mode, ApprovalMode::Automatic), Ordering::Relaxed);
    }

    pub fn set_living_plan(&self, path: Option<PathBuf>) {
        *self.living_plan.lock().unwrap() = path.map(|path| crate::goal::normalize_path(&path));
    }

    pub fn living_plan(&self) -> Option<PathBuf> {
        self.living_plan.lock().unwrap().clone()
    }

    pub fn set_goal_dir(&self, path: Option<PathBuf>) {
        *self.goal_dir.lock().unwrap() = path.map(|path| crate::goal::normalize_path(&path));
    }

    pub fn goal_dir(&self) -> Option<PathBuf> {
        self.goal_dir.lock().unwrap().clone()
    }

    pub fn bind_session_file(&self, session_jsonl: &Path) {
        *self.session_dir.lock().unwrap() = session_jsonl.parent().map(crate::goal::normalize_path);
    }

    pub fn session_dir(&self) -> Option<PathBuf> {
        self.session_dir.lock().unwrap().clone()
    }

    pub fn ensure_plan_mode_unlocked(&self) -> Result<(), ToolError> {
        match self.living_plan() {
            Some(living) => Err(ToolError(format!(
                "workspace mutations locked in Plan Mode; living plan is {}",
                living.display()
            ))),
            None => Ok(()),
        }
    }

    #[allow(dead_code)]
    pub fn store(&self) -> &ApprovalStore {
        &self.store
    }

    pub fn approve(&self, action: &str) -> Result<(), ToolError> {
        match self.policy.evaluate(action) {
            SecurityDecision::Deny => {
                return Err(ToolError(format!("forbidden by security policy: {action}")));
            }
            SecurityDecision::Allow => return Ok(()),
            SecurityDecision::Ask => {}
        }
        if self.store.is_approved(action) {
            return Ok(());
        }
        match self.mode() {
            ApprovalMode::Automatic => return Ok(()),
            ApprovalMode::Interactive => {}
        }
        if (self.callback)(action)? {
            self.store.remember_approval(action);
            Ok(())
        } else {
            Err(ToolError(format!("user denied: {action}")))
        }
    }
}

fn terminal_approval(action: &str) -> Result<bool, ToolError> {
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
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub fn workspace_tools_with_read_roots(
    root: PathBuf,
    approval: ApprovalController,
    extra_read_roots: Vec<PathBuf>,
    sandbox: SandboxKind,
    images: crate::image::ImageStore,
) -> Result<Vec<Box<dyn Tool>>, ToolError> {
    let workspace = Arc::new(Workspace::with_read_roots(
        root,
        approval,
        extra_read_roots,
        sandbox,
    )?);
    let results = ResultStore::default();
    let processes = ProcessManager::new(
        workspace.root.clone(),
        workspace.approval.clone(),
        results.clone(),
        sandbox,
    );
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadFile(Arc::clone(&workspace))),
        Box::new(EditFile(Arc::clone(&workspace))),
        Box::new(WriteFile(Arc::clone(&workspace))),
        Box::new(Shell(Arc::clone(&workspace), results.clone())),
        Box::new(ReadToolResult(results.clone())),
    ];
    tools.extend(crate::web::web_tools(
        Arc::clone(&workspace),
        results.clone(),
    ));
    tools.push(Box::new(ReadImage {
        workspace: Arc::clone(&workspace),
        store: images,
    }));
    tools.extend(crate::subagent::subagent_tools(Arc::clone(&workspace)));
    tools.extend(process_tools(processes));
    Ok(tools)
}

pub struct Workspace {
    pub root: PathBuf,
    pub extra_read_roots: Vec<PathBuf>,
    pub approval: ApprovalController,
    pub sandbox: SandboxKind,
}

impl Workspace {
    pub fn with_read_roots(
        root: PathBuf,
        approval: ApprovalController,
        extra_read_roots: Vec<PathBuf>,
        sandbox: SandboxKind,
    ) -> Result<Self, ToolError> {
        let root = root
            .canonicalize()
            .map_err(|error| ToolError(format!("invalid workspace: {error}")))?;
        let extra_read_roots = extra_read_roots
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.is_dir() && !path.starts_with(&root))
            .collect();
        Ok(Self {
            root,
            extra_read_roots,
            approval,
            sandbox,
        })
    }

    pub fn read_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        if self.is_session_artifact(&resolved) {
            return Ok(resolved);
        }
        self.ensure_readable(resolved)
    }

    pub fn local_file_path(
        &self,
        value: &Value,
        outside_action: &str,
    ) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        if self.is_session_artifact(&resolved) {
            return Ok(resolved);
        }
        if self.ensure_readable(resolved.clone()).is_ok() {
            return Ok(resolved);
        }
        if has_git_component(&resolved) {
            return Err(ToolError("path escapes the workspace".to_string()));
        }
        if !resolved.is_file() {
            return Err(ToolError(format!(
                "cannot read \"{}\": not a regular file",
                resolved.display()
            )));
        }
        self.approve(&format!("{outside_action} {}", resolved.display()))?;
        Ok(resolved)
    }

    fn mutate_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        if self.is_session_artifact(&resolved) {
            return Ok(resolved);
        }
        self.ensure_plan_mode_unlocked()?;
        self.ensure_inside(resolved)
    }

    fn allows_outside_paths(&self) -> bool {
        self.approval.preset() == SecurityPreset::FullMachine
            || self.approval.preset() == SecurityPreset::Turbomode
    }

    fn create_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let session_artifact = self.is_session_artifact(&candidate);
        if candidate.exists() && !session_artifact {
            return Err(ToolError(
                "file already exists; use edit_file for existing files".to_string(),
            ));
        }
        if !session_artifact {
            self.ensure_plan_mode_unlocked()?;
        }
        let parent = candidate
            .parent()
            .ok_or_else(|| ToolError("path has no parent".to_string()))?
            .canonicalize()
            .map_err(|error| ToolError(format!("parent directory must exist: {error}")))?;
        if !session_artifact && !self.allows_outside_paths() && !parent.starts_with(&self.root) {
            return Err(ToolError("path escapes the workspace".to_string()));
        }
        let file_name = candidate
            .file_name()
            .ok_or_else(|| ToolError("path has no file name".to_string()))?;
        Ok(parent.join(file_name))
    }

    fn candidate(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let raw = string_arg(value, "path")?;
        let path = Path::new(raw);
        if path.as_os_str().is_empty() || has_git_component(path) {
            return Err(ToolError(
                "path must remain in the workspace or a configured extension root, and avoid .git"
                    .to_string(),
            ));
        }
        if let Some(living) = self.approval.living_plan()
            && crate::goal::is_plan_md_alias(path)
        {
            return Ok(living);
        }
        if let Some(goal_dir) = self.approval.goal_dir()
            && let Some(rest) = crate::goal::goal_relative_rest(path)
        {
            return Ok(goal_dir.join(rest));
        }
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(ToolError(
                "path must be relative, remain in the workspace, and avoid .git".to_string(),
            ));
        }
        Ok(self.root.join(path))
    }

    fn is_living_plan(&self, path: &Path) -> bool {
        self.approval
            .living_plan()
            .is_some_and(|living| crate::goal::same_path(path, &living))
    }

    fn is_goal_artifact(&self, path: &Path) -> bool {
        self.approval
            .goal_dir()
            .is_some_and(|dir| crate::goal::is_under_dir(path, &dir))
    }

    fn is_session_artifact(&self, path: &Path) -> bool {
        self.is_living_plan(path) || self.is_goal_artifact(path)
    }

    fn ensure_plan_mode_unlocked(&self) -> Result<(), ToolError> {
        self.approval.ensure_plan_mode_unlocked()
    }

    fn ensure_inside(&self, path: PathBuf) -> Result<PathBuf, ToolError> {
        if has_git_component(&path) {
            return Err(ToolError("path escapes the workspace".to_string()));
        }
        if self.allows_outside_paths() {
            return Ok(path);
        }
        if path.starts_with(&self.root) && path != self.root {
            Ok(path)
        } else {
            Err(ToolError("path escapes the workspace".to_string()))
        }
    }

    fn ensure_readable(&self, path: PathBuf) -> Result<PathBuf, ToolError> {
        if has_git_component(&path) {
            return Err(ToolError("path escapes the workspace".to_string()));
        }
        if self.allows_outside_paths() {
            return Ok(path);
        }
        if let Ok(path) = self.ensure_inside(path.clone()) {
            return Ok(path);
        }
        if self
            .extra_read_roots
            .iter()
            .any(|root| path.starts_with(root) && path != *root)
        {
            Ok(path)
        } else {
            Err(ToolError("path escapes the workspace".to_string()))
        }
    }

    pub fn approve(&self, action: &str) -> Result<(), ToolError> {
        self.approval.approve(action)
    }
}

struct ReadImage {
    workspace: Arc<Workspace>,
    store: crate::image::ImageStore,
}

impl Tool for ReadImage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_image".to_string(),
            description: "Read a local PNG/JPEG/GIF/WebP file and return it for vision models. Path may be workspace-relative or an absolute path on this machine (for example a file under Pictures). Do not copy outside images into the workspace. Absolute paths outside the workspace require approval. The host uploads once via the Files API and later turns reuse that file_id. This is not a screenshot tool.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = self.workspace.local_file_path(arguments, "read_image")?;
        let declared = crate::image::declared_media_type(&path).ok_or_else(|| {
            ToolError(format!(
                "cannot read \"{}\": read_image only accepts PNG/JPEG/WebP/GIF paths",
                path.display()
            ))
        })?;
        if !path.is_file() {
            return Err(ToolError(format!(
                "cannot read \"{}\": not a regular file",
                path.display()
            )));
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(io_error)?
            .take(crate::image::MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > crate::image::MAX_IMAGE_BYTES {
            return Err(ToolError(format!(
                "image exceeds {} byte read_image limit",
                crate::image::MAX_IMAGE_BYTES
            )));
        }
        let actual = crate::image::detect_image(&bytes).ok_or_else(|| {
            ToolError(format!(
                "cannot read \"{}\": the bytes are not a PNG/JPEG/WebP/GIF image",
                path.display()
            ))
        })?;
        if actual != declared {
            return Err(ToolError(format!(
                "cannot read \"{}\": the extension declares {declared}, but the bytes use {actual}; rename the file to match its actual format if it is PNG/JPEG/WebP/GIF",
                path.display()
            )));
        }
        let display = path
            .strip_prefix(&self.workspace.root)
            .unwrap_or(path.as_path());
        let stored = self
            .store
            .save(&display.display().to_string(), actual, bytes)?;
        Ok(crate::image::format_envelope(&stored))
    }
}

struct ReadFile(Arc<Workspace>);

impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        file_tool_spec(
            "read_file",
            "Read a UTF-8 file in the workspace or a configured local extension root",
            false,
        )
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
        if crate::image::detect_image(&bytes).is_some() {
            return Err(ToolError(
                "file is not UTF-8; use read_image for PNG/JPEG/GIF/WebP".to_string(),
            ));
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
        let path = self.0.mutate_path(arguments)?;
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
        if self.0.is_session_artifact(&path) {
            fs::write(&path, content.as_bytes()).map_err(io_error)?;
        } else {
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(io_error)?;
            file.write_all(content.as_bytes()).map_err(io_error)?;
        }
        Ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

struct Shell(Arc<Workspace>, ResultStore);

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
        self.0.approval.ensure_plan_mode_unlocked()?;
        self.0.approve(&format!("shell command `{command}`"))?;
        let output = run_shell(command, &self.0.root, self.0.sandbox, COMMAND_TIMEOUT)?;
        if output.text.len() <= INLINE_COMMAND_OUTPUT_BYTES {
            return Ok(output.text);
        }
        let stored = self
            .1
            .store(output.text, output.source_bytes, output.source_truncated);
        Ok(format!(
            "<tool_result_preview handle=\"{}\" stored_bytes=\"{}\" source_bytes=\"{}\" source_truncated=\"{}\">\n{}\n</tool_result_preview>\nUse read_tool_result with this handle to inspect a byte range or literal query.",
            stored.handle,
            stored.stored_bytes,
            stored.source_bytes,
            stored.source_truncated,
            stored.preview
        ))
    }
}

fn shell_description(approval: ApprovalMode) -> String {
    let approval = match approval {
        ApprovalMode::Interactive => "after user approval",
        ApprovalMode::Automatic => "without per-command approval",
    };
    if cfg!(windows) {
        format!(
            "Run one PowerShell 7 command via pwsh in the Windows workspace {approval}, with a 120-second deadline. Use PowerShell syntax and cmdlets; do not use Unix-only commands or options such as `ls -la`, `find -maxdepth`, or `head`. For long-running or interactive programs, use process_start and process_write instead."
        )
    } else {
        format!(
            "Run one POSIX sh command in the workspace {approval}, with a 120-second deadline. For long-running or interactive programs, use process_start and process_write instead"
        )
    }
}

fn apply_utf8_child_env(cmd: &mut Command) {
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUTF8", "1");
    cmd.env("PYTHONLEGACYWINDOWSSTDIO", "0");
}

#[cfg(windows)]
fn windows_utf8_shell_script(command: &str) -> String {
    let preamble = concat!(
        "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
        "[Console]::OutputEncoding = $OutputEncoding; ",
        "[Console]::InputEncoding = $OutputEncoding; ",
        "$PSDefaultParameterValues['Get-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Set-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Add-Content:Encoding'] = 'utf8'; ",
        "$PSDefaultParameterValues['Out-File:Encoding'] = 'utf8'; ",
        "$env:PYTHONIOENCODING = 'utf-8'; ",
        "$env:PYTHONUTF8 = '1'; ",
    );
    format!("{preamble}{command}")
}

pub fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let wrapped = windows_utf8_shell_script(command);
        let mut process = Command::new("pwsh");
        apply_utf8_child_env(&mut process);
        process.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
        process.arg(wrapped);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        apply_utf8_child_env(&mut process);
        process.args(["-lc", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process.process_group(0);
        }
        process
    }
}

pub struct CommandOutput {
    pub text: String,
    pub raw_stdout: String,
    pub raw_stderr: String,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub source_bytes: usize,
    pub source_truncated: bool,
}

pub fn run_sandboxed_command(
    mut cmd: Command,
    root: &Path,
    sandbox_kind: SandboxKind,
    timeout: Duration,
) -> Result<CommandOutput, ToolError> {
    let sandbox = ProcessSandbox::new(sandbox_kind);
    apply_utf8_child_env(&mut cmd);
    let mut child = cmd
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    sandbox.attach_child(&child);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError("cannot capture command stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError("cannot capture command stderr".to_string()))?;
    let stream_limit = MAX_COMMAND_CAPTURE_BYTES / 2;
    let stdout = thread::spawn(move || capture_bounded(stdout, stream_limit));
    let stderr = thread::spawn(move || capture_bounded(stderr, stream_limit));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().map_err(io_error)? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            break (sandbox.terminate(&mut child).map_err(io_error)?, true);
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
    let status_str = if timed_out {
        format!("timed out after {} seconds", timeout.as_secs_f64())
    } else {
        status.code().map_or_else(
            || "terminated by signal".to_string(),
            |code| code.to_string(),
        )
    };
    let source_bytes = stdout.total_bytes.saturating_add(stderr.total_bytes);
    let source_truncated = stdout.truncated || stderr.truncated;
    let raw_stdout = stdout.render();
    let raw_stderr = stderr.render();
    Ok(CommandOutput {
        text: format!("exit: {status_str}\nstdout:\n{raw_stdout}\nstderr:\n{raw_stderr}"),
        raw_stdout,
        raw_stderr,
        timed_out,
        exit_code: if timed_out { None } else { status.code() },
        source_bytes,
        source_truncated,
    })
}

fn run_shell(
    command: &str,
    root: &Path,
    sandbox_kind: SandboxKind,
    timeout: Duration,
) -> Result<CommandOutput, ToolError> {
    if sandbox_kind == SandboxKind::Docker {
        let docker_check = Command::new("docker")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if docker_check.is_err() || !docker_check.unwrap().success() {
            return Err(ToolError(
                "docker sandbox is unavailable on this host; ensure docker daemon is running, or use '--sandbox native'"
                    .to_string(),
            ));
        }
    }
    let cmd = if sandbox_kind == SandboxKind::Docker {
        let mut docker_cmd = Command::new("docker");
        docker_cmd.args([
            "run",
            "--rm",
            "-i",
            "-v",
            &format!("{}:/workspace", root.display()),
            "-w",
            "/workspace",
            "alpine",
            "sh",
            "-c",
            command,
        ]);
        docker_cmd
    } else {
        shell_command(command)
    };
    run_sandboxed_command(cmd, root, sandbox_kind, timeout)
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

fn has_git_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name) if name.to_string_lossy().eq_ignore_ascii_case(".git")
        )
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

pub fn string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
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
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    pub(super) fn test_root() -> PathBuf {
        static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("mini-agent-{nonce}-{sequence}"));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn reads_and_edits_inside_workspace() {
        let root = test_root();
        fs::write(root.join("note.txt"), "hello world").unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let read = ReadFile(Arc::clone(&workspace));
        let edit = EditFile(workspace);

        assert_eq!(
            read.execute(&json!({"path": "note.txt"})).unwrap(),
            "hello world"
        );
        let abs_path = root.join("note.txt").to_string_lossy().to_string();
        assert_eq!(
            read.execute(&json!({"path": abs_path})).unwrap(),
            "hello world"
        );
        edit.execute(&json!({
            "path": abs_path,
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
    fn read_image_uploads_and_rejects_type_mismatch() {
        struct StubFiles;
        impl crate::image::FileUploader for StubFiles {
            fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
                Ok("file-api-test".to_string())
            }
        }

        let root = test_root();
        fs::write(root.join("shot.png"), crate::image::TINY_PNG).unwrap();
        fs::write(root.join("shot.jpg"), crate::image::TINY_PNG).unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let ok = ReadImage {
            workspace: Arc::clone(&workspace),
            store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles)),
        };
        let out = ok.execute(&json!({"path": "shot.png"})).unwrap();
        assert!(out.contains("file_id=\"file-api-test\""));
        let mismatch = ReadImage {
            workspace,
            store: crate::image::ImageStore::memory_only(),
        };
        let error = mismatch.execute(&json!({"path": "shot.jpg"})).unwrap_err();
        assert!(error.0.contains("extension declares"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_image_accepts_absolute_path_outside_workspace_after_approval() {
        struct StubFiles;
        impl crate::image::FileUploader for StubFiles {
            fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<String, ToolError> {
                Ok("file-api-outside".to_string())
            }
        }

        let root = test_root();
        let pictures = test_root();
        fs::write(pictures.join("outside.png"), crate::image::TINY_PNG).unwrap();
        let abs = pictures.join("outside.png").canonicalize().unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = ReadImage {
            workspace: Arc::clone(&workspace),
            store: crate::image::ImageStore::with_uploader(Arc::new(StubFiles)),
        };
        let out = tool
            .execute(&json!({"path": abs.to_string_lossy().to_string()}))
            .unwrap();
        assert!(out.contains("file_id=\"file-api-outside\""), "{out}");
        assert!(
            ReadFile(workspace)
                .execute(&json!({"path": abs.to_string_lossy().to_string()}))
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(pictures).unwrap();
    }

    #[test]
    fn read_image_outside_workspace_can_be_denied() {
        let root = test_root();
        let pictures = test_root();
        fs::write(pictures.join("secret.png"), crate::image::TINY_PNG).unwrap();
        let abs = pictures.join("secret.png").canonicalize().unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_callback(ApprovalMode::Interactive, |_| Ok(false)),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let tool = ReadImage {
            workspace,
            store: crate::image::ImageStore::memory_only(),
        };
        let error = tool
            .execute(&json!({"path": abs.to_string_lossy().to_string()}))
            .unwrap_err();
        assert!(error.0.contains("denied"), "{error:?}");
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(pictures).unwrap();
    }

    #[test]
    fn rejects_escape_and_git_paths() {
        let root = test_root();
        let other = test_root();
        fs::write(other.join("secret.txt"), "secret data").unwrap();
        let outside_abs = other.join("secret.txt").to_string_lossy().to_string();

        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );

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

        let read = ReadFile(Arc::clone(&workspace));
        let err = read.execute(&json!({"path": outside_abs})).unwrap_err();
        assert!(err.0.contains("escapes the workspace"));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn read_file_accepts_configured_extension_roots() {
        let root = test_root();
        let extra = test_root();
        fs::write(extra.join("SKILL.md"), "extension body").unwrap();
        let extra_root = extra.canonicalize().unwrap();
        let skill = extra.join("SKILL.md").canonicalize().unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                vec![extra_root],
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let location = skill.to_string_lossy().replace('\\', "/");
        let read = ReadFile(Arc::clone(&workspace));
        let edit = EditFile(Arc::clone(&workspace));

        assert_eq!(
            read.execute(&json!({"path": location})).unwrap(),
            "extension body"
        );
        assert!(
            edit.execute(&json!({
                "path": location,
                "old_text": "extension",
                "new_text": "changed"
            }))
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(extra.join("SKILL.md")).unwrap(),
            "extension body"
        );

        fs::remove_dir_all(extra).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plan_mode_aliases_plan_md_and_locks_workspace_writes() {
        let root = test_root();
        let session = test_root();
        fs::write(root.join("note.txt"), "workspace note").unwrap();
        let plan = crate::goal::init_plan_mode_with_prompt(&session, None).unwrap();
        let approval = ApprovalController::new(ApprovalMode::Automatic);
        approval.set_living_plan(Some(plan.clone()));
        let workspace = Arc::new(
            Workspace::with_read_roots(root.clone(), approval, Vec::new(), SandboxKind::Native)
                .unwrap(),
        );
        let read = ReadFile(Arc::clone(&workspace));
        let edit = EditFile(Arc::clone(&workspace));
        let write = WriteFile(Arc::clone(&workspace));

        let locked = write
            .execute(&json!({"path": "src.rs", "content": "fn main() {}"}))
            .unwrap_err();
        assert!(
            locked.0.contains("workspace mutations locked in Plan Mode"),
            "{locked:?}"
        );
        let locked_edit = edit
            .execute(&json!({
                "path": "note.txt",
                "old_text": "workspace",
                "new_text": "changed"
            }))
            .unwrap_err();
        assert!(
            locked_edit
                .0
                .contains("workspace mutations locked in Plan Mode")
        );

        write
            .execute(&json!({
                "path": "plan.md",
                "content": "# Implementation Plan\n\n- Goals:\n  - implement auth\n"
            }))
            .unwrap();
        let living = fs::read_to_string(&plan).unwrap();
        assert!(living.contains("- implement auth"));
        assert!(!root.join("plan.md").exists());
        assert_eq!(read.execute(&json!({"path": "plan.md"})).unwrap(), living);

        edit.execute(&json!({
            "path": "plan.md",
            "old_text": "- implement auth",
            "new_text": "- implement auth\n  - add restore"
        }))
        .unwrap();
        assert!(fs::read_to_string(&plan).unwrap().contains("- add restore"));

        let shell = Shell(Arc::clone(&workspace), ResultStore::default());
        let locked_shell = shell
            .execute(&json!({"command": "printf should-not-run"}))
            .unwrap_err();
        assert!(
            locked_shell
                .0
                .contains("workspace mutations locked in Plan Mode")
        );

        fs::remove_dir_all(session).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn goal_mode_allows_session_goal_plan_reads_and_workspace_writes() {
        let root = test_root();
        let session = test_root();
        let state = crate::goal::init_goal_workspace(&session, "Ship HTML intro", 5).unwrap();
        assert_eq!(state.current_milestone, 1);
        let goal_dir = session.join("goal");
        let approval = ApprovalController::new(ApprovalMode::Automatic);
        approval.set_goal_dir(Some(goal_dir.clone()));
        let workspace = Arc::new(
            Workspace::with_read_roots(root.clone(), approval, Vec::new(), SandboxKind::Native)
                .unwrap(),
        );
        let read = ReadFile(Arc::clone(&workspace));
        let write = WriteFile(Arc::clone(&workspace));

        let plan = read.execute(&json!({"path": "goal/plan.md"})).unwrap();
        assert!(plan.contains("Autonomous Goal Plan: Ship HTML intro"));
        let abs = goal_dir.join("plan.md").to_string_lossy().to_string();
        assert!(
            read.execute(&json!({"path": abs}))
                .unwrap()
                .contains("Milestone 1")
        );

        write
            .execute(&json!({
                "path": "goal/plan.md",
                "content": "# Autonomous Goal Plan\n- [x] Milestone 1\n"
            }))
            .unwrap();
        assert!(
            fs::read_to_string(goal_dir.join("plan.md"))
                .unwrap()
                .contains("Milestone 1")
        );
        assert!(!root.join("goal").exists());

        write
            .execute(&json!({"path": "intro.html", "content": "<html></html>"}))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("intro.html")).unwrap(),
            "<html></html>"
        );

        fs::remove_dir_all(session).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn write_file_creates_but_does_not_replace() {
        let root = test_root();
        fs::write(root.join("existing.txt"), "keep me").unwrap();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
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

        let output = run_shell(
            command,
            &root,
            SandboxKind::Native,
            Duration::from_millis(50),
        )
        .unwrap();

        assert!(output.text.contains("timed out"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shell_matches_the_host_environment() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let spec = Shell(workspace, ResultStore::default()).spec();
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

    #[test]
    fn shell_preserves_utf8_from_workspace_files() {
        let root = test_root();
        fs::write(
            root.join("note.html"),
            "/* 数据统计卡片 */\n<p class=\"tagline\">小巧强悍，性能出众</p>\n",
        )
        .unwrap();
        let command = if cfg!(windows) {
            "$lines = Get-Content note.html; $lines[0..20]"
        } else {
            "cat note.html"
        };
        let output = run_shell(command, &root, SandboxKind::Native, COMMAND_TIMEOUT).unwrap();
        assert!(
            output.raw_stdout.contains("小巧强悍，性能出众"),
            "stdout was {:?}",
            output.raw_stdout
        );
        assert!(
            output.raw_stdout.contains("数据统计卡片"),
            "stdout was {:?}",
            output.raw_stdout
        );
        let python = if cfg!(windows) { "python" } else { "python3" };
        let py = run_shell(
            &format!(
                "{python} -c \"from pathlib import Path; print(Path('note.html').read_text(encoding='utf-8'))\""
            ),
            &root,
            SandboxKind::Native,
            COMMAND_TIMEOUT,
        );
        if let Ok(py) = py
            && py.exit_code == Some(0)
        {
            assert!(
                py.raw_stdout.contains("小巧强悍，性能出众"),
                "python stdout was {:?}",
                py.raw_stdout
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_shell_output_is_available_through_a_result_handle() {
        let root = test_root();
        let workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::new(ApprovalMode::Automatic),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let results = ResultStore::default();
        let shell = Shell(workspace, results.clone());
        let command = if cfg!(windows) {
            "Write-Output ('x' * 20000)"
        } else {
            "printf '%020000d' 0"
        };

        let output = shell.execute(&json!({"command": command})).unwrap();
        assert!(output.contains("handle=\"result-1\""), "{output}");
        let read = ReadToolResult(results)
            .execute(&json!({"handle": "result-1", "start_byte": 1, "byte_count": 128}))
            .unwrap();
        assert!(read.contains("stored_bytes="));
        assert!(read.len() >= 128);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn docker_sandbox_checks_availability_or_reports_clear_error() {
        let root = test_root();
        let result = run_shell(
            "echo hello",
            &root,
            SandboxKind::Docker,
            Duration::from_secs(5),
        );
        if let Err(err) = result {
            assert!(err.0.contains("docker sandbox is unavailable"));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_machine_preset_permits_paths_outside_workspace() {
        let root = test_root();
        let outside = test_root();
        fs::write(outside.join("outside.txt"), "outside data").unwrap();
        let outside_file = outside.join("outside.txt").to_string_lossy().to_string();

        let default_workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(ApprovalMode::Automatic, SecurityPreset::Default),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let default_read = ReadFile(default_workspace);
        assert!(
            default_read
                .execute(&json!({"path": &outside_file}))
                .is_err()
        );

        let full_workspace = Arc::new(
            Workspace::with_read_roots(
                root.clone(),
                ApprovalController::with_preset(
                    ApprovalMode::Automatic,
                    SecurityPreset::FullMachine,
                ),
                Vec::new(),
                SandboxKind::Native,
            )
            .unwrap(),
        );
        let full_read = ReadFile(full_workspace);
        assert_eq!(
            full_read.execute(&json!({"path": &outside_file})).unwrap(),
            "outside data"
        );

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}

#[cfg(test)]
#[path = "workspace_edit_experiment.rs"]
mod edit_experiment;
