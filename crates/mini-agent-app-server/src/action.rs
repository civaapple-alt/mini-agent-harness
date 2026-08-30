use crate::AppServerError;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ActionReceipt {
    pub(super) id: ActionId,
    pub(super) sequence: ActionSequence,
    pub(super) base_revision: RuntimeRevision,
}

/// A command together with the metadata assigned by the runtime actor.
pub(super) struct ActionEnvelope<T> {
    pub(super) id: ActionId,
    pub(super) sequence: ActionSequence,
    pub(super) base_revision: RuntimeRevision,
    pub(super) command: T,
}

impl<T> ActionEnvelope<T> {
    pub(super) fn receipt(&self) -> ActionReceipt {
        ActionReceipt {
            id: self.id,
            sequence: self.sequence,
            base_revision: self.base_revision,
        }
    }
}

/// Result returned internally by the runtime actor before the public facade
/// projects away the action metadata.
pub(super) struct ActionResponse<T> {
    pub(super) value: T,
    pub(super) receipt: ActionReceipt,
}

impl<T> ActionResponse<T> {
    pub(super) fn into_value(self) -> T {
        let _receipt = self.receipt;
        self.value
    }
}

pub(super) type ActionResult<T> = Result<ActionResponse<T>, AppServerError>;

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
    ) -> ActionEnvelope<T> {
        let envelope = ActionEnvelope {
            id: ActionId(self.next_id),
            sequence: ActionSequence(self.next_sequence),
            base_revision,
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

    #[test]
    fn assigns_independent_action_identity_and_admission_order() {
        let mut sequencer = ActionSequencer::new();
        let first = sequencer.admit("first", RuntimeRevision::default());
        let second = sequencer.admit("second", RuntimeRevision::default());

        assert_eq!(first.receipt().id, first.id);
        assert_eq!(first.receipt().sequence, first.sequence);
        assert_eq!(first.receipt().base_revision, first.base_revision);
        assert_eq!(second.id.0, first.id.0 + 1);
        assert_eq!(second.sequence.0, first.sequence.0 + 1);
        assert_eq!(first.command, "first");
        assert_eq!(second.command, "second");
    }
}
