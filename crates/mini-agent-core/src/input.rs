use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

pub const DEFAULT_MAX_PENDING_INPUTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputQueueError {
    Full { capacity: usize },
    UnsupportedMode(TurnInputMode),
}

impl fmt::Display for InputQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { capacity } => write!(formatter, "input queue is full: {capacity}"),
            Self::UnsupportedMode(mode) => write!(formatter, "cannot queue input mode: {mode:?}"),
        }
    }
}

impl std::error::Error for InputQueueError {}

#[derive(Clone)]
pub struct PendingInputQueue {
    inputs: Arc<Mutex<VecDeque<TurnInput>>>,
    capacity: usize,
}

impl Default for PendingInputQueue {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PENDING_INPUTS)
    }
}

impl PendingInputQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            inputs: Arc::new(Mutex::new(VecDeque::new())),
            capacity,
        }
    }

    pub fn submit(&self, input: TurnInput) -> Result<(), InputQueueError> {
        if matches!(
            input.mode,
            TurnInputMode::Start | TurnInputMode::StartIfIdle
        ) {
            return Err(InputQueueError::UnsupportedMode(input.mode));
        }
        let Ok(mut inputs) = self.inputs.lock() else {
            return Err(InputQueueError::Full {
                capacity: self.capacity,
            });
        };
        if inputs.len() >= self.capacity {
            return Err(InputQueueError::Full {
                capacity: self.capacity,
            });
        }
        inputs.push_back(input);
        Ok(())
    }

    pub fn take_steer(&self) -> Option<TurnInput> {
        self.take_matching(|input| input.mode == TurnInputMode::Steer)
    }

    pub fn take_follow_up(&self) -> Option<TurnInput> {
        self.take_matching(|input| input.mode == TurnInputMode::FollowUp)
    }

    pub fn has_steer(&self) -> bool {
        self.inputs
            .lock()
            .map(|inputs| {
                inputs
                    .iter()
                    .any(|input| input.mode == TurnInputMode::Steer)
            })
            .unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.inputs.lock().map(|inputs| inputs.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn take_matching(&self, matches: impl Fn(&TurnInput) -> bool) -> Option<TurnInput> {
        let mut inputs = self.inputs.lock().ok()?;
        let index = inputs.iter().position(matches)?;
        inputs.remove(index)
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
