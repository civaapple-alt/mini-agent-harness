use crate::AppServerError;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Identifies one command admitted by the App Server runtime actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ActionId(u64);

/// Records the order in which the runtime actor admitted commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ActionSequence(u64);

/// Identifies the runtime state version observed by an action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RuntimeRevision(u64);

impl RuntimeRevision {
    pub(super) fn value(self) -> u64 {
        self.0
    }

    pub(super) fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl From<u64> for RuntimeRevision {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Metadata attached to the result of an admitted action.
#[derive(Clone, Debug)]
pub(crate) struct ActionReceipt {
    pub(crate) id: ActionId,
    pub(crate) sequence: ActionSequence,
    pub(crate) runtime_revision: Arc<AtomicU64>,
}

/// A command together with the metadata assigned by the runtime actor.
pub(super) struct ActionEnvelope<T> {
    pub(super) id: ActionId,
    pub(super) sequence: ActionSequence,
    pub(super) base_revision: RuntimeRevision,
    pub(super) runtime_revision: Arc<AtomicU64>,
    pub(super) command: T,
}

impl<T> ActionEnvelope<T> {
    pub(super) fn receipt(&self) -> ActionReceipt {
        ActionReceipt {
            id: self.id,
            sequence: self.sequence,
            runtime_revision: self.runtime_revision.clone(),
        }
    }
}

/// Result returned internally by the runtime actor before the public facade
/// projects away the action metadata.
pub(crate) struct ActionResponse<T> {
    pub(crate) value: T,
    pub(crate) receipt: ActionReceipt,
    pub(crate) state_revision: RuntimeRevision,
}

impl<T> ActionResponse<T> {
    pub(super) fn into_value(self) -> T {
        let _receipt = self.receipt;
        self.value
    }

    pub(crate) fn into_protocol(self) -> mini_agent_app_server_protocol::ActionResult<T> {
        mini_agent_app_server_protocol::ActionResult {
            value: self.value,
            action_id: self.receipt.id.0,
            action_sequence: self.receipt.sequence.0,
            state_revision: self.state_revision.value(),
        }
    }

    pub(crate) fn metadata(&self) -> mini_agent_app_server_protocol::ActionMetadata {
        mini_agent_app_server_protocol::ActionMetadata {
            action_id: self.receipt.id.0,
            action_sequence: self.receipt.sequence.0,
            state_revision: self.state_revision.value(),
        }
    }

    pub(crate) fn map_value<U>(self, value: U) -> ActionResponse<U> {
        ActionResponse {
            value,
            receipt: self.receipt,
            state_revision: self.state_revision,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ActionFailure {
    pub(crate) error: AppServerError,
    pub(crate) receipt: Option<ActionReceipt>,
    pub(crate) state_revision: Option<RuntimeRevision>,
}

impl ActionFailure {
    pub(crate) fn without_receipt(error: AppServerError) -> Self {
        Self {
            error,
            receipt: None,
            state_revision: None,
        }
    }

    pub(crate) fn into_error(self) -> AppServerError {
        self.error
    }

    pub(crate) fn metadata(&self) -> Option<mini_agent_app_server_protocol::ActionMetadata> {
        let receipt = self.receipt.as_ref()?;
        Some(mini_agent_app_server_protocol::ActionMetadata {
            action_id: receipt.id.0,
            action_sequence: receipt.sequence.0,
            state_revision: self.state_revision?.value(),
        })
    }
}

pub(super) type ActionResult<T> = Result<ActionResponse<T>, ActionFailure>;

impl ActionReceipt {
    pub(super) fn current_revision(&self) -> RuntimeRevision {
        self.runtime_revision.load(Ordering::SeqCst).into()
    }
}

pub(super) struct ActionSequencer {
    next_id: u64,
    next_sequence: u64,
}

impl ActionSequencer {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            next_sequence: 1,
        }
    }

    /// Assigns identity and server-admission order to the next queued command.
    pub(super) fn admit<T>(
        &mut self,
        command: T,
        base_revision: RuntimeRevision,
        runtime_revision: Arc<AtomicU64>,
    ) -> ActionEnvelope<T> {
        let envelope = ActionEnvelope {
            id: ActionId(self.next_id),
            sequence: ActionSequence(self.next_sequence),
            base_revision,
            runtime_revision,
            command,
        };
        self.next_id = self.next_id.saturating_add(1);
        self.next_sequence = self.next_sequence.saturating_add(1);
        envelope
    }
}

#[cfg(test)]
mod tests {
    use super::ActionSequencer;
    use super::RuntimeRevision;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn assigns_independent_action_identity_and_admission_order() {
        let mut sequencer = ActionSequencer::new();
        let runtime_revision = Arc::new(AtomicU64::new(0));
        let first = sequencer.admit(
            "first",
            RuntimeRevision::default(),
            runtime_revision.clone(),
        );
        let second = sequencer.admit("second", RuntimeRevision::default(), runtime_revision);

        assert_eq!(first.receipt().id, first.id);
        assert_eq!(first.receipt().sequence, first.sequence);
        assert_eq!(
            first.receipt().current_revision(),
            RuntimeRevision::default()
        );
        assert_eq!(second.id.0, first.id.0 + 1);
        assert_eq!(second.sequence.0, first.sequence.0 + 1);
        assert_eq!(first.command, "first");
        assert_eq!(second.command, "second");
    }
}
