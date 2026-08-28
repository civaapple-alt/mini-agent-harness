use crate::Harness;
use crate::HarnessError;
use crate::Model;
use crate::Observer;
use crate::RunControl;
use crate::RunOutcome;
use crate::SteeringMode;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStatus;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStatus;
use std::error::Error;
use std::fmt;
use std::ops::Deref;
use std::ops::DerefMut;

#[derive(Debug)]
pub enum ThreadError<E> {
    Harness(HarnessError<E>),
    Busy,
    Closed,
    NoActiveTurn,
    TurnNotActive(TurnId),
    InvalidInputMode(TurnInputMode),
}

impl<E: fmt::Display> fmt::Display for ThreadError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Harness(error) => error.fmt(formatter),
            Self::Busy => formatter.write_str("thread already has an active turn"),
            Self::Closed => formatter.write_str("thread is closed"),
            Self::NoActiveTurn => formatter.write_str("thread has no active turn"),
            Self::TurnNotActive(turn_id) => {
                write!(formatter, "turn {} is not active", turn_id.as_str())
            }
            Self::InvalidInputMode(mode) => write!(formatter, "cannot start turn with {mode:?}"),
        }
    }
}

impl<E: Error + 'static> Error for ThreadError<E> {}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnResult {
    pub id: TurnId,
    pub status: TurnStatus,
    pub outcome: RunOutcome,
}

pub struct Thread<M> {
    id: ThreadId,
    harness: Harness<M>,
    status: ThreadStatus,
    next_turn_number: u64,
    last_turn_id: Option<TurnId>,
    next_event_sequence: u64,
}

impl<M: Model> Thread<M> {
    pub fn new(id: ThreadId, harness: Harness<M>) -> Self {
        Self {
            id,
            harness,
            status: ThreadStatus::Idle,
            next_turn_number: 1,
            last_turn_id: None,
            next_event_sequence: 1,
        }
    }

    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    pub fn status(&self) -> ThreadStatus {
        self.status
    }

    pub fn set_id(&mut self, id: ThreadId) {
        if self.id != id {
            self.id = id;
            self.next_turn_number = 1;
            self.last_turn_id = None;
            self.next_event_sequence = 1;
        }
    }

    pub fn set_next_turn_number(&mut self, next_turn_number: u64) {
        self.next_turn_number = next_turn_number.max(1);
    }

    pub fn last_turn_id(&self) -> Option<&TurnId> {
        self.last_turn_id.as_ref()
    }

    pub fn next_event_sequence(&self) -> u64 {
        self.next_event_sequence
    }

    pub fn harness(&self) -> &Harness<M> {
        &self.harness
    }

    pub fn harness_mut(&mut self) -> &mut Harness<M> {
        &mut self.harness
    }

    pub fn close(&mut self) -> Result<(), ThreadError<M::Error>> {
        if self.status == ThreadStatus::Running {
            return Err(ThreadError::Busy);
        }
        self.status = ThreadStatus::Closed;
        Ok(())
    }

    pub fn cancel_turn(
        &self,
        cancel: TurnCancel,
        control: &RunControl,
    ) -> Result<(), ThreadError<M::Error>> {
        if self.status == ThreadStatus::Closed {
            return Err(ThreadError::Closed);
        }
        if self.status != ThreadStatus::Running {
            return Err(ThreadError::NoActiveTurn);
        }
        if self.last_turn_id.as_ref() != Some(&cancel.turn_id) {
            return Err(ThreadError::TurnNotActive(cancel.turn_id));
        }
        control.request_cancel();
        Ok(())
    }

    pub async fn run_turn<O: Observer + Send>(
        &mut self,
        input: TurnInput,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<TurnResult, ThreadError<M::Error>> {
        let id = self.begin_turn(&input)?;
        observer.observe(&Event::TurnStarted {
            mode: input.mode,
            prompt: input.text.clone(),
        });
        let outcome = self
            .harness
            .run_with_control_mode(input.text, observer, control, steering_mode)
            .await;
        observer.observe(&Event::TurnFinished {
            status: outcome
                .as_ref()
                .map(status_for_outcome)
                .unwrap_or(TurnStatus::Failed),
        });
        self.finish_turn(id, outcome)
    }

    pub async fn run_turn_with_events<S: EventSink + Send>(
        &mut self,
        input: TurnInput,
        sink: &mut S,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<TurnResult, ThreadError<M::Error>> {
        let id = self.begin_turn(&input)?;
        let mut observer = EnvelopeObserver {
            sink,
            thread_id: self.id.clone(),
            turn_id: id.clone(),
            next_sequence: self.next_event_sequence,
        };
        observer.observe(&Event::TurnStarted {
            mode: input.mode,
            prompt: input.text.clone(),
        });
        let outcome = self
            .harness
            .run_with_control_mode(input.text, &mut observer, control, steering_mode)
            .await;
        observer.observe(&Event::TurnFinished {
            status: outcome
                .as_ref()
                .map(status_for_outcome)
                .unwrap_or(TurnStatus::Failed),
        });
        self.next_event_sequence = observer.next_sequence;
        self.finish_turn(id, outcome)
    }

    pub async fn run_turn_outcome<O: Observer + Send>(
        &mut self,
        input: TurnInput,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        self.run_turn(input, observer, control, steering_mode)
            .await
            .map(|result| result.outcome)
            .map_err(|error| match error {
                ThreadError::Harness(error) => error,
                other => HarnessError::Compaction(other.to_string()),
            })
    }

    pub async fn run_turn_with_events_outcome<S: EventSink + Send>(
        &mut self,
        input: TurnInput,
        sink: &mut S,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        self.run_turn_with_events(input, sink, control, steering_mode)
            .await
            .map(|result| result.outcome)
            .map_err(|error| match error {
                ThreadError::Harness(error) => error,
                other => HarnessError::Compaction(other.to_string()),
            })
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<TurnId, ThreadError<M::Error>> {
        if self.status == ThreadStatus::Closed {
            return Err(ThreadError::Closed);
        }
        if self.status == ThreadStatus::Running {
            return Err(ThreadError::Busy);
        }
        if !matches!(
            input.mode,
            TurnInputMode::Start | TurnInputMode::StartIfIdle
        ) {
            return Err(ThreadError::InvalidInputMode(input.mode));
        }

        let id = TurnId::new(format!("turn-{}", self.next_turn_number));
        self.next_turn_number = self.next_turn_number.saturating_add(1);
        self.last_turn_id = Some(id.clone());
        self.status = ThreadStatus::Running;
        Ok(id)
    }

    fn finish_turn(
        &mut self,
        id: TurnId,
        outcome: Result<RunOutcome, HarnessError<M::Error>>,
    ) -> Result<TurnResult, ThreadError<M::Error>> {
        match outcome {
            Ok(outcome) => {
                let status = status_for_outcome(&outcome);
                self.status = ThreadStatus::Idle;
                Ok(TurnResult {
                    id,
                    status,
                    outcome,
                })
            }
            Err(error) => {
                self.status = ThreadStatus::Failed;
                Err(ThreadError::Harness(error))
            }
        }
    }
}

fn status_for_outcome(outcome: &RunOutcome) -> TurnStatus {
    match outcome.stop_reason {
        crate::StopReason::Completed => TurnStatus::Completed,
        crate::StopReason::StepLimit => TurnStatus::StepLimit,
        crate::StopReason::Steered => TurnStatus::Steered,
        crate::StopReason::Cancelled => TurnStatus::Cancelled,
    }
}

struct EnvelopeObserver<'a, S> {
    sink: &'a mut S,
    thread_id: ThreadId,
    turn_id: TurnId,
    next_sequence: u64,
}

impl<S: EventSink> Observer for EnvelopeObserver<'_, S> {
    fn observe(&mut self, event: &Event) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.sink.emit(EventEnvelope::new(
            self.thread_id.clone(),
            Some(self.turn_id.clone()),
            sequence,
            event.clone(),
        ));
    }
}

impl<M> Deref for Thread<M> {
    type Target = Harness<M>;

    fn deref(&self) -> &Self::Target {
        &self.harness
    }
}

impl<M> DerefMut for Thread<M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.harness
    }
}

#[cfg(test)]
#[path = "thread_tests.rs"]
mod tests;
