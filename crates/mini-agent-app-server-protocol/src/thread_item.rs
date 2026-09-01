use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::Message;
use mini_agent_protocol::ToolCall as ModelToolCall;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

const MAX_ITEM_TEXT_BYTES: usize = 16 * 1024;
const MAX_PROJECTED_ITEMS: usize = 256;

/// Lifecycle state for a projected ThreadItem.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
}

/// The smallest public ThreadItem projection backed by current Core events.
///
/// Items are a client-facing projection, not a second Session log. Tool items
/// reuse the model call ID so approval, event, and completion correlation stay
/// on the existing callId identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadItem {
    UserMessage {
        id: String,
        text: String,
    },
    AgentMessage {
        id: String,
        text: String,
    },
    Reasoning {
        id: String,
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        status: ItemStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    ContextCompaction {
        id: String,
        status: ItemStatus,
    },
}

impl ThreadItem {
    /// Projects one Core event into zero or more bounded public Items.
    pub fn from_event(event: &EventEnvelope) -> Vec<Self> {
        let turn_prefix = event
            .turn_id
            .as_ref()
            .map(|turn_id| turn_id.as_str().to_string())
            .unwrap_or_else(|| format!("event-{}", event.sequence));
        match &event.event {
            Event::TurnStarted { prompt, .. } => vec![Self::UserMessage {
                id: format!("{turn_prefix}:user"),
                text: bound_text(prompt),
            }],
            Event::ModelResponded {
                reasoning,
                text,
                tool_calls,
                ..
            } => {
                let mut items = Vec::with_capacity(tool_calls.len() + 2);
                if !reasoning.is_empty() {
                    items.push(Self::Reasoning {
                        id: format!("{turn_prefix}:reasoning:{}", event.sequence),
                        text: bound_text(reasoning),
                    });
                }
                if !text.is_empty() {
                    items.push(Self::AgentMessage {
                        id: format!("{turn_prefix}:agent:{}", event.sequence),
                        text: bound_text(text),
                    });
                }
                items.extend(
                    tool_calls
                        .iter()
                        .map(|call| tool_item(call, ItemStatus::InProgress, None)),
                );
                items
            }
            Event::ToolStarted { call } => {
                vec![tool_item(call, ItemStatus::InProgress, None)]
            }
            Event::ToolFinished {
                call_id,
                name,
                content,
                is_error,
                ..
            } => vec![Self::ToolCall {
                id: call_id.clone(),
                name: name.clone(),
                arguments: Value::Null,
                status: if *is_error {
                    ItemStatus::Failed
                } else {
                    ItemStatus::Completed
                },
                output: Some(bound_text(content)),
            }],
            Event::ContextCompactionStarted { .. } => vec![Self::ContextCompaction {
                id: format!("{turn_prefix}:compaction"),
                status: ItemStatus::InProgress,
            }],
            Event::ContextCompactionFinished { .. } => vec![Self::ContextCompaction {
                id: format!("{turn_prefix}:compaction"),
                status: ItemStatus::Completed,
            }],
            Event::RunStarted { .. }
            | Event::ModelStarted { .. }
            | Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::RunFinished { .. }
            | Event::TurnFinished { .. }
            | Event::RunFailed { .. } => Vec::new(),
        }
    }

    /// Projects bounded settled messages for turn/read without adding a
    /// separate persistence source.
    pub fn from_messages(messages: &[Message]) -> Vec<Self> {
        messages
            .iter()
            .take(MAX_PROJECTED_ITEMS)
            .enumerate()
            .filter_map(|(index, message)| match message {
                Message::User { text } => Some(vec![Self::UserMessage {
                    id: format!("message-{index}"),
                    text: bound_text(text),
                }]),
                Message::Assistant {
                    reasoning,
                    text,
                    tool_calls,
                } => {
                    let mut items = Vec::with_capacity(tool_calls.len() + 2);
                    if !reasoning.is_empty() {
                        items.push(Self::Reasoning {
                            id: format!("message-{index}:reasoning"),
                            text: bound_text(reasoning),
                        });
                    }
                    if !text.is_empty() {
                        items.push(Self::AgentMessage {
                            id: format!("message-{index}:agent"),
                            text: bound_text(text),
                        });
                    }
                    items.extend(
                        tool_calls
                            .iter()
                            .map(|call| tool_item(call, ItemStatus::Completed, None)),
                    );
                    Some(items)
                }
                Message::Tool {
                    call_id,
                    name,
                    content,
                    is_error,
                    ..
                } => Some(vec![Self::ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: Value::Null,
                    status: if *is_error {
                        ItemStatus::Failed
                    } else {
                        ItemStatus::Completed
                    },
                    output: Some(bound_text(content)),
                }]),
                Message::Context { .. } => None,
            })
            .flatten()
            .take(MAX_PROJECTED_ITEMS)
            .collect()
    }
}

fn tool_item(call: &ModelToolCall, status: ItemStatus, output: Option<String>) -> ThreadItem {
    ThreadItem::ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        status,
        output,
    }
}

fn bound_text(text: &str) -> String {
    if text.len() <= MAX_ITEM_TEXT_BYTES {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .take_while(|(index, _)| *index < MAX_ITEM_TEXT_BYTES - 3)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    format!("{}...", &text[..end])
}

#[cfg(test)]
#[path = "thread_item_tests.rs"]
mod tests;
