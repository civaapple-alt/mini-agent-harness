use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::error::Error;
use std::future::Future;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
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
    TextDelta(String),
}

pub trait ModelEventSink {
    fn emit(&mut self, event: ModelEvent);
}

pub struct ModelRequest<'a> {
    pub system_prompt: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [crate::ToolSpec],
    pub max_response_bytes: usize,
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
