use crate::processes::ProcessManager;
mod approval;
mod files;
mod shell;

use crate::processes::process_tools;
use crate::result_store::ReadToolResult;
use crate::result_store::ResultStore;
use crate::sandbox::ProcessSandbox;
use crate::sandbox::SandboxKind;
use crate::security::ApprovalStore;
use crate::security::SecurityDecision;
use crate::security::SecurityPolicy;
use crate::security::SecurityPreset;
pub use approval::{ApprovalController, ApprovalMode};
#[cfg(test)]
use files::{EditFile, ReadFile, ReadImage, WriteFile};
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use serde_json::json;
pub use shell::{CommandOutput, run_sandboxed_command, shell_command};
#[cfg(test)]
use shell::{Shell, capture_bounded, run_shell};
#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
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
use std::sync::RwLock;
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

pub fn workspace_tools_with_read_roots_and_results(
    root: PathBuf,
    approval: ApprovalController,
    extra_read_roots: Vec<PathBuf>,
    sandbox: SandboxKind,
    images: crate::image::ImageStore,
    results: ResultStore,
) -> Result<Vec<Box<dyn Tool>>, ToolError> {
    let workspace = Arc::new(Workspace::with_read_roots(
        root,
        approval,
        extra_read_roots,
        sandbox,
    )?);
    let processes = ProcessManager::new(
        workspace.root.clone(),
        workspace.approval.clone(),
        results.clone(),
        sandbox,
    );
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(files::ReadFile(Arc::clone(&workspace))),
        Box::new(files::EditFile(Arc::clone(&workspace))),
        Box::new(files::WriteFile(Arc::clone(&workspace))),
        Box::new(shell::Shell(Arc::clone(&workspace), results.clone())),
        Box::new(ReadToolResult(results.clone())),
    ];
    tools.extend(crate::web::web_tools(results.clone()));
    tools.push(Box::new(files::ReadImage {
        workspace: Arc::clone(&workspace),
        store: images,
    }));
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
            && crate::path_policy::is_plan_md_alias(path)
        {
            return Ok(living);
        }
        if let Some(goal_dir) = self.approval.goal_dir()
            && let Some(rest) = crate::path_policy::goal_relative_rest(path)
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
            .is_some_and(|living| crate::path_policy::same_path(path, &living))
    }

    fn is_goal_artifact(&self, path: &Path) -> bool {
        self.approval
            .goal_dir()
            .is_some_and(|dir| crate::path_policy::is_under_dir(path, &dir))
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
