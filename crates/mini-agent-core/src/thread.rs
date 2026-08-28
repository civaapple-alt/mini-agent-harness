use crate::Harness;
use crate::HarnessError;
use crate::Model;
use crate::Observer;
use crate::RunControl;
use crate::RunOutcome;
use crate::SteeringMode;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStatus;
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
    InvalidInputMode(TurnInputMode),
}

impl<E: fmt::Display> fmt::Display for ThreadError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Harness(error) => error.fmt(formatter),
            Self::Busy => formatter.write_str("thread already has an active turn"),
            Self::Closed => formatter.write_str("thread is closed"),
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
}

impl<M: Model> Thread<M> {
    pub fn new(id: ThreadId, harness: Harness<M>) -> Self {
        Self {
            id,
            harness,
            status: ThreadStatus::Idle,
            next_turn_number: 1,
            last_turn_id: None,
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
        }
    }

    pub fn set_next_turn_number(&mut self, next_turn_number: u64) {
        self.next_turn_number = next_turn_number.max(1);
    }

    pub fn last_turn_id(&self) -> Option<&TurnId> {
        self.last_turn_id.as_ref()
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

    pub async fn run_turn<O: Observer + Send>(
        &mut self,
        input: TurnInput,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<TurnResult, ThreadError<M::Error>> {
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
        let outcome = self
            .harness
            .run_with_control_mode(input.text, observer, control, steering_mode)
            .await;
        match outcome {
            Ok(outcome) => {
                let status = match outcome.stop_reason {
                    crate::StopReason::Completed => TurnStatus::Completed,
                    crate::StopReason::StepLimit => TurnStatus::StepLimit,
                    crate::StopReason::Steered => TurnStatus::Steered,
                };
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
