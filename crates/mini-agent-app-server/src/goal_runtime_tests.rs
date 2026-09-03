use super::PendingVerification;
use mini_agent_host::GoalLimits;
use mini_agent_host::HostWorkflowStore;
use mini_agent_host::VerdictOutcome;
use mini_agent_host::VerifierVerdict;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

fn temporary_session_dir() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("mini-agent-goal-runtime-test-{suffix}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn prepared_runtime(
    objective: &str,
) -> (
    std::path::PathBuf,
    HostWorkflowStore,
    mini_agent_host::GoalState,
    super::GoalRuntimeHandle,
) {
    let session_dir = temporary_session_dir();
    let store = HostWorkflowStore::new(&session_dir, GoalLimits::default());
    let state = store.set_goal(objective, None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let (events, _) = broadcast::channel(4);
    let runtime = super::GoalRuntimeHandle::with_notifications(store.clone(), events, None, None);
    (session_dir, store, state, runtime)
}

fn set_pending_verification(
    runtime: &mut super::GoalRuntimeHandle,
    goal_id: &str,
    turn_id: &str,
    checkpoint_seq: u64,
) {
    runtime.pending_verification = Some(PendingVerification {
        goal_id: goal_id.to_string(),
        turn_id: TurnId::new(turn_id),
        checkpoint_seq,
    });
}

#[test]
fn ignores_verifier_result_after_goal_clear() {
    let (session_dir, store, state, mut runtime) = prepared_runtime("test stale verifier");
    set_pending_verification(&mut runtime, &state.goal_id, "turn-1", 1);

    assert!(store.clear_goal().unwrap());
    let result = runtime
        .complete_verification(
            &state.goal_id,
            &TurnId::new("turn-1"),
            1,
            1,
            Ok((
                "verdict: approved".to_string(),
                VerifierVerdict {
                    outcome: VerdictOutcome::Approved,
                    score: Some(100),
                    summary: "stale".to_string(),
                },
            )),
        )
        .unwrap();

    assert_eq!(result, None);
    assert!(!session_dir.join("goal").join("state.json").exists());
    std::fs::remove_dir_all(session_dir).unwrap();
}

#[test]
fn applies_approved_verdict_after_settled_checkpoint() {
    let (session_dir, _store, state, mut runtime) = prepared_runtime("advance a milestone");
    set_pending_verification(&mut runtime, &state.goal_id, "turn-1", 1);

    let next = runtime
        .complete_verification(
            &state.goal_id,
            &TurnId::new("turn-1"),
            1,
            1,
            Ok((
                "verdict: approved".to_string(),
                VerifierVerdict {
                    outcome: VerdictOutcome::Approved,
                    score: Some(100),
                    summary: "milestone complete".to_string(),
                },
            )),
        )
        .unwrap()
        .unwrap();

    assert_eq!(next.current_milestone, 2);
    assert_eq!(next.loop_count, 1);
    assert_eq!(next.active_turn_id, None);
    assert!(!next.active_turn_settled);
    std::fs::remove_dir_all(session_dir).unwrap();
}

#[test]
fn handles_rejected_and_failed_verifier_results() {
    let (session_dir, store, state, mut runtime) = prepared_runtime("retain rejected milestone");
    set_pending_verification(&mut runtime, &state.goal_id, "turn-1", 1);

    let rejected = runtime
        .complete_verification(
            &state.goal_id,
            &TurnId::new("turn-1"),
            1,
            1,
            Ok((
                "verdict: rejected\nsummary: more evidence needed".to_string(),
                VerifierVerdict {
                    outcome: VerdictOutcome::Rejected,
                    score: Some(40),
                    summary: "more evidence needed".to_string(),
                },
            )),
        )
        .unwrap()
        .unwrap();
    assert_eq!(rejected.status, mini_agent_host::GoalStatus::Running);
    assert_eq!(rejected.current_milestone, state.current_milestone);
    assert_eq!(rejected.last_verifier_score, Some(40));

    store
        .mark_goal_turn_started(&state.goal_id, "turn-2")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-2")
        .unwrap();
    set_pending_verification(&mut runtime, &state.goal_id, "turn-2", 2);
    let failed = runtime
        .complete_verification(
            &state.goal_id,
            &TurnId::new("turn-2"),
            2,
            2,
            Err("verifier provider timed out".to_string()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, mini_agent_host::GoalStatus::Failed);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("verifier provider timed out")
    );
    assert!(
        runtime
            .complete_verification(
                &state.goal_id,
                &TurnId::new("turn-2"),
                2,
                2,
                Err("late verifier result".to_string()),
            )
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(session_dir).unwrap();
}

#[test]
fn ignores_verifier_result_for_changed_checkpoint() {
    let (session_dir, _store, state, mut runtime) = prepared_runtime("reject stale checkpoint");
    set_pending_verification(&mut runtime, &state.goal_id, "turn-1", 1);

    let next = runtime
        .complete_verification(
            &state.goal_id,
            &TurnId::new("turn-1"),
            1,
            2,
            Ok((
                "verdict: approved".to_string(),
                VerifierVerdict {
                    outcome: VerdictOutcome::Approved,
                    score: Some(100),
                    summary: "stale".to_string(),
                },
            )),
        )
        .unwrap()
        .unwrap();

    assert_eq!(next.status, mini_agent_host::GoalStatus::Failed);
    assert_eq!(
        next.last_error.as_deref(),
        Some("goal verifier result was stale for the settled checkpoint")
    );
    std::fs::remove_dir_all(session_dir).unwrap();
}

#[test]
fn missing_verifier_configuration_is_a_preparation_error() {
    let (session_dir, _store, state, mut runtime) = prepared_runtime("prepare a verifier");

    assert!(
        runtime
            .prepare_verification(ThreadId::new("thread-1"), &state.goal_id, 1, Vec::new())
            .is_err()
    );
    std::fs::remove_dir_all(session_dir).unwrap();
}
