use crate::Context;
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
    context: Context,
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
            context: Context::from_messages(messages),
            context_revision: 0,
        }
    }

    pub fn messages(&self) -> &[Message] {
        self.context.messages()
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.context.replace(messages);
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub fn truncate_messages(&mut self, len: usize) {
        if len < self.context.messages().len() {
            self.context.truncate(len);
            self.context_revision = self.context_revision.saturating_add(1);
        }
    }

    pub fn clear(&mut self) {
        self.context.clear();
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub fn push(&mut self, message: Message) {
        self.context.push(message);
        self.context_revision = self.context_revision.saturating_add(1);
    }

    pub(crate) fn context_bytes(&self, system_prompt: &str, tool_specs: &[ToolSpec]) -> usize {
        self.context.bytes(system_prompt, tool_specs)
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
