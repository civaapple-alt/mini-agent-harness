use mini_codex_core::Tool;
use mini_codex_core::ToolError;
use mini_codex_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

const MAX_RESULTS: usize = 8;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const MAX_READ_BYTES: usize = 16 * 1024;
const DEFAULT_READ_BYTES: usize = 8 * 1024;
const PREVIEW_BYTES: usize = 4 * 1024;

#[derive(Clone, Default)]
pub struct ResultStore(Arc<Mutex<StoreState>>);

#[derive(Default)]
struct StoreState {
    next_id: u64,
    total_bytes: usize,
    entries: VecDeque<StoredEntry>,
}

struct StoredEntry {
    handle: String,
    content: String,
    source_bytes: usize,
    source_truncated: bool,
}

pub struct StoredResult {
    pub handle: String,
    pub preview: String,
    pub stored_bytes: usize,
    pub source_bytes: usize,
    pub source_truncated: bool,
}

impl ResultStore {
    pub fn store(
        &self,
        content: String,
        source_bytes: usize,
        source_truncated: bool,
    ) -> StoredResult {
        let content = retain_head_and_tail(content, MAX_RESULT_BYTES);
        let preview = retain_head_and_tail(content.clone(), PREVIEW_BYTES);
        let mut state = self.0.lock().unwrap();
        state.next_id = state.next_id.saturating_add(1);
        let handle = format!("result-{}", state.next_id);
        while state.entries.len() >= MAX_RESULTS
            || state.total_bytes.saturating_add(content.len()) > MAX_TOTAL_BYTES
        {
            let Some(removed) = state.entries.pop_front() else {
                break;
            };
            state.total_bytes = state.total_bytes.saturating_sub(removed.content.len());
        }
        state.total_bytes = state.total_bytes.saturating_add(content.len());
        let stored_bytes = content.len();
        state.entries.push_back(StoredEntry {
            handle: handle.clone(),
            content,
            source_bytes,
            source_truncated,
        });
        StoredResult {
            handle,
            preview,
            stored_bytes,
            source_bytes,
            source_truncated,
        }
    }

    fn read(
        &self,
        handle: &str,
        start_byte: usize,
        byte_count: usize,
        query: Option<&str>,
    ) -> Result<String, ToolError> {
        let state = self.0.lock().unwrap();
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.handle == handle)
            .ok_or_else(|| ToolError(format!("unknown or expired result handle: {handle}")))?;
        if let Some(query) = query {
            if query.is_empty() {
                return Err(ToolError("query must not be empty".to_string()));
            }
            let Some(index) = entry.content.find(query) else {
                return Ok(format!("query not found in {handle}: {query}"));
            };
            let radius = byte_count / 2;
            let start = floor_boundary(&entry.content, index.saturating_sub(radius));
            let end = ceil_boundary(
                &entry.content,
                index
                    .saturating_add(query.len())
                    .saturating_add(radius)
                    .min(entry.content.len()),
            );
            return Ok(format_read(entry, start, end));
        }

        let start = start_byte.saturating_sub(1).min(entry.content.len());
        let start = ceil_boundary(&entry.content, start);
        let end = ceil_boundary(
            &entry.content,
            start.saturating_add(byte_count).min(entry.content.len()),
        );
        Ok(format_read(entry, start, end))
    }
}

pub struct ReadToolResult(pub ResultStore);

impl Tool for ReadToolResult {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_tool_result".to_string(),
            description:
                "Read a bounded byte range or literal match from a large tool result handle"
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "handle": {"type": "string"},
                    "start_byte": {"type": "integer", "minimum": 1},
                    "byte_count": {"type": "integer", "minimum": 1, "maximum": MAX_READ_BYTES},
                    "query": {"type": "string"}
                },
                "required": ["handle"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let handle = string_arg(arguments, "handle")?;
        let start_byte = usize_arg(arguments, "start_byte")?.unwrap_or(1);
        let byte_count = usize_arg(arguments, "byte_count")?
            .unwrap_or(DEFAULT_READ_BYTES)
            .min(MAX_READ_BYTES);
        let query = arguments.get("query").map(|value| {
            value
                .as_str()
                .ok_or_else(|| ToolError("query must be a string".to_string()))
        });
        self.0
            .read(handle, start_byte, byte_count, query.transpose()?)
    }
}

fn format_read(entry: &StoredEntry, start: usize, end: usize) -> String {
    format!(
        "handle={} bytes={}-{} stored_bytes={} source_bytes={} source_truncated={}\n{}",
        entry.handle,
        start.saturating_add(1),
        end,
        entry.content.len(),
        entry.source_bytes,
        entry.source_truncated,
        &entry.content[start..end]
    )
}

fn retain_head_and_tail(content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    let marker = "\n... [stored result truncated] ...\n";
    let retained = max_bytes.saturating_sub(marker.len());
    let head_end = floor_boundary(&content, retained.div_ceil(2));
    let tail_start = ceil_boundary(&content, content.len() - retained / 2);
    format!(
        "{}{}{}",
        &content[..head_end],
        marker,
        &content[tail_start..]
    )
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn string_arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError(format!("{name} must be a string")))
}

fn usize_arg(arguments: &Value, name: &str) -> Result<Option<usize>, ToolError> {
    let Some(value) = arguments.get(name) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| ToolError(format!("{name} must be a positive integer")))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_results_support_ranges_queries_and_eviction() {
        let store = ResultStore::default();
        let first = store.store("alpha needle omega".to_string(), 18, false);

        assert!(
            store
                .read(&first.handle, 1, 5, None)
                .unwrap()
                .ends_with("alpha")
        );
        assert!(
            store
                .read(&first.handle, 1, 8, Some("needle"))
                .unwrap()
                .contains("needle")
        );

        for index in 0..MAX_RESULTS {
            store.store(format!("entry {index}"), 7, false);
        }
        assert!(store.read(&first.handle, 1, 5, None).is_err());
    }
}
