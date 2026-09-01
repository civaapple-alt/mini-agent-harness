use mini_agent_protocol::Message;

use crate::context::context_bytes_for;
use crate::tool_batch_executor::truncate_utf8;

pub(super) const LOOP_WARNING_PREFIX: &str = "[Loop warning:";

pub(super) const COMPACTION_PREFIX: &str = "[Compacted conversation context]";

pub(super) fn compaction_prompt() -> &'static str {
    include_str!("../builtin/prompts/system/compaction.md").trim_end()
}
pub(super) const COMPACT_TAIL_GROUPS: usize = 2;
pub(super) const COMPACT_TAIL_MAX_BYTES: usize = 128 * 1024;

pub(super) fn split_compaction_parts(
    messages: &[Message],
) -> (Vec<Message>, Option<Message>, Vec<Message>) {
    let (without_context, context) = take_latest_context(messages);
    let (prefix, tail) = split_prefix_tail(&without_context);
    (prefix, context, tail)
}

pub(super) fn take_latest_context(messages: &[Message]) -> (Vec<Message>, Option<Message>) {
    let Some(index) = messages.iter().rposition(|message| match message {
        Message::Context { text } => !text.starts_with(LOOP_WARNING_PREFIX),
        _ => false,
    }) else {
        return (messages.to_vec(), None);
    };
    let context = messages[index].clone();
    let mut rest = Vec::with_capacity(messages.len().saturating_sub(1));
    rest.extend_from_slice(&messages[..index]);
    rest.extend_from_slice(&messages[index + 1..]);
    (rest, Some(context))
}

pub(super) fn split_prefix_tail(messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
    let starts = assistant_starts(messages);
    if starts.is_empty() {
        return (messages.to_vec(), Vec::new());
    }
    let group_count = starts.len().min(COMPACT_TAIL_GROUPS);
    let mut tail_start = starts[starts.len() - group_count];
    let mut tail = messages[tail_start..].to_vec();
    while assistant_starts(&tail).len() > 1 && serialized_len(&tail) > COMPACT_TAIL_MAX_BYTES {
        let inner = assistant_starts(&tail);
        tail_start += inner[1];
        tail = messages[tail_start..].to_vec();
    }
    (messages[..tail_start].to_vec(), tail)
}

pub(super) fn assistant_starts(messages: &[Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            matches!(message, Message::Assistant { .. }).then_some(index)
        })
        .collect()
}

pub(super) fn serialized_len(messages: &[Message]) -> usize {
    serde_json::to_vec(messages)
        .expect("messages must serialize")
        .len()
}

pub(super) fn remove_first_message_group(messages: &mut Vec<Message>) {
    if messages.is_empty() {
        return;
    }
    match messages.remove(0) {
        Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
            while matches!(messages.first(), Some(Message::Tool { .. })) {
                messages.remove(0);
            }
        }
        _ => {}
    }
    while matches!(messages.first(), Some(Message::Tool { .. })) {
        messages.remove(0);
    }
}

pub(super) fn trim_prefix_to_fit(
    prefix: &mut Vec<Message>,
    prompt: &str,
    system_prompt: &str,
    tool_specs: &[mini_agent_protocol::ToolSpec],
    max_bytes: usize,
) {
    while !prefix.is_empty() {
        let mut request = prefix.clone();
        request.push(Message::User {
            text: prompt.to_string(),
        });
        if context_bytes_for(system_prompt, &request, tool_specs) <= max_bytes {
            return;
        }
        remove_first_message_group(prefix);
    }
}

pub(super) fn assemble_compacted(
    summary: Option<&str>,
    context: Option<Message>,
    tail: Vec<Message>,
    max_user_input_bytes: usize,
) -> Vec<Message> {
    let mut compacted = Vec::new();
    if let Some(summary) = summary {
        let full_summary = format!("{COMPACTION_PREFIX}\n{summary}");
        compacted.push(Message::User {
            text: truncate_utf8(full_summary, max_user_input_bytes),
        });
    }
    if let Some(context) = context {
        compacted.push(context);
    }
    compacted.extend(tail);
    compacted
}

pub(super) fn mechanical_compact(
    mut prefix: Vec<Message>,
    context: Option<Message>,
    tail: Vec<Message>,
    compact_at: usize,
    system_prompt: &str,
    tool_specs: &[mini_agent_protocol::ToolSpec],
    max_user_input_bytes: usize,
) -> Vec<Message> {
    loop {
        let compacted =
            assemble_compacted(None, context.clone(), tail.clone(), max_user_input_bytes);
        let mut candidate = prefix.clone();
        candidate.extend(compacted.iter().cloned());
        if prefix.is_empty()
            || context_bytes_for(system_prompt, &candidate, tool_specs) < compact_at
        {
            return candidate;
        }
        remove_first_message_group(&mut prefix);
    }
}
