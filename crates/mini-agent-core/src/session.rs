use mini_agent_protocol::Message;
use mini_agent_protocol::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

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

    /// Repairs a settled history left by an interrupted tool batch.
    ///
    /// A model assistant message with tool calls is only valid when every
    /// call has a matching tool message before the next conversation message.
    /// Older runtimes could persist that assistant message before steering
    /// stopped the turn, leaving the next provider request unreplayable. Drop
    /// the incomplete assistant/tool group and retain the following user
    /// input so the caller can retry from a valid boundary.
    pub(crate) fn repair_incomplete_tool_groups(&mut self) -> usize {
        let repaired = repair_tool_groups(&self.messages);
        let removed = self.messages.len().saturating_sub(repaired.len());
        if removed > 0 {
            self.replace_messages(repaired);
        }
        removed
    }
}

fn repair_tool_groups(messages: &[Message]) -> Vec<Message> {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut pending_calls = HashSet::new();
    let mut group_start = None;

    for message in messages {
        match message {
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                if group_start.is_some() {
                    repaired.truncate(group_start.take().unwrap_or(repaired.len()));
                    pending_calls.clear();
                }
                group_start = Some(repaired.len());
                pending_calls.extend(tool_calls.iter().map(|call| call.id.clone()));
                repaired.push(message.clone());
            }
            Message::Tool { call_id, .. } if group_start.is_some() => {
                if pending_calls.remove(call_id) {
                    repaired.push(message.clone());
                    if pending_calls.is_empty() {
                        group_start = None;
                    }
                } else {
                    repaired.truncate(group_start.take().unwrap_or(repaired.len()));
                    pending_calls.clear();
                }
            }
            Message::Tool { .. } => {
                repaired.push(message.clone());
            }
            _ if group_start.is_some() => {
                repaired.truncate(group_start.take().unwrap_or(repaired.len()));
                pending_calls.clear();
                repaired.push(message.clone());
            }
            _ => repaired.push(message.clone()),
        }
    }

    if let Some(start) = group_start {
        repaired.truncate(start);
    }
    repaired
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
