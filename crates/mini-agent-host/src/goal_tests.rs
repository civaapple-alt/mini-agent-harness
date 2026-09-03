use super::*;
use crate::test_support::test_root as test_dir;

fn verdict(outcome: VerdictOutcome, score: u32, summary: &str) -> VerifierVerdict {
    VerifierVerdict {
        outcome,
        score: Some(score),
        summary: summary.to_string(),
    }
}

#[test]
fn plan_mode_overlay_keeps_architect_foundation() {
    let overlay = with_plan_mode_overlay("You are a coding agent.");
    assert!(overlay.contains("read-only software architect"));
    assert!(overlay.contains("=== LIVING PLAN MODE ==="));
    assert!(overlay.contains("Do not produce the final deliverable"));
    assert_eq!(with_plan_mode_overlay(&overlay), overlay);
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
    let state = init_goal_workspace_with_limits(
        &dir,
        "Refactor auth",
        GoalLimits {
            max_loops: 10,
            ..GoalLimits::default()
        },
    )
    .unwrap();
    assert_eq!(state.schema_version, 3);
    assert_eq!(state.objective, "Refactor auth");
    assert_eq!(state.status, GoalStatus::Running);
    assert_eq!(state.current_milestone, 1);
    assert_eq!(state.total_milestones, 3);
    assert_eq!(state.max_loops, 10);
    assert_eq!(state.milestone_step_budget, 50);

    let plan_file = dir.join("goal/plan.md");
    assert!(plan_file.is_file());
    let plan_content = fs::read_to_string(plan_file).unwrap();
    assert!(plan_content.contains("Autonomous Goal Plan: Refactor auth"));

    let verdict_pass = verdict(VerdictOutcome::Approved, 90, "Milestone 1 verified");

    let next = advance_goal_milestone(&dir, Some(verdict_pass)).unwrap();
    assert_eq!(next.current_milestone, 2);
    assert_eq!(next.loop_count, 1);
    assert_eq!(next.last_verifier_score, Some(90));

    let verdict_pass2 = verdict(VerdictOutcome::Approved, 95, "Milestone 2 verified");
    let next2 = advance_goal_milestone(&dir, Some(verdict_pass2)).unwrap();
    assert_eq!(next2.current_milestone, 3);

    let verdict_pass3 = verdict(VerdictOutcome::Approved, 100, "Final milestone verified");
    let final_state = advance_goal_milestone(&dir, Some(verdict_pass3)).unwrap();
    assert_eq!(final_state.status, GoalStatus::Converged);

    pause_goal(&dir).unwrap();
    let paused = load_goal_state(&dir).unwrap().unwrap();
    assert_eq!(paused.status, GoalStatus::UserPaused);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn goal_objective_is_bounded_at_creation() {
    let dir = test_dir();
    let objective = "x".repeat(MAX_GOAL_OBJECTIVE_BYTES + 1);
    let error = init_goal_workspace_with_limits(&dir, &objective, GoalLimits::default())
        .expect_err("oversized objective should be rejected");
    assert!(error.to_string().contains("goal objective exceeds"));
    assert!(!dir.join("goal").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_verdict_does_not_advance_milestone() {
    let dir = test_dir();
    let state = init_goal_workspace_with_limits(
        &dir,
        "Investigate",
        GoalLimits {
            max_loops: 2,
            ..GoalLimits::default()
        },
    )
    .unwrap();
    let next = advance_goal_milestone(
        &dir,
        Some(verdict(VerdictOutcome::Rejected, 30, "Needs more evidence")),
    )
    .unwrap();
    assert_eq!(next.current_milestone, state.current_milestone);
    assert_eq!(next.status, GoalStatus::Running);

    let failed = fail_goal_with_reason(&dir, None).unwrap();
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

    let invalid = parse_verifier_verdict("The evidence is incomplete.");
    assert_eq!(invalid.outcome, VerdictOutcome::Invalid);
}

#[test]
fn terminal_goal_state_does_not_advance_again() {
    let dir = test_dir();
    init_goal_workspace_with_limits(
        &dir,
        "Finish",
        GoalLimits {
            max_loops: 5,
            ..GoalLimits::default()
        },
    )
    .unwrap();
    advance_goal_milestone(&dir, Some(verdict(VerdictOutcome::Approved, 100, "first"))).unwrap();
    advance_goal_milestone(&dir, Some(verdict(VerdictOutcome::Approved, 100, "second"))).unwrap();
    let converged =
        advance_goal_milestone(&dir, Some(verdict(VerdictOutcome::Approved, 100, "final")))
            .unwrap();

    let unchanged = advance_goal_milestone(
        &dir,
        Some(verdict(VerdictOutcome::Rejected, 0, "late result")),
    )
    .unwrap();
    assert_eq!(unchanged, converged);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_verdict_exhausts_retry_budget_without_advancing() {
    let dir = test_dir();
    init_goal_workspace_with_limits(
        &dir,
        "Exhaust",
        GoalLimits {
            max_loops: 1,
            ..GoalLimits::default()
        },
    )
    .unwrap();
    let failed = advance_goal_milestone(
        &dir,
        Some(verdict(VerdictOutcome::Rejected, 10, "retry exhausted")),
    )
    .unwrap();
    assert_eq!(failed.status, GoalStatus::Failed);
    assert_eq!(failed.current_milestone, 1);
    assert_eq!(failed.loop_count, 1);
    fs::remove_dir_all(dir).unwrap();
}
