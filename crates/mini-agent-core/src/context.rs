use mini_agent_protocol::Message;
use mini_agent_protocol::ToolSpec;
use serde::Deserialize;
use serde::Serialize;

/// The ordered set of conversation items that can be projected into a model
/// request.
///
/// `Context` owns conversation items inside the execution core. It deliberately
/// does not know how a session is stored on disk or how external effects are
/// replayed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Context {
    messages: Vec<Message>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn replace(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    pub fn truncate(&mut self, len: usize) {
        self.messages.truncate(len);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub(crate) fn bytes(&self, system_prompt: &str, tool_specs: &[ToolSpec]) -> usize {
        context_bytes_for(system_prompt, &self.messages, tool_specs)
    }
}

pub(crate) fn context_bytes_for(
    system_prompt: &str,
    messages: &[Message],
    tool_specs: &[ToolSpec],
) -> usize {
    system_prompt.len()
        + serde_json::to_vec(messages)
            .expect("messages must serialize")
            .len()
        + serde_json::to_vec(tool_specs)
            .expect("tool specs must serialize")
            .len()
}

pub(crate) fn model_input_digest(
    system_prompt: &str,
    messages: &[Message],
    tool_specs: &[ToolSpec],
) -> String {
    let input = serde_json::to_vec(&(system_prompt, messages, tool_specs))
        .expect("model input must serialize");
    mini_agent_protocol::stable_digest(&input)
}

pub(crate) fn tool_manifest_digest(tool_specs: &[ToolSpec]) -> String {
    let manifest = serde_json::to_vec(tool_specs).expect("tool manifest must serialize");
    mini_agent_protocol::stable_digest(&manifest)
}
