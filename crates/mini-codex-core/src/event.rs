use crate::LimitExceeded;
use crate::ModelUsage;
use crate::StopReason;
use crate::ToolCall;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        prompt: String,
    },
    ModelStarted {
        step: usize,
    },
    AssistantTextDelta {
        delta: String,
    },
    ModelResponded {
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
        content: String,
        is_error: bool,
        truncated: bool,
    },
    RunFinished {
        stop_reason: StopReason,
        steps: usize,
    },
    RunFailed {
        reason: RunFailure,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum RunFailure {
    Model,
    LimitExceeded(LimitExceeded),
}

pub trait Observer {
    fn observe(&mut self, event: &Event);
}

impl Observer for () {
    fn observe(&mut self, _event: &Event) {}
}
