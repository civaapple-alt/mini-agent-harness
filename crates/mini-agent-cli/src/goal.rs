use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GOAL_SCHEMA_VERSION: u32 = 1;
pub const MAX_GOAL_PLAN_BYTES: usize = 32 * 1024;

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
    pub status: GoalStatus,
    pub current_milestone: usize,
    pub total_milestones: usize,
    pub loop_count: usize,
    pub max_loops: usize,
    pub milestone_step_budget: usize,
    pub milestone_timeout_secs: u64,
    pub verifier_model: Option<String>,
    pub last_verifier_score: Option<u32>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierVerdict {
    pub outcome: VerdictOutcome,
    pub score: Option<u32>,
    pub summary: String,
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanSlash {
    Enable { prompt: Option<String> },
    Disable,
}

pub fn parse_plan_slash(input: &str) -> Option<PlanSlash> {
    let rest = input.strip_prefix("/plan")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = unquote(rest.trim());
    if rest.is_empty() || rest == "on" {
        Some(PlanSlash::Enable { prompt: None })
    } else if rest == "off" {
        Some(PlanSlash::Disable)
    } else {
        Some(PlanSlash::Enable {
            prompt: Some(one_line(rest)),
        })
    }
}

pub fn living_plan_path(session_dir: &Path) -> PathBuf {
    session_dir.join("plan.md")
}

pub fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let Some(parent) = path.parent()
        && let Ok(parent) = parent.canonicalize()
        && let Some(name) = path.file_name()
    {
        return parent.join(name);
    }
    path.to_path_buf()
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    left == right || normalize_path(left) == normalize_path(right)
}

pub fn is_plan_md_alias(path: &Path) -> bool {
    let mut name = None;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) if name.is_none() => name = Some(part),
            _ => return false,
        }
    }
    name.is_some_and(|name| name.eq_ignore_ascii_case("plan.md"))
}

pub fn goal_relative_rest(path: &Path) -> Option<PathBuf> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part),
            _ => return None,
        }
    }
    let name = parts.first()?;
    if !name.eq_ignore_ascii_case("goal") || parts.len() < 2 {
        return None;
    }
    Some(parts.into_iter().skip(1).collect())
}

pub fn is_under_dir(path: &Path, dir: &Path) -> bool {
    let path = normalize_path(path);
    let dir = normalize_path(dir);
    path.starts_with(&dir) && path != dir
}

fn unquote(text: &str) -> &str {
    let bytes = text.as_bytes();
    let last = bytes.last().copied();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && last == Some(b'"')) || (bytes[0] == b'\'' && last == Some(b'\'')))
    {
        text[1..text.len() - 1].trim()
    } else {
        text
    }
}

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
    let architect = crate::persona::AgentPromptKind::Plan.prompt_template();
    if base.contains("=== LIVING PLAN MODE ===") {
        base.to_string()
    } else {
        format!("{base}\n\n{architect}\n\n{LIVING_PLAN_RIDER}")
    }
}

pub fn planning_turn_prompt(request: &str) -> String {
    format!(
        "Draft or update the living plan for this request. Do not produce the final deliverable.\n\nRequest:\n{request}"
    )
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

#[allow(dead_code)]
pub fn is_living_plan_whitelisted(target: &Path, session_dir: &Path) -> bool {
    same_path(target, &living_plan_path(session_dir))
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
        schema_version: GOAL_SCHEMA_VERSION,
        goal_id,
        status: GoalStatus::Running,
        current_milestone: 1,
        total_milestones: 3,
        loop_count: 0,
        max_loops: if max_loops == 0 { 20 } else { max_loops },
        milestone_step_budget: 50,
        milestone_timeout_secs: 600,
        verifier_model: std::env::var("MENTOR_OPENAI_MODEL").ok(),
        last_verifier_score: None,
        updated_at_ms: current_time_ms(),
    };

    let state_json = serde_json::to_vec_pretty(&state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(goal_dir.join("state.json"), state_json)?;

    let root_plan = living_plan_path(session_dir);
    if root_plan.is_file() {
        let _ = fs::copy(&root_plan, goal_dir.join("plan.baseline.md"));
        let _ = fs::copy(&root_plan, goal_dir.join("plan.md"));
    } else {
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
    }

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

#[allow(dead_code)]
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
    } else {
        VerdictOutcome::Rejected
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

#[allow(dead_code)]
pub fn advance_goal_milestone(
    session_dir: &Path,
    verdict: Option<VerifierVerdict>,
) -> io::Result<GoalState> {
    let goal_dir = session_dir.join("goal");
    let state_file = goal_dir.join("state.json");
    let mut state = load_goal_state(session_dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "goal state not found"))?;

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
    fn parse_plan_slash_accepts_prompt_and_ignores_lookalikes() {
        assert_eq!(
            parse_plan_slash("/plan"),
            Some(PlanSlash::Enable { prompt: None })
        );
        assert_eq!(
            parse_plan_slash("/plan on"),
            Some(PlanSlash::Enable { prompt: None })
        );
        assert_eq!(parse_plan_slash("/plan off"), Some(PlanSlash::Disable));
        assert_eq!(
            parse_plan_slash("/plan implement auth"),
            Some(PlanSlash::Enable {
                prompt: Some("implement auth".to_string())
            })
        );
        assert_eq!(
            parse_plan_slash("/plan \"ship the login flow\""),
            Some(PlanSlash::Enable {
                prompt: Some("ship the login flow".to_string())
            })
        );
        assert_eq!(parse_plan_slash("/planner"), None);
        assert_eq!(parse_plan_slash("/status"), None);
    }

    #[test]
    fn plan_mode_overlay_keeps_architect_foundation() {
        let overlay = with_plan_mode_overlay("You are a coding agent.");
        assert!(overlay.contains("read-only software architect"));
        assert!(overlay.contains("=== LIVING PLAN MODE ==="));
        assert!(overlay.contains("Do not produce the final deliverable"));
        assert_eq!(with_plan_mode_overlay(&overlay), overlay);
        let prompt = planning_turn_prompt("提供最新 Mac Studio 介绍的 html");
        assert!(prompt.contains("Do not produce the final deliverable"));
        assert!(prompt.contains("提供最新 Mac Studio 介绍的 html"));
        let goal = goal_turn_prompt("提供最新 Mac Studio 介绍的 html", 1, 3);
        assert!(goal.contains("Execute the objective now"));
        assert!(goal.contains("1/3"));
        assert!(goal.contains("提供最新 Mac Studio 介绍的 html"));
    }

    #[test]
    fn plan_mode_lifecycle_creates_and_toggles_state() {
        let dir = test_dir();
        assert!(!is_plan_mode_active(&dir));

        let plan_file = init_plan_mode_with_prompt(&dir, None).unwrap();
        assert!(plan_file.is_file());
        assert!(is_plan_mode_active(&dir));
        assert!(is_living_plan_whitelisted(&plan_file, &dir));
        assert!(is_plan_md_alias(Path::new("plan.md")));
        assert!(is_plan_md_alias(Path::new("./plan.md")));
        assert!(!is_plan_md_alias(Path::new("docs/plan.md")));
        assert_eq!(
            goal_relative_rest(Path::new("goal/plan.md")).as_deref(),
            Some(Path::new("plan.md"))
        );
        assert_eq!(
            goal_relative_rest(Path::new("./goal/state.json")).as_deref(),
            Some(Path::new("state.json"))
        );
        assert_eq!(goal_relative_rest(Path::new("plan.md")), None);

        disable_plan_mode(&dir).unwrap();
        assert!(!is_plan_mode_active(&dir));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plan_mode_prompt_seeds_living_plan_without_clobbering() {
        let dir = test_dir();
        let plan_file = init_plan_mode_with_prompt(&dir, Some("implement auth")).unwrap();
        let first = fs::read_to_string(&plan_file).unwrap();
        assert!(first.contains("- implement auth"));

        let again = init_plan_mode_with_prompt(&dir, Some("add session restore")).unwrap();
        let second = fs::read_to_string(again).unwrap();
        assert!(second.contains("- implement auth"));
        assert!(second.contains("## User Request"));
        assert!(second.contains("- add session restore"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn goal_workspace_lifecycle_and_milestones() {
        let dir = test_dir();
        let state = init_goal_workspace(&dir, "Refactor auth", 10).unwrap();
        assert_eq!(state.schema_version, 1);
        assert_eq!(state.status, GoalStatus::Running);
        assert_eq!(state.current_milestone, 1);
        assert_eq!(state.total_milestones, 3);
        assert_eq!(state.max_loops, 10);
        assert_eq!(state.milestone_step_budget, 50);

        let plan_file = dir.join("goal/plan.md");
        assert!(plan_file.is_file());
        let plan_content = fs::read_to_string(plan_file).unwrap();
        assert!(plan_content.contains("Autonomous Goal Plan: Refactor auth"));

        let verdict_pass = VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(90),
            summary: "Milestone 1 verified".to_string(),
        };

        let next = advance_goal_milestone(&dir, Some(verdict_pass)).unwrap();
        assert_eq!(next.current_milestone, 2);
        assert_eq!(next.loop_count, 1);
        assert_eq!(next.last_verifier_score, Some(90));

        let verdict_pass2 = VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(95),
            summary: "Milestone 2 verified".to_string(),
        };
        let next2 = advance_goal_milestone(&dir, Some(verdict_pass2)).unwrap();
        assert_eq!(next2.current_milestone, 3);

        let verdict_pass3 = VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(100),
            summary: "Final milestone verified".to_string(),
        };
        let final_state = advance_goal_milestone(&dir, Some(verdict_pass3)).unwrap();
        assert_eq!(final_state.status, GoalStatus::Converged);

        pause_goal(&dir).unwrap();
        let paused = load_goal_state(&dir).unwrap().unwrap();
        assert_eq!(paused.status, GoalStatus::UserPaused);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejected_verdict_does_not_advance_milestone() {
        let dir = test_dir();
        let state = init_goal_workspace(&dir, "Investigate", 2).unwrap();
        let next = advance_goal_milestone(
            &dir,
            Some(VerifierVerdict {
                outcome: VerdictOutcome::Rejected,
                score: Some(30),
                summary: "Needs more evidence".to_string(),
            }),
        )
        .unwrap();
        assert_eq!(next.current_milestone, state.current_milestone);
        assert_eq!(next.status, GoalStatus::Running);

        let failed = fail_goal(&dir).unwrap();
        assert_eq!(failed.status, GoalStatus::Failed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parse_verifier_verdict_handles_outcomes_and_scores() {
        let text = r#"
---
verdict: approved
score: 95
summary: All tests pass and architecture is clean.
---
### Summary: All tests pass and architecture is clean.
"#;
        let v = parse_verifier_verdict(text);
        assert_eq!(v.outcome, VerdictOutcome::Approved);
        assert_eq!(v.score, Some(95));

        let text_fail = "verdict: rejected\nscore: 40\nsummary: Missing regression test.";
        let vf = parse_verifier_verdict(text_fail);
        assert_eq!(vf.outcome, VerdictOutcome::Rejected);
        assert_eq!(vf.score, Some(40));
    }
}
