use mini_agent_protocol::ContextByteBreakdown;
use mini_agent_protocol::ContextInjectionRecord;
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

    /// Appends a context snapshot only when the latest snapshot for its slot
    /// differs. Earlier messages remain byte-for-byte stable for API caches.
    pub fn append_context_slot_if_changed(&mut self, slot: &str, text: String) -> bool {
        let prefix = format!("<{slot}");
        if self
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Context { text: current } if context_slot_matches(current, &prefix) => {
                    Some(current == &text)
                }
                _ => None,
            })
            == Some(true)
        {
            return false;
        }
        self.push(Message::Context { text });
        true
    }

    /// Appends a host-injected source without rewriting earlier model input.
    /// The returned record contains the supersedes relationship derived from
    /// the active source inventory, and identical fingerprints are omitted.
    pub fn append_context_injection(
        &mut self,
        text: String,
        mut record: ContextInjectionRecord,
    ) -> Option<ContextInjectionRecord> {
        let slot_prefix = format!("<{}", record.id);
        let fingerprint_marker = format!("fingerprint=\"{}\"", record.fingerprint);
        if !text.strip_prefix(&slot_prefix).is_some_and(|tail| {
            (tail.starts_with('>') || tail.starts_with(char::is_whitespace))
                && tail.contains(&fingerprint_marker)
        }) {
            return None;
        }
        if let Some(previous) = context_injections_from_messages(&self.messages)
            .iter()
            .find(|previous| previous.id == record.id)
            .cloned()
        {
            if previous.fingerprint == record.fingerprint {
                return None;
            }
            record.supersedes = Some(previous.fingerprint.clone());
        }
        let text = record.replace_context_message_metadata(&text)?;
        self.messages.push(Message::Context { text });
        self.context_revision = self.context_revision.saturating_add(1);
        Some(record)
    }

    pub fn context_injections(&self) -> Vec<ContextInjectionRecord> {
        context_injections_from_messages(&self.messages)
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

fn context_slot_matches(text: &str, prefix: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|rest| {
        rest.starts_with('>') || rest.chars().next().is_some_and(char::is_whitespace)
    })
}

fn context_injections_from_messages(messages: &[Message]) -> Vec<ContextInjectionRecord> {
    let mut injections = Vec::new();
    for message in messages {
        let Message::Context { text } = message else {
            continue;
        };
        let Some(record) = ContextInjectionRecord::from_context_message(text) else {
            continue;
        };
        injections.retain(|current: &ContextInjectionRecord| current.id != record.id);
        injections.push(record);
    }
    injections
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

pub(crate) fn context_byte_breakdown_for(
    system_prompt: &str,
    messages: &[Message],
    tool_specs: &[ToolSpec],
) -> ContextByteBreakdown {
    let mut breakdown = ContextByteBreakdown {
        system_prompt: system_prompt.len() as u64,
        tools: serde_json::to_vec(tool_specs)
            .expect("tool specs must serialize")
            .len() as u64,
        ..ContextByteBreakdown::default()
    };
    for message in messages {
        let bytes = serde_json::to_vec(message)
            .expect("messages must serialize")
            .len() as u64;
        match message {
            Message::Context { text }
                if context_slot_matches(text, "<activated_skills")
                    || context_slot_matches(text, "<available_extensions")
                    || text.starts_with("<skill_definition_") =>
            {
                breakdown.skills = breakdown.skills.saturating_add(bytes);
            }
            Message::Context { text }
                if context_slot_matches(text, "<world_state")
                    || context_slot_matches(text, "<session_capabilities") =>
            {
                breakdown.workspace_state = breakdown.workspace_state.saturating_add(bytes);
            }
            Message::Context { text } if text.starts_with("<workspace_instruction_") => {
                breakdown.project_instructions =
                    breakdown.project_instructions.saturating_add(bytes);
            }
            Message::Context { .. } => {
                breakdown.other = breakdown.other.saturating_add(bytes);
            }
            _ => breakdown.conversation = breakdown.conversation.saturating_add(bytes),
        }
    }
    breakdown
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
