use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GOAL_SCHEMA_VERSION: u32 = 2;
pub const MAX_GOAL_PLAN_BYTES: usize = 32 * 1024;
pub const DEFAULT_GOAL_MAX_LOOPS: usize = 20;
pub const DEFAULT_GOAL_MILESTONE_STEPS: usize = 50;
pub const DEFAULT_GOAL_MILESTONE_TIMEOUT_SECS: u64 = 600;

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
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanModeState {
    pub schema_version: u32,
    pub active: bool,
    pub plan_file: PathBuf,
    pub updated_at_ms: u64,
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

/// Host-owned workflow persistence used by the App Server workflow facade.
///
/// This type binds the low-level Goal and Plan files to one session directory.
/// Frontends should use `mini-agent-app-server::WorkflowService`, which keeps
/// this storage seam behind the App Server command boundary.
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

    pub fn init_plan_mode(&self, prompt: Option<&str>) -> io::Result<PathBuf> {
        init_plan_mode_with_prompt(&self.session_dir, prompt)
    }

    pub fn disable_plan_mode(&self) -> io::Result<()> {
        disable_plan_mode(&self.session_dir)
    }

    pub fn plan_active(&self) -> bool {
        is_plan_mode_active(&self.session_dir)
    }

    pub fn init_goal(&self, objective: &str) -> io::Result<GoalState> {
        init_goal_workspace_with_limits(&self.session_dir, objective, self.goal_limits)
    }

    pub fn set_goal(&self, objective: &str, token_budget: Option<i64>) -> io::Result<GoalState> {
        let mut state =
            init_goal_workspace_with_limits(&self.session_dir, objective, self.goal_limits)?;
        state.token_budget = token_budget;
        write_goal_state(&self.session_dir, &state)?;
        Ok(state)
    }

    pub fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        load_goal_state(&self.session_dir)
    }

    pub fn clear_goal(&self) -> io::Result<bool> {
        clear_goal(&self.session_dir)
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

    pub fn pause_goal(&self) -> io::Result<()> {
        pause_goal(&self.session_dir)
    }

    pub fn fail_goal(&self) -> io::Result<GoalState> {
        fail_goal(&self.session_dir)
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn living_plan_path(session_dir: &Path) -> PathBuf {
    session_dir.join("plan.md")
}

use mini_agent_capabilities::normalize_path;

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const LIVING_PLAN_RIDER: &str = "\
=== LIVING PLAN MODE ===
This session is Plan Mode. Keep the software-architect planning discipline.
Write the living plan to plan.md with write_file (it may replace the existing session plan.md). Relative path plan.md maps to the session file.
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
    let plan_state = PlanModeState {
        schema_version: GOAL_SCHEMA_VERSION,
        active: true,
        plan_file: plan_path.clone(),
        updated_at_ms: current_time_ms(),
    };

    let state_json = serde_json::to_vec_pretty(&plan_state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(session_dir.join("plan_mode.json"), state_json)?;

    Ok(plan_path)
}

pub fn disable_plan_mode(session_dir: &Path) -> io::Result<()> {
    let state_file = session_dir.join("plan_mode.json");
    if state_file.exists() {
        let plan_state = PlanModeState {
            schema_version: GOAL_SCHEMA_VERSION,
            active: false,
            plan_file: living_plan_path(session_dir),
            updated_at_ms: current_time_ms(),
        };
        let state_json = serde_json::to_vec_pretty(&plan_state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(state_file, state_json)?;
    }
    Ok(())
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
    let goal_dir = session_dir.join("goal");
    fs::create_dir_all(&goal_dir)?;

    let goal_id = format!("g_{}", current_time_ms() / 1000);
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
        created_at_ms,
        updated_at_ms: created_at_ms,
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

pub fn fail_goal(session_dir: &Path) -> io::Result<GoalState> {
    let state_file = session_dir.join("goal").join("state.json");
    let mut state = load_goal_state(session_dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "goal state not found"))?;
    state.status = GoalStatus::Failed;
    state.updated_at_ms = current_time_ms();
    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(state_file, state_json)?;
    Ok(state)
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
    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(state_file, state_json)?;

    Ok(state)
}

pub fn pause_goal(session_dir: &Path) -> io::Result<()> {
    let goal_dir = session_dir.join("goal");
    let state_file = goal_dir.join("state.json");
    if let Some(mut state) = load_goal_state(session_dir)? {
        state.status = GoalStatus::UserPaused;
        state.updated_at_ms = current_time_ms();
        let state_json = serde_json::to_vec_pretty(&state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(state_file, state_json)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "goal_tests.rs"]
mod tests;
