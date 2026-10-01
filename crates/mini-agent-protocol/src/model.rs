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
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: u64,
}

/// Host-observed latency for one provider response.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTiming {
    /// Time from request dispatch to the first non-empty streamed output.
    /// This is absent when the adapter did not expose streamed output.
    pub ttft_ms: Option<u64>,
    /// Time from request dispatch until the response completed.
    pub response_ms: u64,
}

/// Byte counts for the model input, grouped for approximate token attribution.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextByteBreakdown {
    pub system_prompt: u64,
    pub project_instructions: u64,
    pub skills: u64,
    pub workspace_state: u64,
    pub conversation: u64,
    pub tools: u64,
    pub other: u64,
}

/// Bounded metadata describing host-owned context added to a model request.
/// Source content stays in model input and is never copied into this record.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInjectionRecord {
    pub id: String,
    pub kind: ContextInjectionKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub scope: String,
    pub bytes: u64,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reused: bool,
}

impl ContextInjectionRecord {
    /// Encodes source metadata alongside a model-visible context body. The
    /// metadata is safe for client projection; the body remains in the
    /// Session message only.
    pub fn context_message(&self, body: &str) -> String {
        let metadata =
            serde_json::to_string(self).expect("context injection metadata must serialize");
        format!(
            "<{id} fingerprint=\"{fingerprint}\">\n<context_injection_metadata>{metadata}</context_injection_metadata>\n{body}\n</{id}>",
            id = self.id,
            fingerprint = self.fingerprint,
        )
    }

    /// Rewrites only the structured metadata header of a context message.
    pub fn replace_context_message_metadata(&self, text: &str) -> Option<String> {
        let (opening, rest) = text.split_once('\n')?;
        if !opening.starts_with(&format!("<{} ", self.id))
            && !opening.starts_with(&format!("<{}>", self.id))
        {
            return None;
        }
        let (_, suffix) = rest.split_once("</context_injection_metadata>")?;
        let metadata = serde_json::to_string(self).ok()?;
        Some(format!(
            "{opening}\n<context_injection_metadata>{metadata}</context_injection_metadata>{suffix}"
        ))
    }

    /// Reads only the bounded metadata header from a persisted Host context
    /// message. It never returns or copies the source body.
    pub fn from_context_message(text: &str) -> Option<Self> {
        let (opening, rest) = text.split_once('\n')?;
        let slot = opening.strip_prefix('<')?.split([' ', '>', '\t']).next()?;
        let fingerprint = opening.split_once("fingerprint=\"")?.1.split_once('\"')?.0;
        let metadata = rest
            .strip_prefix("<context_injection_metadata>")?
            .split_once("</context_injection_metadata>")?
            .0;
        let record: Self = serde_json::from_str(metadata).ok()?;
        (record.id == slot && record.fingerprint == fingerprint).then_some(record)
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextInjectionKind {
    ProjectInstructions,
    Skill,
    WorkspaceState,
    Other,
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
    /// Optional Host-selected tool allowlist. Tool definitions remain complete
    /// and stable; adapters may enforce this natively or receive a prompt hint.
    pub allowed_tools: Option<&'a [String]>,
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

    /// Whether this adapter can enforce `ModelRequest::allowed_tools` without
    /// changing the stable tool definitions or prompt content.
    fn supports_allowed_tools(&self) -> bool {
        false
    }

    fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> impl Future<Output = Result<ModelResponse, Self::Error>> + Send + 'a;
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
