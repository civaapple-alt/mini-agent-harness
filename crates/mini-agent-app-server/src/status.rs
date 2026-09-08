use crate::notification::RuntimeNotification;
use mini_agent_app_server_protocol::{RuntimePhase, RuntimeStatus};
use mini_agent_protocol::{ThreadId, TurnId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub(crate) type RuntimeStatusHandle = Arc<Mutex<RuntimeStatus>>;

pub(crate) fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish(
    handle: &RuntimeStatusHandle,
    notifications: &broadcast::Sender<RuntimeNotification>,
    thread_id: ThreadId,
    phase: RuntimePhase,
    turn_id: Option<TurnId>,
    operation_id: Option<String>,
    checkpoint_seq: Option<u64>,
    revision: &AtomicU64,
    error: Option<&str>,
) {
    publish_at(
        handle,
        notifications,
        thread_id,
        phase,
        turn_id,
        operation_id,
        checkpoint_seq,
        revision.load(Ordering::SeqCst),
        error,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_at(
    handle: &RuntimeStatusHandle,
    notifications: &broadcast::Sender<RuntimeNotification>,
    thread_id: ThreadId,
    phase: RuntimePhase,
    turn_id: Option<TurnId>,
    operation_id: Option<String>,
    checkpoint_seq: Option<u64>,
    state_revision: u64,
    error: Option<&str>,
) {
    let next = RuntimeStatus {
        phase,
        thread_id,
        turn_id,
        operation_id,
        checkpoint_seq,
        state_revision,
        timestamp_ms: timestamp_ms(),
        error: error.map(|value| value.chars().take(1024).collect()),
    };
    let changed = {
        let mut current = handle.lock().unwrap();
        if *current == next {
            false
        } else {
            *current = next.clone();
            true
        }
    };
    if changed {
        let _ = notifications.send(RuntimeNotification::Status(next));
    }
}

pub(crate) fn operation(prefix: &str, value: impl std::fmt::Display) -> String {
    format!("{prefix}:{value}")
}
