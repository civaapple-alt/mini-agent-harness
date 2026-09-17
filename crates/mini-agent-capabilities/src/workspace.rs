mod approval;
mod files;
mod patch;
mod shell;

use crate::result_store::ResultStore;
use crate::sandbox::{ProcessSandbox, SandboxKind};
use crate::security::{SecurityDecision, SecurityPolicy, SecurityPreset};
pub use approval::{ApprovalController, ApprovalFailure};
#[cfg(test)]
use files::{ReadFile, ReadImage};
use mini_agent_protocol::{
    Tool, ToolAdmission, ToolApprovalRequest, ToolError, ToolExecutionOutcome,
    ToolExecutionRequest, ToolHandler, ToolRuntime, ToolSpec,
};
#[cfg(test)]
use patch::ApplyPatch;
use serde_json::{Value, json};
#[cfg(test)]
use shell::{Shell, is_read_only_shell_command, run_shell};
#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

const MAX_READ_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 2_000;
const MAX_READ_PAGE_BYTES: usize = 15 * 1024;
pub const MAX_SKILL_READ_BYTES: usize = 64 * 1024;
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
    workspace_tools_with_config(
        WorkspaceToolConfig {
            root,
            approval,
            extra_read_roots,
            skill_read_roots: Vec::new(),
            extra_write_roots,
            sandbox,
        },
        images,
        results,
    )
}

pub(crate) struct WorkspaceToolConfig {
    pub(crate) root: PathBuf,
    pub(crate) approval: ApprovalController,
    pub(crate) extra_read_roots: Vec<PathBuf>,
    pub(crate) skill_read_roots: Vec<PathBuf>,
    pub(crate) extra_write_roots: Vec<PathBuf>,
    pub(crate) sandbox: SandboxKind,
}

pub(crate) fn workspace_tools_with_config(
    config: WorkspaceToolConfig,
    images: crate::image::ImageStore,
    results: ResultStore,
) -> Result<Vec<Box<dyn Tool>>, ToolError> {
    let workspace = Arc::new(Workspace::with_read_roots_and_skill_roots(
        config.root,
        config.approval,
        config.extra_read_roots,
        config.skill_read_roots,
        config.extra_write_roots,
        config.sandbox,
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
    skill_read_roots: Vec<PathBuf>,
    extra_write_roots: Vec<PathBuf>,
    approval: ApprovalController,
    sandbox: SandboxKind,
    skill_read_budget: Mutex<SkillReadBudget>,
}

impl Workspace {
    fn with_read_roots_and_skill_roots(
        root: PathBuf,
        approval: ApprovalController,
        extra_read_roots: Vec<PathBuf>,
        skill_read_roots: Vec<PathBuf>,
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
        let mut skill_read_roots = skill_read_roots
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        skill_read_roots.sort();
        skill_read_roots.dedup();
        let extra_write_roots = extra_write_roots
            .into_iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.is_dir() && !path.starts_with(&root))
            .collect();
        Ok(Self {
            root,
            extra_read_roots,
            skill_read_roots,
            extra_write_roots,
            approval,
            sandbox,
            skill_read_budget: Mutex::new(SkillReadBudget::default()),
        })
    }

    fn read_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        let resolved = self.existing_path(value)?;
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
        let resolved = self.existing_path(value)?;
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
        let resolved = self.existing_path(value)?;
        if self.is_session_artifact(&resolved) {
            return Ok(resolved);
        }
        self.ensure_plan_mode_unlocked()?;
        self.ensure_inside(resolved)
    }

    fn existing_path(&self, value: &Value) -> Result<PathBuf, ToolError> {
        self.candidate(value)?
            .canonicalize()
            .map_err(|error| ToolError(format!("cannot resolve path: {error}")))
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
        if let Some(scratch) = self.approval.plan_scratch()
            && let Some(rest) = crate::path_policy::plan_scratch_relative_rest(path)
        {
            return Ok(scratch.join(rest));
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
        let workspace_candidate = self.root.join(path);
        if workspace_candidate.exists() {
            return Ok(workspace_candidate);
        }
        if let Some(skill_candidate) = self.skill_alias_path(path) {
            return Ok(skill_candidate);
        }
        Ok(workspace_candidate)
    }

    /// Resolve the controlled logical locations exposed in Skill metadata to
    /// an already-authorized global Skill root. This keeps physical user
    /// paths out of model-visible metadata without making `read_file` guess
    /// across arbitrary directories.
    fn skill_alias_path(&self, path: &Path) -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)?;
        let raw = path.to_string_lossy().replace('\\', "/");
        let raw = raw.strip_prefix("./").unwrap_or(&raw);
        for (logical, actual) in [
            (".mini-agent/skills", home.join(".mini-agent/skills")),
            (".agents/skills", home.join(".agents/skills")),
        ] {
            let Some(rest) = raw.strip_prefix(logical) else {
                continue;
            };
            if !rest.is_empty() && !rest.starts_with('/') {
                continue;
            }
            let candidate = actual.join(rest.trim_start_matches('/'));
            let Ok(canonical) = candidate.canonicalize() else {
                continue;
            };
            if self
                .skill_read_roots
                .iter()
                .any(|root| canonical.starts_with(root))
            {
                return Some(canonical);
            }
        }
        None
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
        self.is_living_plan(path) || self.is_plan_scratch(path) || self.is_goal_artifact(path)
    }

    fn is_plan_scratch(&self, path: &Path) -> bool {
        self.approval
            .plan_scratch()
            .is_some_and(|scratch| crate::path_policy::is_under_dir(path, &scratch))
    }

    fn is_plan_scratch_command(&self, command: &str) -> bool {
        let Some(scratch) = self.approval.plan_scratch() else {
            return false;
        };
        if command.chars().any(|character| {
            matches!(
                character,
                '\n' | '\r' | '&' | '>' | '<' | '`' | '(' | ')' | '{' | '}' | ';' | '|'
            )
        }) || command.contains("$(")
        {
            return false;
        }
        let mut tokens = command.split_whitespace();
        let Some(program) = tokens.next() else {
            return false;
        };
        let program = program
            .trim_matches(['\'', '"'])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        if !matches!(program.as_str(), "python" | "python3" | "py" | "node") {
            return false;
        }
        tokens.any(|token| {
            let token = token.trim_matches(['\'', '"']);
            if token.starts_with('-') {
                return false;
            }
            let path = Path::new(token);
            let resolved = if let Some(rest) = crate::path_policy::plan_scratch_relative_rest(path)
            {
                scratch.join(rest)
            } else if path.is_absolute() {
                path.to_path_buf()
            } else {
                scratch.join(path)
            };
            crate::path_policy::is_under_dir(&resolved, &scratch)
                && resolved.extension().is_some_and(|extension| {
                    matches!(
                        extension.to_string_lossy().to_ascii_lowercase().as_str(),
                        "py" | "js" | "mjs" | "cjs"
                    )
                })
        })
    }

    fn shell_root(&self, command: &str) -> PathBuf {
        if self.is_plan_scratch_command(command)
            && let Some(scratch) = self.approval.plan_scratch()
        {
            if command.split_whitespace().any(|token| {
                crate::path_policy::plan_scratch_relative_rest(Path::new(
                    token.trim_matches(['\'', '"']),
                ))
                .is_some()
            }) {
                return self.root.clone();
            }
            return scratch;
        }
        self.root.clone()
    }

    fn is_bounded_read_only_shell_command(&self, command: &str) -> bool {
        shell::is_read_only_shell_command(command)
            && command.split([';', '|']).all(|segment| {
                !segment.is_empty()
                    && segment
                        .split_whitespace()
                        .all(|token| self.is_bounded_shell_token(token))
            })
    }

    fn is_bounded_shell_token(&self, token: &str) -> bool {
        let token = token.trim_matches(['\'', '"']);
        if token.is_empty()
            || token.contains(['$', '%'])
            || token.starts_with('~')
            || token.contains("://")
            || token.contains('=')
        {
            return false;
        }
        !shell_token_looks_like_path(token) || self.is_readable_shell_path(token)
    }

    fn is_readable_shell_path(&self, token: &str) -> bool {
        let token = token.trim_matches(['\'', '"']);
        if token.contains(':')
            && !(cfg!(windows)
                && token.len() > 2
                && token.as_bytes()[1] == b':'
                && matches!(token.as_bytes()[2], b'/' | b'\\'))
        {
            return false;
        }
        if Path::new(token)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return false;
        }
        let Ok(path) = self
            .candidate(&json!({"path": token}))
            .map(|path| crate::path_policy::normalize_path(&path))
        else {
            return false;
        };
        (path.starts_with(&self.root)
            || self
                .extra_read_roots
                .iter()
                .any(|root| path.starts_with(root)))
            && self.ensure_readable(path).is_ok()
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
            || self
                .skill_read_roots
                .iter()
                .any(|root| path.starts_with(root) && path != *root)
        {
            Ok(path)
        } else {
            Err(ToolError("path escapes the workspace".to_string()))
        }
    }

    fn record_skill_read(
        &self,
        path: &Path,
        turn_id: Option<&str>,
        bytes: usize,
    ) -> Result<(), ToolError> {
        if !self
            .skill_read_roots
            .iter()
            .any(|root| path.starts_with(root))
        {
            return Ok(());
        }
        self.skill_read_budget
            .lock()
            .map_err(|_| ToolError("skill read budget is unavailable".to_string()))?
            .record(turn_id, bytes)
    }

    fn approve(&self, action: &str) -> Result<(), ToolError> {
        self.approval.approve(action)
    }
}

#[derive(Default)]
struct SkillReadBudget {
    turn_id: Option<String>,
    bytes: usize,
}

impl SkillReadBudget {
    fn record(&mut self, turn_id: Option<&str>, bytes: usize) -> Result<(), ToolError> {
        let Some(turn_id) = turn_id else {
            return Ok(());
        };
        if self.turn_id.as_deref() != Some(turn_id) {
            self.turn_id = Some(turn_id.to_string());
            self.bytes = 0;
        }
        let total = self.bytes.saturating_add(bytes);
        if total > MAX_SKILL_READ_BYTES {
            return Err(ToolError(format!(
                "skill file reads exceed {MAX_SKILL_READ_BYTES} bytes for this turn"
            )));
        }
        self.bytes = total;
        Ok(())
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

fn shell_token_looks_like_path(token: &str) -> bool {
    token.starts_with(['.', '~'])
        || token
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b':'))
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
