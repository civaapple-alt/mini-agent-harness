use mini_agent_protocol::Message;
use mini_agent_protocol::ModelContextSnapshot;
use mini_agent_protocol::ModelUsage;

use crate::session::context_bytes_for;
use crate::tool_batch_executor::truncate_utf8;

pub(super) const LOOP_WARNING_PREFIX: &str = "[Loop warning:";

pub(super) const COMPACTION_PREFIX: &str = "[Compacted conversation context]";

const SMALL_CONTEXT_WINDOW_MAX: u32 = 262_144;
const LARGE_CONTEXT_WINDOW_MIN: u32 = 1_000_000;
const SMALL_CONTEXT_WATERLINE_PERCENT: u64 = 80;
const LARGE_CONTEXT_WATERLINE_PERCENT: u64 = 50;

/// Applies the feedback watermark to provider usage from the request that just completed.
pub(super) fn usage_reaches_context_waterline(
    context: &ModelContextSnapshot,
    usage: ModelUsage,
) -> bool {
    let Some(window) = context.context_window_tokens.filter(|window| *window > 0) else {
        return false;
    };
    let ratio = match window {
        ..=SMALL_CONTEXT_WINDOW_MAX => SMALL_CONTEXT_WATERLINE_PERCENT,
        LARGE_CONTEXT_WINDOW_MIN.. => LARGE_CONTEXT_WATERLINE_PERCENT,
        _ => 0,
    };
    let watermark = (ratio > 0).then(|| (u64::from(window) * ratio).div_ceil(100));
    let reached_watermark = watermark.is_some_and(|tokens| usage.input_tokens >= tokens);
    let output_reserve_exceeded = context
        .max_output_tokens
        .filter(|output| *output <= window)
        .is_some_and(|output| {
            usage.input_tokens.saturating_add(u64::from(output)) > u64::from(window)
        });
    reached_watermark || output_reserve_exceeded
}

pub(super) fn compaction_prompt() -> &'static str {
    include_str!("../builtin/prompts/system/compaction.md").trim_end()
}

pub(super) fn bounded_compaction_prompt(max_user_input_bytes: usize) -> String {
    truncate_utf8(compaction_prompt().to_string(), max_user_input_bytes)
}
pub(super) const COMPACT_TAIL_GROUPS: usize = 2;
pub(super) const COMPACT_TAIL_MAX_BYTES: usize = 128 * 1024;

pub(super) fn split_compaction_parts(
    messages: &[Message],
) -> (Vec<Message>, Vec<Message>, Vec<Message>) {
    let (without_context, contexts) = take_latest_contexts(messages);
    let (prefix, tail) = split_prefix_tail(&without_context);
    (prefix, contexts, tail)
}

pub(super) fn take_latest_contexts(messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
    let mut seen = std::collections::HashSet::new();
    let mut contexts = Vec::new();
    for (index, message) in messages.iter().enumerate().rev() {
        let Message::Context { text } = message else {
            continue;
        };
        if text.starts_with(LOOP_WARNING_PREFIX) {
            continue;
        }
        let slot = context_slot(text)
            .map(str::to_string)
            .unwrap_or_else(|| format!("<untagged:{index}>"));
        if seen.insert(slot) {
            contexts.push(message.clone());
        }
    }
    contexts.reverse();
    let without_context = messages
        .iter()
        .filter(|message| {
            !matches!(message, Message::Context { text } if !text.starts_with(LOOP_WARNING_PREFIX))
        })
        .cloned()
        .collect();
    (without_context, contexts)
}

fn context_slot(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('<')?;
    let end = rest.find(['>', '/', ' ', '\t', '\n'])?;
    (end > 0).then_some(&rest[..end])
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
    contexts: Vec<Message>,
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
    compacted.extend(contexts);
    compacted.extend(tail);
    compacted
}

pub(super) fn mechanical_compact(
    mut prefix: Vec<Message>,
    contexts: Vec<Message>,
    tail: Vec<Message>,
    compact_at: usize,
    system_prompt: &str,
    tool_specs: &[mini_agent_protocol::ToolSpec],
    max_user_input_bytes: usize,
) -> Vec<Message> {
    loop {
        let compacted =
            assemble_compacted(None, contexts.clone(), tail.clone(), max_user_input_bytes);
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

#[cfg(test)]
mod tests {
    use super::take_latest_contexts;
    use super::usage_reaches_context_waterline;
    use mini_agent_protocol::{
        ContextInjectionKind, ContextInjectionRecord, Message, ModelContextSnapshot,
        ModelSelection, ModelUsage,
    };

    fn snapshot(window: u32, output: Option<u32>) -> ModelContextSnapshot {
        ModelContextSnapshot {
            selection: ModelSelection::new("provider", "model"),
            context_window_tokens: Some(window),
            max_output_tokens: output,
        }
    }

    #[test]
    fn usage_waterline_is_selected_by_configured_window_size() {
        for (window, threshold) in [
            (100_000, 80_000),
            (256_000, 204_800),
            (262_144, 209_716),
            (1_000_000, 500_000),
            (1_048_576, 524_288),
        ] {
            assert!(!usage_reaches_context_waterline(
                &snapshot(window, None),
                ModelUsage {
                    input_tokens: threshold - 1,
                    cached_input_tokens: None,
                    output_tokens: 0
                },
            ));
            assert!(usage_reaches_context_waterline(
                &snapshot(window, None),
                ModelUsage {
                    input_tokens: threshold,
                    cached_input_tokens: None,
                    output_tokens: 0
                },
            ));
        }
        assert!(!usage_reaches_context_waterline(
            &snapshot(500_000, None),
            ModelUsage {
                input_tokens: 400_000,
                cached_input_tokens: None,
                output_tokens: 0
            },
        ));
    }

    #[test]
    fn output_reserve_can_trigger_before_the_window_percentage() {
        let context = snapshot(262_144, Some(64_000));
        assert!(!usage_reaches_context_waterline(
            &context,
            ModelUsage {
                input_tokens: 198_144,
                cached_input_tokens: None,
                output_tokens: 0
            },
        ));
        assert!(usage_reaches_context_waterline(
            &context,
            ModelUsage {
                input_tokens: 198_145,
                cached_input_tokens: None,
                output_tokens: 0
            },
        ));
    }

    #[test]
    fn compaction_keeps_latest_active_injected_instruction() {
        let older = ContextInjectionRecord {
            id: "workspace_instruction_fixture".to_string(),
            kind: ContextInjectionKind::ProjectInstructions,
            source: "AGENTS.md".to_string(),
            workspace: Some("主工作区".to_string()),
            path: Some("AGENTS.md".to_string()),
            scope: "整个工作区及其子目录".to_string(),
            bytes: 5,
            fingerprint: "old".to_string(),
            supersedes: None,
            reused: false,
        };
        let mut newer = older.clone();
        newer.fingerprint = "new".to_string();
        newer.supersedes = Some("old".to_string());
        let messages = vec![
            Message::Context {
                text: older.context_message("old rule"),
            },
            Message::User {
                text: "old task".to_string(),
            },
            Message::Context {
                text: newer.context_message("new rule"),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "answer".to_string(),
                tool_calls: Vec::new(),
            },
        ];

        let (_, contexts) = take_latest_contexts(&messages);

        assert_eq!(contexts.len(), 1);
        assert!(matches!(&contexts[0], Message::Context { text } if text.contains("new rule")));
    }

    #[test]
    fn compaction_preserves_each_unscoped_context_message() {
        let messages = vec![
            Message::Context {
                text: "first unscoped context".to_string(),
            },
            Message::User {
                text: "question".to_string(),
            },
            Message::Context {
                text: "second unscoped context".to_string(),
            },
        ];

        let (_, contexts) = take_latest_contexts(&messages);

        assert_eq!(
            contexts,
            [
                Message::Context {
                    text: "first unscoped context".to_string()
                },
                Message::Context {
                    text: "second unscoped context".to_string()
                }
            ]
        );
    }
}
