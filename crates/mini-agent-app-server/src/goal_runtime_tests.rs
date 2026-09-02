use super::GoalRuntime;
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

#[test]
fn ignores_verifier_result_after_goal_clear() {
    let session_dir = temporary_session_dir();
    let store = HostWorkflowStore::new(&session_dir, GoalLimits::default());
    let state = store.set_goal("test stale verifier", None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let (events, _) = broadcast::channel(4);
    let mut runtime = GoalRuntime::new(store.clone(), events, None);
    runtime.pending_verification = Some(PendingVerification {
        goal_id: state.goal_id.clone(),
        turn_id: TurnId::new("turn-1"),
        checkpoint_seq: 1,
    });

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
    let session_dir = temporary_session_dir();
    let store = HostWorkflowStore::new(&session_dir, GoalLimits::default());
    let state = store.set_goal("advance a milestone", None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let (events, _) = broadcast::channel(4);
    let mut runtime = GoalRuntime::new(store, events, None);
    runtime.pending_verification = Some(PendingVerification {
        goal_id: state.goal_id.clone(),
        turn_id: TurnId::new("turn-1"),
        checkpoint_seq: 1,
    });

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
fn ignores_verifier_result_for_changed_checkpoint() {
    let session_dir = temporary_session_dir();
    let store = HostWorkflowStore::new(&session_dir, GoalLimits::default());
    let state = store.set_goal("reject stale checkpoint", None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let (events, _) = broadcast::channel(4);
    let mut runtime = GoalRuntime::new(store, events, None);
    runtime.pending_verification = Some(PendingVerification {
        goal_id: state.goal_id.clone(),
        turn_id: TurnId::new("turn-1"),
        checkpoint_seq: 1,
    });

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
    let session_dir = temporary_session_dir();
    let store = HostWorkflowStore::new(&session_dir, GoalLimits::default());
    let state = store.set_goal("prepare a verifier", None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let (events, _) = broadcast::channel(4);
    let mut runtime = GoalRuntime::new(store, events, None);

    assert!(
        runtime
            .prepare_verification(ThreadId::new("thread-1"), &state.goal_id, 1, Vec::new())
            .is_err()
    );
    std::fs::remove_dir_all(session_dir).unwrap();
}
