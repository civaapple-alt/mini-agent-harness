use mini_agent_protocol::TurnInput;
use std::future::Future;
use std::future::poll_fn;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::task::Waker;

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
    cancel_waker: Arc<Mutex<Option<Waker>>>,
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
        if let Some(waker) = self.cancel_waker.lock().unwrap().take() {
            waker.wake();
        }
    }

    pub fn clear_cancel(&self) {
        self.cancel_requested.store(false, Ordering::Release);
    }

    /// Returns the host-local cancellation state used by blocking tools.
    ///
    /// The token is deliberately an atomic handle rather than a protocol
    /// field. A Host can use it to stop a concrete side effect while the
    /// Core turn loop is still waiting for that tool to return.
    pub fn cancellation_token(&self) -> Arc<AtomicBool> {
        self.cancel_requested.clone()
    }

    pub fn submit(&self, input: TurnInput) -> Result<(), InputQueueError> {
        self.submit_steer(input, None)
    }

    pub fn submit_steer(
        &self,
        mut input: TurnInput,
        request_id: Option<String>,
    ) -> Result<(), InputQueueError> {
        let is_steer = input.mode == mini_agent_protocol::TurnInputMode::Steer;
        input.steer_request_id = request_id;
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

    pub fn cancel_pending_steers(&self) -> Vec<TurnInput> {
        let cancelled = self.pending_inputs.cancel_steers();
        self.clear_steer();
        cancelled
    }

    pub(crate) async fn until_cancelled<F: Future>(&self, future: F) -> Result<F::Output, ()> {
        let mut future = Box::pin(future);
        poll_fn(|context| {
            if self.cancel_requested.load(Ordering::Acquire) {
                return Poll::Ready(Err(()));
            }
            *self.cancel_waker.lock().unwrap() = Some(context.waker().clone());
            if self.cancel_requested.load(Ordering::Acquire) {
                return Poll::Ready(Err(()));
            }
            future.as_mut().poll(context).map(Ok)
        })
        .await
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

    pub(crate) fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::Acquire)
    }
}
