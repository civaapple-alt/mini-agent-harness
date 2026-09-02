use super::*;
use crate::test_support::test_root as test_dir;

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
    assert_eq!(state.schema_version, 2);
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
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Rejected,
            score: Some(30),
            summary: "Needs more evidence".to_string(),
        }),
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
    advance_goal_milestone(
        &dir,
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(100),
            summary: "first".to_string(),
        }),
    )
    .unwrap();
    advance_goal_milestone(
        &dir,
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(100),
            summary: "second".to_string(),
        }),
    )
    .unwrap();
    let converged = advance_goal_milestone(
        &dir,
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Approved,
            score: Some(100),
            summary: "final".to_string(),
        }),
    )
    .unwrap();

    let unchanged = advance_goal_milestone(
        &dir,
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Rejected,
            score: Some(0),
            summary: "late result".to_string(),
        }),
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
        Some(VerifierVerdict {
            outcome: VerdictOutcome::Rejected,
            score: Some(10),
            summary: "retry exhausted".to_string(),
        }),
    )
    .unwrap();
    assert_eq!(failed.status, GoalStatus::Failed);
    assert_eq!(failed.current_milestone, 1);
    assert_eq!(failed.loop_count, 1);
    fs::remove_dir_all(dir).unwrap();
}
