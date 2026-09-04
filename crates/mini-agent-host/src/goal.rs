use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GOAL_SCHEMA_VERSION: u32 = 3;
pub const MAX_GOAL_OBJECTIVE_BYTES: usize = 8 * 1024;
pub const MAX_GOAL_PLAN_BYTES: usize = 32 * 1024;
pub const DEFAULT_GOAL_MAX_LOOPS: usize = 100;
pub const DEFAULT_GOAL_MILESTONE_STEPS: usize = 200;
pub const DEFAULT_GOAL_MILESTONE_TIMEOUT_SECS: u64 = 1_800;
pub const MAX_PLAN_SCRATCH_ENTRIES: usize = 256;
pub const MAX_PLAN_SCRATCH_BYTES: u64 = 16 * 1024 * 1024;
const MAX_GOAL_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalLimits {
    pub max_loops: usize,
    pub milestone_step_budget: usize,
    pub milestone_timeout_secs: u64,
}

impl Default for GoalLimits {
    fn default() -> Self {
        Self {
            max_loops: DEFAULT_GOAL_MAX_LOOPS,
            milestone_step_budget: DEFAULT_GOAL_MILESTONE_STEPS,
            milestone_timeout_secs: DEFAULT_GOAL_MILESTONE_TIMEOUT_SECS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Running,
    Converged,
    Failed,
    UserPaused,
    UsageLimited,
    BudgetLimited,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalState {
    pub schema_version: u32,
    pub goal_id: String,
    #[serde(default)]
    pub objective: String,
    pub status: GoalStatus,
    pub current_milestone: usize,
    pub total_milestones: usize,
    pub loop_count: usize,
    pub max_loops: usize,
    pub milestone_step_budget: usize,
    pub milestone_timeout_secs: u64,
    pub verifier_model: Option<String>,
    pub last_verifier_score: Option<u32>,
    #[serde(default)]
    pub token_budget: Option<i64>,
    #[serde(default)]
    pub tokens_used: i64,
    #[serde(default)]
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub active_turn_id: Option<String>,
    #[serde(default)]
    pub active_turn_settled: bool,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanModeState {
    pub schema_version: u32,
    pub active: bool,
    pub plan_file: PathBuf,
    pub scratch_dir: PathBuf,
    pub cleanup_manifest: PathBuf,
    pub cleanup_pending: bool,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanCleanupManifest {
    pub schema_version: u32,
    pub status: String,
    pub scratch_dir: PathBuf,
    pub paths: Vec<String>,
    pub updated_at_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictOutcome {
    Approved,
    Rejected,
    NeedsClarification,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierVerdict {
    pub outcome: VerdictOutcome,
    pub score: Option<u32>,
    pub summary: String,
}

/// Host-owned Goal persistence used by the App Server Goal service.
///
/// This type binds the low-level Goal and Plan files to one session directory.
/// The App Server keeps this storage seam behind the Goal command boundary.
#[derive(Clone, Debug)]
pub struct HostWorkflowStore {
    session_dir: PathBuf,
    goal_limits: GoalLimits,
}

impl HostWorkflowStore {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            session_dir: session_dir.into(),
            goal_limits,
        }
    }

    pub fn goal_limits(&self) -> GoalLimits {
        self.goal_limits
    }

    pub fn goal_dir(&self) -> PathBuf {
        self.session_dir.join("goal")
    }

    pub fn init_plan_mode(&self, prompt: Option<&str>) -> io::Result<PathBuf> {
        init_plan_mode_with_prompt(&self.session_dir, prompt)
    }

    pub fn disable_plan_mode(&self) -> io::Result<()> {
        disable_plan_mode(&self.session_dir)
    }

    pub fn cleanup_plan_scratch(&self) -> io::Result<()> {
        cleanup_plan_scratch(&self.session_dir).map(|_| ())
    }

    pub fn plan_active(&self) -> bool {
        is_plan_mode_active(&self.session_dir)
    }

    pub fn set_goal(&self, objective: &str, token_budget: Option<i64>) -> io::Result<GoalState> {
        if token_budget.is_some_and(|budget| budget <= 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "goal token budget must be positive",
            ));
        }
        let mut state =
            init_goal_workspace_with_limits(&self.session_dir, objective, self.goal_limits)?;
        state.token_budget = token_budget;
        write_goal_state(&self.session_dir, &state)?;
        Ok(state)
    }

    pub fn update_goal(
        &self,
        objective: Option<&str>,
        token_budget: Option<Option<i64>>,
    ) -> io::Result<Option<GoalState>> {
        if objective.is_some_and(|value| value.trim().is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "goal objective must not be empty",
            ));
        }
        if token_budget.flatten().is_some_and(|budget| budget <= 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "goal token budget must be positive",
            ));
        }
        update_goal_state(&self.session_dir, |state| {
            if objective.is_none() && token_budget.is_none() {
                return Ok(Some(state.clone()));
            }
            if let Some(objective) = objective {
                state.objective = objective.trim().to_string();
            }
            if let Some(token_budget) = token_budget {
                state.token_budget = token_budget;
            }
            state.updated_at_ms = current_time_ms();
            Ok(Some(state.clone()))
        })
    }

    pub fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        load_goal_state(&self.session_dir)
    }

    pub fn clear_goal(&self) -> io::Result<bool> {
        clear_goal(&self.session_dir)
    }

    pub fn mark_goal_turn_started(
        &self,
        goal_id: &str,
        turn_id: &str,
    ) -> io::Result<Option<GoalState>> {
        update_goal_state(&self.session_dir, |state| {
            if state.goal_id != goal_id || state.status != GoalStatus::Running {
                return Ok(None);
            }
            state.active_turn_id = Some(turn_id.to_string());
            state.active_turn_settled = false;
            state.updated_at_ms = current_time_ms();
            Ok(Some(state.clone()))
        })
    }

    pub fn mark_goal_turn_settled(
        &self,
        goal_id: &str,
        turn_id: &str,
    ) -> io::Result<Option<GoalState>> {
        update_goal_state(&self.session_dir, |state| {
            if state.goal_id != goal_id
                || state.status != GoalStatus::Running
                || state.active_turn_id.as_deref() != Some(turn_id)
            {
                return Ok(None);
            }
            state.active_turn_settled = true;
            state.updated_at_ms = current_time_ms();
            Ok(Some(state.clone()))
        })
    }

    pub fn verification_criteria(&self) -> io::Result<String> {
        goal_verification_criteria(&self.session_dir)
    }

    pub fn record_verifier_verdict(&self, checkpoint_seq: u64, output: &str) -> io::Result<()> {
        record_verifier_verdict(&self.session_dir, checkpoint_seq, output)
    }

    pub fn advance_goal(&self, verdict: Option<VerifierVerdict>) -> io::Result<GoalState> {
        advance_goal_milestone(&self.session_dir, verdict)
    }

    pub fn pause_goal(&self) -> io::Result<Option<GoalState>> {
        pause_goal(&self.session_dir)
    }

    pub fn resume_goal(&self) -> io::Result<Option<GoalState>> {
        resume_goal(&self.session_dir)
    }

    pub fn fail_goal_with_reason(&self, reason: &str) -> io::Result<GoalState> {
        fail_goal_with_reason(&self.session_dir, Some(reason))
    }

    pub fn record_goal_usage(
        &self,
        goal_id: &str,
        turn_id: &str,
        tokens: u64,
    ) -> io::Result<Option<GoalState>> {
        update_goal_state(&self.session_dir, |state| {
            if state.goal_id != goal_id
                || state.status != GoalStatus::Running
                || state.active_turn_id.as_deref() != Some(turn_id)
            {
                return Ok(None);
            }
            state.tokens_used = state
                .tokens_used
                .saturating_add(i64::try_from(tokens).unwrap_or(i64::MAX));
            if state
                .token_budget
                .is_some_and(|budget| state.tokens_used >= budget)
            {
                state.status = GoalStatus::BudgetLimited;
                state.last_error = Some("goal token budget exhausted".to_string());
            }
            state.updated_at_ms = current_time_ms();
            Ok(Some(state.clone()))
        })
    }

    pub fn limit_goal_turn(
        &self,
        goal_id: &str,
        turn_id: &str,
        status: GoalStatus,
        reason: &str,
    ) -> io::Result<Option<GoalState>> {
        update_goal_state(&self.session_dir, |state| {
            if state.goal_id != goal_id
                || state.status != GoalStatus::Running
                || state.active_turn_id.as_deref() != Some(turn_id)
            {
                return Ok(None);
            }
            state.status = status;
            state.last_error = Some(reason.to_string());
            state.updated_at_ms = current_time_ms();
            Ok(Some(state.clone()))
        })
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn living_plan_path(session_dir: &Path) -> PathBuf {
    session_dir.join("plan").join("plan.md")
}

pub fn plan_scratch_path(session_dir: &Path) -> PathBuf {
    session_dir.join("plan").join("scratch")
}

pub fn plan_cleanup_path(session_dir: &Path) -> PathBuf {
    session_dir.join("plan").join("cleanup.json")
}

use mini_agent_capabilities::normalize_path;

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const LIVING_PLAN_RIDER: &str = "\
=== LIVING PLAN MODE ===
This session is Plan Mode. Keep the software-architect planning discipline.
Write the living plan to plan.md with apply_patch. Relative path plan.md maps to the Session-owned plan file.
For bounded exploration, write scripts and outputs under plan/scratch/; they are disposable and cleaned after the turn.
Do not produce the final deliverable in reasoning or the assistant message: no complete HTML/CSS/JS pages, full source files, or finished documents.
Research only to inform the plan. Cite sources; do not copy full page content.
Reply with a short summary, risks, and open questions.";

pub fn with_plan_mode_overlay(base: &str) -> String {
    let architect = mini_agent_capabilities::AgentPromptKind::Plan.prompt_template();
    if base.contains("=== LIVING PLAN MODE ===") {
        base.to_string()
    } else {
        format!("{base}\n\n{architect}\n\n{LIVING_PLAN_RIDER}")
    }
}

pub fn goal_turn_prompt(objective: &str, milestone: usize, total: usize) -> String {
    format!(
        "Autonomous Goal Mode is active. Execute the objective now without waiting for another prompt. Current milestone {milestone}/{total}. Read and update goal/plan.md (relative path maps to the session goal file). If a previous verifier rejected the milestone, read goal/verifier_verdict.md and address its findings. Use tools and keep working until this milestone is done.\n\nObjective:\n{objective}"
    )
}

fn initial_plan_markdown(prompt: Option<&str>) -> String {
    let goals = match prompt.map(str::trim).filter(|text| !text.is_empty()) {
        Some(prompt) => format!("- Goals:\n  - {prompt}"),
        None => "- Goals:".to_string(),
    };
    format!(
        "# Implementation Plan\n\n## 1. Problem & Scope\n{goals}\n- Non-Goals:\n\n## 2. Critical Files\n- `path/to/file` [MODIFY/NEW]\n\n## 3. Phased Milestones\n- [ ] Phase 1: Exploration & Setup\n- [ ] Phase 2: Core Implementation\n- [ ] Phase 3: Verification & Tests\n\n## 4. Verification Plan\n- Automated test checks\n"
    )
}

pub fn init_plan_mode_with_prompt(session_dir: &Path, prompt: Option<&str>) -> io::Result<PathBuf> {
    let scratch_dir = plan_scratch_path(session_dir);
    fs::create_dir_all(&scratch_dir)?;
    let plan_path = living_plan_path(session_dir);
    let prompt = prompt.map(one_line).filter(|text| !text.is_empty());
    if plan_path.exists() {
        if let Some(prompt) = prompt.as_deref() {
            let mut content = fs::read_to_string(&plan_path)?;
            let marker = format!("- {prompt}");
            if !content.contains(&marker) {
                content.push_str(&format!("\n## User Request\n{marker}\n"));
                fs::write(&plan_path, content)?;
            }
        }
    } else {
        fs::write(&plan_path, initial_plan_markdown(prompt.as_deref()))?;
    }

    let plan_path = normalize_path(&plan_path);
    let scratch_dir = normalize_path(&scratch_dir);
    let cleanup_manifest = normalize_path(&plan_cleanup_path(session_dir));
    write_plan_cleanup_manifest(session_dir, "active", Vec::new(), None)?;
    let plan_state = PlanModeState {
        schema_version: GOAL_SCHEMA_VERSION,
        active: true,
        plan_file: plan_path.clone(),
        scratch_dir,
        cleanup_manifest,
        cleanup_pending: false,
        updated_at_ms: current_time_ms(),
    };

    let state_json = serde_json::to_vec_pretty(&plan_state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(session_dir.join("plan_mode.json"), state_json)?;

    Ok(plan_path)
}

pub fn disable_plan_mode(session_dir: &Path) -> io::Result<()> {
    let cleanup_result = cleanup_plan_scratch(session_dir);
    let state_file = session_dir.join("plan_mode.json");
    let mut state_result = Ok(());
    if state_file.exists() {
        let plan_state = PlanModeState {
            schema_version: GOAL_SCHEMA_VERSION,
            active: false,
            plan_file: living_plan_path(session_dir),
            scratch_dir: plan_scratch_path(session_dir),
            cleanup_manifest: plan_cleanup_path(session_dir),
            cleanup_pending: cleanup_result.is_err(),
            updated_at_ms: current_time_ms(),
        };
        let state_json = serde_json::to_vec_pretty(&plan_state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        state_result = fs::write(state_file, state_json);
    }
    match (cleanup_result, state_result) {
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
        (Ok(_), Ok(())) => Ok(()),
    }
}

pub fn cleanup_plan_scratch(session_dir: &Path) -> io::Result<PlanCleanupManifest> {
    let scratch_dir = plan_scratch_path(session_dir);
    let mut paths = Vec::new();
    let mut bytes = 0;
    let collect_result = if scratch_dir.is_dir() {
        collect_plan_scratch_entries(&scratch_dir, &scratch_dir, &mut paths, &mut bytes)
    } else {
        Ok(())
    };
    if let Err(error) = collect_result {
        let _ = write_plan_cleanup_manifest(
            session_dir,
            "cleanup_pending",
            paths,
            Some(error.to_string()),
        );
        return Err(error);
    }

    write_plan_cleanup_manifest(session_dir, "cleanup_pending", paths.clone(), None)?;
    let removal_result = if scratch_dir.exists() {
        fs::remove_dir_all(&scratch_dir)
    } else {
        Ok(())
    };
    if let Err(error) = removal_result {
        let message = error.to_string();
        let _ = write_plan_cleanup_manifest(
            session_dir,
            "cleanup_pending",
            paths,
            Some(message.clone()),
        );
        return Err(io::Error::new(error.kind(), message));
    }
    fs::create_dir_all(&scratch_dir)?;
    write_plan_cleanup_manifest(session_dir, "clean", Vec::new(), None)
}

fn collect_plan_scratch_entries(
    root: &Path,
    current: &Path,
    paths: &mut Vec<String>,
    bytes: &mut u64,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if paths.len() >= MAX_PLAN_SCRATCH_ENTRIES {
            return Err(io::Error::other(format!(
                "Plan scratch exceeds {MAX_PLAN_SCRATCH_ENTRIES} entries"
            )));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| io::Error::other(error.to_string()))?;
        paths.push(relative.display().to_string());
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            *bytes = bytes.saturating_add(metadata.len());
            if *bytes > MAX_PLAN_SCRATCH_BYTES {
                return Err(io::Error::other(format!(
                    "Plan scratch exceeds {MAX_PLAN_SCRATCH_BYTES} bytes"
                )));
            }
        } else if metadata.is_dir() {
            collect_plan_scratch_entries(root, &path, paths, bytes)?;
        }
    }
    Ok(())
}

fn write_plan_cleanup_manifest(
    session_dir: &Path,
    status: &str,
    paths: Vec<String>,
    error: Option<String>,
) -> io::Result<PlanCleanupManifest> {
    let manifest = PlanCleanupManifest {
        schema_version: GOAL_SCHEMA_VERSION,
        status: status.to_string(),
        scratch_dir: normalize_path(&plan_scratch_path(session_dir)),
        paths,
        updated_at_ms: current_time_ms(),
        error: error.map(|value| value.chars().take(MAX_GOAL_ERROR_CHARS).collect()),
    };
    let content = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::create_dir_all(
        plan_cleanup_path(session_dir)
            .parent()
            .expect("plan directory"),
    )?;
    fs::write(plan_cleanup_path(session_dir), content)?;
    Ok(manifest)
}

pub fn is_plan_mode_active(session_dir: &Path) -> bool {
    let state_file = session_dir.join("plan_mode.json");
    if let Ok(content) = fs::read_to_string(state_file)
        && let Ok(state) = serde_json::from_str::<PlanModeState>(&content)
    {
        return state.active;
    }
    false
}

pub fn init_goal_workspace_with_limits(
    session_dir: &Path,
    objective: &str,
    limits: GoalLimits,
) -> io::Result<GoalState> {
    if objective.len() > MAX_GOAL_OBJECTIVE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("goal objective exceeds {MAX_GOAL_OBJECTIVE_BYTES} byte limit"),
        ));
    }
    let goal_dir = session_dir.join("goal");
    fs::create_dir_all(&goal_dir)?;

    let goal_id = format!("g_{}", current_time_ms());
    let created_at_ms = current_time_ms();
    let state = GoalState {
        schema_version: GOAL_SCHEMA_VERSION,
        goal_id,
        objective: objective.to_string(),
        status: GoalStatus::Running,
        current_milestone: 1,
        total_milestones: 3,
        loop_count: 0,
        max_loops: if limits.max_loops == 0 {
            DEFAULT_GOAL_MAX_LOOPS
        } else {
            limits.max_loops
        },
        milestone_step_budget: limits.milestone_step_budget,
        milestone_timeout_secs: limits.milestone_timeout_secs,
        verifier_model: std::env::var("VERIFIER_OPENAI_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        last_verifier_score: None,
        token_budget: None,
        tokens_used: 0,
        created_at_ms,
        updated_at_ms: created_at_ms,
        active_turn_id: None,
        active_turn_settled: false,
        last_error: None,
    };

    write_goal_state(session_dir, &state)?;

    let root_plan = living_plan_path(session_dir);
    if root_plan.is_file() {
        let _ = fs::copy(&root_plan, goal_dir.join("plan.baseline.md"));
        let _ = fs::copy(&root_plan, goal_dir.join("plan.md"));
    } else {
        let initial_goal_plan = format!(
            r#"# Autonomous Goal Plan: {objective}

## Acceptance Criteria
- Full workspace tests and linting pass with zero warnings.
- Independent Goal verifier gives APPROVED verdict.

## Milestones
- [ ] Milestone 1: Workspace inspection and baseline test verification.
- [ ] Milestone 2: Incremental implementation and unit testing.
- [ ] Milestone 3: Full end-to-end verification and cleanup.
"#
        );
        fs::write(goal_dir.join("plan.md"), initial_goal_plan)?;
    }

    Ok(state)
}

pub fn load_goal_state(session_dir: &Path) -> io::Result<Option<GoalState>> {
    let state_file = session_dir.join("goal").join("state.json");
    if !state_file.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(state_file)?;
    let state = serde_json::from_str::<GoalState>(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(state))
}

fn write_goal_state(session_dir: &Path, state: &GoalState) -> io::Result<()> {
    let state_json = serde_json::to_vec_pretty(state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(session_dir.join("goal").join("state.json"), state_json)
}

fn update_goal_state<F>(session_dir: &Path, update: F) -> io::Result<Option<GoalState>>
where
    F: FnOnce(&mut GoalState) -> io::Result<Option<GoalState>>,
{
    let mut state = load_goal_state(session_dir)?;
    let Some(state) = state.as_mut() else {
        return Ok(None);
    };
    let updated = update(state)?;
    if updated.is_some() {
        write_goal_state(session_dir, state)?;
    }
    Ok(updated)
}

pub fn clear_goal(session_dir: &Path) -> io::Result<bool> {
    let state_file = session_dir.join("goal").join("state.json");
    match fs::remove_file(state_file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn goal_verification_criteria(session_dir: &Path) -> io::Result<String> {
    let plan = fs::read_to_string(session_dir.join("goal").join("plan.md"))?;
    if plan.len() > MAX_GOAL_PLAN_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("goal plan exceeds {MAX_GOAL_PLAN_BYTES} byte limit"),
        ));
    }
    Ok(plan)
}

pub fn record_verifier_verdict(
    session_dir: &Path,
    checkpoint_seq: u64,
    output: &str,
) -> io::Result<()> {
    let record = format!("source_checkpoint_seq: {checkpoint_seq}\n\n{output}");
    if record.len() > MAX_GOAL_PLAN_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("verifier output exceeds {MAX_GOAL_PLAN_BYTES} byte limit"),
        ));
    }
    fs::write(session_dir.join("goal").join("verifier_verdict.md"), record)
}

pub fn limit_goal_with_reason(
    session_dir: &Path,
    status: GoalStatus,
    reason: Option<&str>,
) -> io::Result<GoalState> {
    let state_file = session_dir.join("goal").join("state.json");
    let mut state = load_goal_state(session_dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "goal state not found"))?;
    state.status = status;
    state.last_error = reason.map(|reason| reason.chars().take(MAX_GOAL_ERROR_CHARS).collect());
    state.updated_at_ms = current_time_ms();
    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(state_file, state_json)?;
    Ok(state)
}

pub fn fail_goal_with_reason(session_dir: &Path, reason: Option<&str>) -> io::Result<GoalState> {
    limit_goal_with_reason(session_dir, GoalStatus::Failed, reason)
}

pub fn parse_verifier_verdict(content: &str) -> VerifierVerdict {
    let lower = content.to_ascii_lowercase();
    let outcome = if lower.contains("verdict: approved")
        || lower.contains("verdict: approve")
        || lower.contains("**verdict**: approved")
    {
        VerdictOutcome::Approved
    } else if lower.contains("verdict: needs_clarification")
        || lower.contains("verdict: clarification")
    {
        VerdictOutcome::NeedsClarification
    } else if lower.contains("verdict: rejected") || lower.contains("verdict: reject") {
        VerdictOutcome::Rejected
    } else {
        VerdictOutcome::Invalid
    };

    let score = content
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("score:"))
        .and_then(|l| {
            l.split(':')
                .nth(1)?
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok()
        });

    let summary = content
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("summary:") || l.starts_with("### Summary"))
        .unwrap_or("No summary provided")
        .to_string();

    VerifierVerdict {
        outcome,
        score,
        summary,
    }
}

pub fn advance_goal_milestone(
    session_dir: &Path,
    verdict: Option<VerifierVerdict>,
) -> io::Result<GoalState> {
    let goal_dir = session_dir.join("goal");
    let state_file = goal_dir.join("state.json");
    let mut state = load_goal_state(session_dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "goal state not found"))?;

    if state.status != GoalStatus::Running {
        return Ok(state);
    }

    state.loop_count += 1;
    if let Some(ref v) = verdict {
        state.last_verifier_score = v.score;
        if v.outcome == VerdictOutcome::Approved {
            if state.current_milestone >= state.total_milestones {
                state.status = GoalStatus::Converged;
            } else {
                state.current_milestone += 1;
            }
        } else if state.loop_count >= state.max_loops {
            state.status = GoalStatus::Failed;
        }
    } else if state.loop_count >= state.max_loops {
        state.status = GoalStatus::Failed;
    }

    state.updated_at_ms = current_time_ms();
    state.active_turn_id = None;
    state.active_turn_settled = false;
    state.last_error = None;
    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(state_file, state_json)?;

    Ok(state)
}

pub fn pause_goal(session_dir: &Path) -> io::Result<Option<GoalState>> {
    update_goal_state(session_dir, |state| {
        if state.status == GoalStatus::UserPaused {
            return Ok(None);
        }
        state.status = GoalStatus::UserPaused;
        state.active_turn_id = None;
        state.active_turn_settled = false;
        state.updated_at_ms = current_time_ms();
        Ok(Some(state.clone()))
    })
}

pub fn resume_goal(session_dir: &Path) -> io::Result<Option<GoalState>> {
    update_goal_state(session_dir, |state| {
        if state.status != GoalStatus::UserPaused {
            return Ok(None);
        }
        state.status = GoalStatus::Running;
        state.updated_at_ms = current_time_ms();
        Ok(Some(state.clone()))
    })
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
