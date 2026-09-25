use crate::ToolExecutionStatus;
use crate::ToolSpec;
use serde::Deserialize;
use serde::Serialize;

/// Stable provider/model identity selected by a Thread or Turn.
/// This carries no endpoint, credential, or provider implementation state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelSelection {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }
}

/// How a Turn chooses a model's reasoning behavior. `ApiDefault` means the
/// provider request omits the model-specific reasoning parameter. `Level`
/// carries one value declared by the selected model profile, including values
/// such as `disabled` when the model exposes them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ReasoningSelection {
    #[default]
    ApiDefault,
    Level(String),
}

impl ReasoningSelection {
    pub fn level(value: impl Into<String>) -> Self {
        Self::Level(value.into())
    }

    pub fn level_value(&self) -> Option<&str> {
        match self {
            Self::ApiDefault => None,
            Self::Level(value) => Some(value),
        }
    }
}
use serde_json::Value;
use std::error::Error;
use std::future::Future;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    Context {
        text: String,
    },
    User {
        text: String,
    },
    Assistant {
        reasoning: String,
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
        /// Structured policy/execution status; omitted when projecting to a
        /// provider request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<ToolExecutionStatus>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelResponse {
    pub reasoning: String,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<ModelUsage>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelEvent {
    ReasoningDelta(String),
    TextDelta(String),
}

pub trait ModelEventSink {
    fn emit(&mut self, event: ModelEvent);
}

#[derive(Clone, Copy)]
pub struct ModelRequest<'a> {
    pub system_prompt: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub max_response_bytes: usize,
    pub model_selection: Option<&'a ModelSelection>,
    pub reasoning_selection: Option<&'a ReasoningSelection>,
    /// Legacy reasoning parameter retained for older callers.
    pub reasoning_effort: Option<&'a str>,
}

/// A model proposes the next assistant text and tool calls.
///
/// Implementations translate the portable request into a provider protocol.
/// They do not execute tools or decide when a run is complete.
pub trait Model {
    type Error: Error + Send + Sync + 'static;

    fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> impl Future<Output = Result<ModelResponse, Self::Error>> + Send + 'a;
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
