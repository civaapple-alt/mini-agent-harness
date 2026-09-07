use mini_agent_protocol::Message;
use mini_agent_protocol::ToolSpec;
use serde::Deserialize;
use serde::Serialize;

/// Storage-neutral conversation state owned by the execution core.
///
/// Hosts may serialize checkpoints or append JSONL records, but the runtime
/// only exchanges this value and never opens files or replays external effects.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SessionState {
    messages: Vec<Message>,
    context_revision: u64,
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            context_revision: 0,
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub fn truncate_messages(&mut self, len: usize) {
        if len < self.messages.len() {
            self.messages.truncate(len);
            self.context_revision = self.context_revision.saturating_add(1);
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub(crate) fn context_bytes(&self, system_prompt: &str, tool_specs: &[ToolSpec]) -> usize {
        context_bytes_for(system_prompt, &self.messages, tool_specs)
    }

    pub fn context_revision(&self) -> u64 {
        self.context_revision
    }

    /// Restores the host-visible revision associated with a serialized
    /// checkpoint after its messages have been validated.
    pub fn with_context_revision(mut self, revision: u64) -> Self {
        self.context_revision = revision;
        self
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
