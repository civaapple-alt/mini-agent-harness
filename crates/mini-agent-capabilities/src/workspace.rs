mod approval;
mod files;
mod patch;
mod shell;

use crate::result_store::ResultStore;
use crate::sandbox::ProcessSandbox;
use crate::sandbox::SandboxKind;
pub use crate::security::ApprovalScope;
use crate::security::SecurityDecision;
use crate::security::SecurityPolicy;
use crate::security::SecurityPreset;
pub use approval::{ApprovalController, ApprovalMode};
#[cfg(test)]
use files::{ReadFile, ReadImage};
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolAdmission;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
#[cfg(test)]
use patch::ApplyPatch;
use serde_json::Value;
use serde_json::json;
#[cfg(test)]
use shell::{Shell, is_read_only_shell_command, run_shell};
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

const MAX_READ_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 2_000;
const MAX_READ_PAGE_BYTES: usize = 15 * 1024;
const MAX_WRITE_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 16 * 1024;
const MAX_COMMAND_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const INLINE_COMMAND_OUTPUT_BYTES: usize = 16 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

pub fn workspace_tools_with_read_roots_and_results(
    root: PathBuf,
    approval: ApprovalController,
    extra_read_roots: Vec<PathBuf>,
    extra_write_roots: Vec<PathBuf>,
    sandbox: SandboxKind,
    images: crate::image::ImageStore,
    results: ResultStore,
) -> Result<Vec<Box<dyn Tool>>, ToolError> {
    let workspace = Arc::new(Workspace::with_read_roots(
        root,
        approval,
        extra_read_roots,
        extra_write_roots,
        sandbox,
    )?);
    let mut tools: Vec<Box<dyn Tool>> = vec![
        Box::new(files::ReadFile(Arc::clone(&workspace))),
        Box::new(patch::ApplyPatch(Arc::clone(&workspace))),
        Box::new(shell::Shell(Arc::clone(&workspace), results.clone())),
    ];
    tools.extend(crate::web::web_tools(results.clone()));
    tools.push(Box::new(files::ReadImage {
        workspace: Arc::clone(&workspace),
        store: images,
    }));
    Ok(tools)
}

struct Workspace {
    root: PathBuf,
    extra_read_roots: Vec<PathBuf>,
    extra_write_roots: Vec<PathBuf>,
    approval: ApprovalController,
    sandbox: SandboxKind,
}

impl Workspace {
    fn with_read_roots(
        root: PathBuf,
        approval: ApprovalController,
        extra_read_roots: Vec<PathBuf>,
        extra_write_roots: Vec<PathBuf>,
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
        let extra_write_roots = extra_write_roots
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.is_dir() && !path.starts_with(&root))
            .collect();
        Ok(Self {
            root,
            extra_read_roots,
            extra_write_roots,
            approval,
            sandbox,
        })
    }

    fn read_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        if self.is_session_artifact(&resolved) {
            return Ok(resolved);
        }
        self.ensure_readable(resolved)
    }

    fn local_file_path(&self, value: &Value, outside_action: &str) -> Result<PathBuf, ToolError> {
        let (resolved, requires_approval) = self.local_file_path_with_admission(value)?;
        if requires_approval {
            self.approve(&format!("{outside_action} {}", resolved.display()))?;
        }
        Ok(resolved)
    }

    fn local_file_path_with_admission(&self, value: &Value) -> Result<(PathBuf, bool), ToolError> {
        let candidate = self.candidate(value)?;
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))?;
        if self.is_session_artifact(&resolved) {
            return Ok((resolved, false));
        }
        if self.ensure_readable(resolved.clone()).is_ok() {
            return Ok((resolved, false));
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
        Ok((resolved, true))
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
    }

    fn create_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let candidate = self.candidate(value)?;
        let session_artifact = self.is_session_artifact(&candidate);
        if candidate.exists() && !session_artifact {
            return Err(ToolError(
                "file already exists; use apply_patch for existing files".to_string(),
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
        if !session_artifact && !self.allows_outside_paths() && !self.is_write_path(&parent) {
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
        if self.is_write_path(&path) {
            Ok(path)
        } else {
            Err(ToolError("path escapes the workspace".to_string()))
        }
    }

    fn is_write_path(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
            || self
                .extra_write_roots
                .iter()
                .any(|root| path.starts_with(root))
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

    fn approve(&self, action: &str) -> Result<(), ToolError> {
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

pub(crate) fn string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError(format!("{name} must be a string")))
}

pub(crate) fn io_error(error: io::Error) -> ToolError {
    ToolError(error.to_string())
}
