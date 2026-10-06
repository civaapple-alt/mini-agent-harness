use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

pub const DEFAULT_MAX_PENDING_INPUTS: usize = 16;
const MAX_PENDING_INPUT_BYTES: usize = 512 * 1024;
const MAX_PENDING_INPUT_ITEM_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputQueueError {
    Full { capacity: usize },
    ByteLimit { limit: usize, actual: usize },
    UnsupportedMode(TurnInputMode),
}

impl fmt::Display for InputQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { capacity } => write!(formatter, "input queue is full: {capacity}"),
            Self::ByteLimit { limit, actual } => {
                write!(
                    formatter,
                    "input queue byte limit exceeded: {actual} > {limit}"
                )
            }
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

fn retained_bytes(input: &TurnInput) -> usize {
    let skill_bytes = input
        .selected_skills
        .capacity()
        .saturating_mul(std::mem::size_of::<String>())
        .saturating_add(
            input
                .selected_skills
                .iter()
                .map(String::capacity)
                .fold(0usize, usize::saturating_add),
        );
    let model_bytes = input.model_selection.as_ref().map_or(0, |selection| {
        selection
            .provider_id
            .capacity()
            .saturating_add(selection.model_id.capacity())
    });
    let reasoning_bytes =
        input
            .reasoning_selection
            .as_ref()
            .map_or(0, |selection| match selection {
                mini_agent_protocol::ReasoningSelection::ApiDefault => 0,
                mini_agent_protocol::ReasoningSelection::Level(value) => value.capacity(),
            });
    input
        .text
        .capacity()
        .saturating_add(skill_bytes)
        .saturating_add(
            input
                .workflow
                .as_ref()
                .map_or(0, |workflow| workflow.id.capacity()),
        )
        .saturating_add(model_bytes)
        .saturating_add(reasoning_bytes)
        .saturating_add(input.reasoning_effort.as_ref().map_or(0, String::capacity))
        .saturating_add(input.steer_request_id.as_ref().map_or(0, String::capacity))
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
        let input_bytes = retained_bytes(&input);
        if input_bytes > MAX_PENDING_INPUT_ITEM_BYTES {
            return Err(InputQueueError::ByteLimit {
                limit: MAX_PENDING_INPUT_ITEM_BYTES,
                actual: input_bytes,
            });
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
        let queued_bytes = inputs
            .iter()
            .map(retained_bytes)
            .fold(0, usize::saturating_add);
        let actual = queued_bytes.saturating_add(input_bytes);
        if actual > MAX_PENDING_INPUT_BYTES {
            return Err(InputQueueError::ByteLimit {
                limit: MAX_PENDING_INPUT_BYTES,
                actual,
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

    pub fn cancel_steers(&self) -> Vec<TurnInput> {
        let Ok(mut inputs) = self.inputs.lock() else {
            return Vec::new();
        };
        let mut cancelled = Vec::new();
        inputs.retain(|pending| {
            if pending.mode == TurnInputMode::Steer {
                cancelled.push(pending.clone());
                false
            } else {
                true
            }
        });
        cancelled
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
