use mini_agent_protocol::TurnInput;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use crate::input::InputQueueError;
use crate::input::PendingInputQueue;

/// Determines when a steering message is applied to a running turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringMode {
    StopAtCheckpoint,
    ContinueSameTurn,
}

/// Cooperative control for a running turn.
///
/// A steering request is observed only at safe boundaries between model
/// steps and after a complete tool batch, so tool side effects are never
/// interrupted halfway through.
#[derive(Clone, Default)]
pub struct RunControl {
    steer_requested: Arc<AtomicBool>,
    cancel_requested: Arc<AtomicBool>,
    pending_inputs: PendingInputQueue,
}

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request_steer(&self) {
        self.steer_requested.store(true, Ordering::Release);
    }

    /// Requests cancellation at the next safe boundary of the active turn.
    pub fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::Release);
    }

    pub fn clear_cancel(&self) {
        self.cancel_requested.store(false, Ordering::Release);
    }

    pub fn submit(&self, input: TurnInput) -> Result<(), InputQueueError> {
        let is_steer = input.mode == mini_agent_protocol::TurnInputMode::Steer;
        self.pending_inputs.submit(input)?;
        if is_steer {
            self.request_steer();
        }
        Ok(())
    }

    pub fn take_steer_input(&self) -> Option<TurnInput> {
        let input = self.pending_inputs.take_steer();
        if input.is_none() || !self.pending_inputs.has_steer() {
            self.clear_steer();
        }
        input
    }

    pub fn take_follow_up_input(&self) -> Option<TurnInput> {
        self.pending_inputs.take_follow_up()
    }

    pub fn pending_input_count(&self) -> usize {
        self.pending_inputs.len()
    }

    pub fn clear_steer(&self) {
        self.steer_requested.store(false, Ordering::Release);
    }

    pub(super) fn is_steer_requested(&self) -> bool {
        self.steer_requested.load(Ordering::Acquire)
    }

    pub(super) fn take_cancel_requested(&self) -> bool {
        self.cancel_requested.swap(false, Ordering::AcqRel)
    }
}
