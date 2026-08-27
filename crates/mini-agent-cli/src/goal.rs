use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub goal_id: String,
    pub status: GoalStatus,
    pub current_milestone: usize,
    pub total_milestones: usize,
    pub loop_count: usize,
    pub max_loops: usize,
    pub last_verifier_score: Option<u32>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanModeState {
    pub active: bool,
    pub plan_file: PathBuf,
    pub updated_at_ms: u64,
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn init_plan_mode(session_dir: &Path) -> io::Result<PathBuf> {
    let plan_path = session_dir.join("plan.md");
    if !plan_path.exists() {
        let initial_plan = r#"# Implementation Plan

## 1. Problem & Scope
- Goals:
- Non-Goals:

## 2. Critical Files
- `path/to/file` [MODIFY/NEW]

## 3. Phased Milestones
- [ ] Phase 1: Exploration & Setup
- [ ] Phase 2: Core Implementation
- [ ] Phase 3: Verification & Tests

## 4. Verification Plan
- Automated test checks
"#;
        fs::write(&plan_path, initial_plan)?;
    }

    let plan_state = PlanModeState {
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
            active: false,
            plan_file: session_dir.join("plan.md"),
            updated_at_ms: current_time_ms(),
        };
        let state_json = serde_json::to_vec_pretty(&plan_state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(state_file, state_json)?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn is_plan_mode_active(session_dir: &Path) -> bool {
    let state_file = session_dir.join("plan_mode.json");
    if let Ok(content) = fs::read_to_string(state_file)
        && let Ok(state) = serde_json::from_str::<PlanModeState>(&content)
    {
        return state.active;
    }
    false
}

pub fn init_goal_workspace(
    session_dir: &Path,
    objective: &str,
    max_loops: usize,
) -> io::Result<GoalState> {
    let goal_dir = session_dir.join("goal");
    fs::create_dir_all(&goal_dir)?;

    let goal_id = format!("g_{}", current_time_ms() / 1000);
    let state = GoalState {
        goal_id,
        status: GoalStatus::Running,
        current_milestone: 1,
        total_milestones: 3,
        loop_count: 0,
        max_loops: if max_loops == 0 { 20 } else { max_loops },
        last_verifier_score: None,
        updated_at_ms: current_time_ms(),
    };

    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(goal_dir.join("state.json"), state_json)?;

    let initial_goal_plan = format!(
        r#"# Autonomous Goal Plan: {objective}

## Acceptance Criteria
- Full workspace tests and linting pass with zero warnings.
- Independent mentor verifier gives APPROVED verdict.

## Milestones
- [ ] Milestone 1: Workspace inspection and baseline test verification.
- [ ] Milestone 2: Incremental implementation and unit testing.
- [ ] Milestone 3: Full end-to-end verification and cleanup.
"#
    );
    fs::write(goal_dir.join("plan.md"), initial_goal_plan)?;

    Ok(state)
}

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn advance_goal_milestone(
    session_dir: &Path,
    verifier_score: Option<u32>,
) -> io::Result<GoalState> {
    let goal_dir = session_dir.join("goal");
    let state_file = goal_dir.join("state.json");
    let mut state = load_goal_state(session_dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "goal state not found"))?;

    state.loop_count += 1;
    state.last_verifier_score = verifier_score;
    state.updated_at_ms = current_time_ms();

    if state.current_milestone >= state.total_milestones {
        state.status = GoalStatus::Converged;
    } else {
        state.current_milestone += 1;
    }

    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(state_file, state_json)?;

    Ok(state)
}

#[allow(dead_code)]
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
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    fn test_dir() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mini-agent-goal-test-{nonce}-{seq}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plan_mode_lifecycle_creates_and_toggles_state() {
        let dir = test_dir();
        assert!(!is_plan_mode_active(&dir));

        let plan_file = init_plan_mode(&dir).unwrap();
        assert!(plan_file.is_file());
        assert!(is_plan_mode_active(&dir));

        disable_plan_mode(&dir).unwrap();
        assert!(!is_plan_mode_active(&dir));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn goal_workspace_lifecycle_and_milestones() {
        let dir = test_dir();
        let state = init_goal_workspace(&dir, "Refactor auth", 10).unwrap();
        assert_eq!(state.status, GoalStatus::Running);
        assert_eq!(state.current_milestone, 1);
        assert_eq!(state.total_milestones, 3);
        assert_eq!(state.max_loops, 10);

        let plan_file = dir.join("goal/plan.md");
        assert!(plan_file.is_file());
        let plan_content = fs::read_to_string(plan_file).unwrap();
        assert!(plan_content.contains("Autonomous Goal Plan: Refactor auth"));

        let next = advance_goal_milestone(&dir, Some(85)).unwrap();
        assert_eq!(next.current_milestone, 2);
        assert_eq!(next.loop_count, 1);
        assert_eq!(next.last_verifier_score, Some(85));

        let next2 = advance_goal_milestone(&dir, Some(95)).unwrap();
        assert_eq!(next2.current_milestone, 3);

        let final_state = advance_goal_milestone(&dir, Some(100)).unwrap();
        assert_eq!(final_state.status, GoalStatus::Converged);

        pause_goal(&dir).unwrap();
        let paused = load_goal_state(&dir).unwrap().unwrap();
        assert_eq!(paused.status, GoalStatus::UserPaused);

        fs::remove_dir_all(dir).unwrap();
    }
}
