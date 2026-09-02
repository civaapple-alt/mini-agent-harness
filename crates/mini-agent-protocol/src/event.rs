use crate::LimitExceeded;
use crate::ModelUsage;
use crate::StopReason;
use crate::ThreadId;
use crate::ToolCall;
use crate::ToolExecutionStatus;
use crate::TurnId;
use crate::TurnInputMode;
use crate::TurnStatus;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TurnStarted {
        mode: TurnInputMode,
        prompt: String,
    },
    RunStarted {
        prompt: String,
    },
    ModelStarted {
        step: usize,
        /// Rust-only diagnostic metadata; omitted from protocol serialization.
        #[serde(skip)]
        input_bytes: usize,
        /// Digest of the complete bounded model input assembled for this round.
        #[serde(skip)]
        input_hash: String,
        /// Digest of the bounded tool manifest supplied for this round.
        #[serde(skip)]
        tool_manifest_hash: String,
    },
    AssistantReasoningDelta {
        delta: String,
    },
    AssistantTextDelta {
        delta: String,
    },
    ModelResponded {
        reasoning: String,
        text: String,
        tool_calls: Vec<ToolCall>,
        usage: Option<ModelUsage>,
    },
    ToolStarted {
        call: ToolCall,
    },
    ToolFinished {
        call_id: String,
        name: String,
        /// The original call arguments let public Item projections merge the
        /// completed result into the started ToolCall without new state.
        #[serde(skip, default)]
        arguments: Value,
        content: String,
        is_error: bool,
        truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<ToolExecutionStatus>,
    },
    ContextCompactionStarted {
        before_bytes: usize,
    },
    ContextCompactionFinished {
        before_bytes: usize,
        after_bytes: usize,
        usage: Option<ModelUsage>,
    },
    RunFinished {
        stop_reason: StopReason,
        steps: usize,
    },
    TurnFinished {
        status: TurnStatus,
    },
    RunFailed {
        reason: RunFailure,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum RunFailure {
    Model,
    Compaction,
    LimitExceeded(LimitExceeded),
}

/// An observer event with the identity and ordering metadata required by a
/// host projection or a future wire adapter.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventEnvelope {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub sequence: u64,
    pub event: Event,
}

impl EventEnvelope {
    pub fn new(thread_id: ThreadId, turn_id: Option<TurnId>, sequence: u64, event: Event) -> Self {
        Self {
            thread_id,
            turn_id,
            sequence,
            event,
        }
    }
}

pub trait Observer {
    fn observe(&mut self, event: &Event);
}

/// Receives ordered events emitted by a core Thread.
pub trait EventSink {
    fn emit(&mut self, event: EventEnvelope);
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;

impl Observer for () {
    fn observe(&mut self, _event: &Event) {}
}
