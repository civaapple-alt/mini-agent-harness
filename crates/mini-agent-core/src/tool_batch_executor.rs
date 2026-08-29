use mini_agent_protocol::Event;
use mini_agent_protocol::Observer;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolExecutionRequest;

use crate::SessionState;
use crate::ToolRegistry;

/// Executes one complete tool batch and records its bounded outputs.
pub(super) fn execute_tool_batch<O: Observer>(
    tools: &ToolRegistry,
    calls: Vec<ToolCall>,
    max_output_bytes: usize,
    session: &mut SessionState,
    observer: &mut O,
) -> Vec<(String, serde_json::Value, String)> {
    let mut executed = Vec::with_capacity(calls.len());
    for call in calls {
        observer.observe(&Event::ToolStarted { call: call.clone() });
        let request = ToolExecutionRequest::from(call.clone());
        let outcome = tools.execute_outcome(&request);
        let is_error = outcome.status.is_error();
        let content = outcome.content.clone();
        let truncated = content.len() > max_output_bytes;
        let content = truncate_utf8(content, max_output_bytes);

        observer.observe(&Event::ToolFinished {
            call_id: call.id.clone(),
            name: call.name.clone(),
            content: content.clone(),
            is_error,
            truncated,
            outcome: Some(outcome.status),
        });
        session.push(mini_agent_protocol::Message::Tool {
            call_id: call.id,
            name: call.name.clone(),
            content: content.clone(),
            is_error,
            outcome: Some(outcome.status),
        });
        executed.push((call.name, call.arguments, content));
    }
    executed
}

pub(super) fn truncate_utf8(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }

    const MARKER: &str = "\n[truncated]";
    if max_bytes <= MARKER.len() {
        content.truncate(floor_char_boundary(&content, max_bytes));
        return content;
    }

    let retained_bytes = max_bytes - MARKER.len();
    let head_bytes = retained_bytes.div_ceil(2);
    let tail_bytes = retained_bytes - head_bytes;
    let head_end = floor_char_boundary(&content, head_bytes);
    let tail_start = ceil_char_boundary(&content, content.len() - tail_bytes);
    let mut output = String::with_capacity(max_bytes);
    output.push_str(&content[..head_end]);
    output.push_str(MARKER);
    output.push_str(&content[tail_start..]);
    output
}

pub(super) fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}
