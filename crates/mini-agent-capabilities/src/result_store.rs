use mini_agent_protocol::ToolError;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

const MAX_RESULTS: usize = 8;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
// JSON escaping can expand newline-heavy output; keep persisted payloads well
// below the 512 KiB session record ceiling.
const MAX_PERSISTED_RESULT_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 4 * 1024;

#[derive(Clone)]
pub struct ResultStore {
    inner: Arc<Mutex<StoreState>>,
    session: Option<SessionBinding>,
}

impl Default for ResultStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreState::default())),
            session: None,
        }
    }
}

#[derive(Clone)]
struct SessionBinding {
    path: PathBuf,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Default)]
struct StoreState {
    next_id: u64,
    total_bytes: usize,
    entries: VecDeque<StoredEntry>,
}

struct StoredEntry {
    content: String,
}

pub struct StoredResult {
    pub handle: String,
    pub preview: String,
    pub stored_bytes: usize,
    pub source_bytes: usize,
    pub source_truncated: bool,
}

impl ResultStore {
    pub(crate) fn for_session(path: PathBuf, append_lock: Arc<Mutex<()>>) -> Self {
        let store = Self {
            inner: Arc::new(Mutex::new(load_session_results(&path))),
            session: Some(SessionBinding { path, append_lock }),
        };
        store.trim_to_limits();
        store
    }

    pub fn store(
        &self,
        content: String,
        source_bytes: usize,
        source_truncated: bool,
    ) -> Result<StoredResult, ToolError> {
        let source_truncated = source_truncated || content.len() > MAX_RESULT_BYTES;
        let max_bytes = if self.session.is_some() {
            MAX_PERSISTED_RESULT_BYTES
        } else {
            MAX_RESULT_BYTES
        };
        let source_truncated = source_truncated || content.len() > max_bytes;
        let content = retain_head_and_tail(content, max_bytes);
        let preview = retain_head_and_tail(content.clone(), PREVIEW_BYTES);
        let mut state = self.inner.lock().unwrap();
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
        if let Some(session) = &self.session {
            append_session_result(session, &handle, &content, source_bytes, source_truncated)?;
        }
        state.total_bytes = state.total_bytes.saturating_add(content.len());
        let stored_bytes = content.len();
        state.entries.push_back(StoredEntry { content });
        Ok(StoredResult {
            handle,
            preview,
            stored_bytes,
            source_bytes,
            source_truncated,
        })
    }

    fn trim_to_limits(&self) {
        let mut state = self.inner.lock().unwrap();
        while state.entries.len() > MAX_RESULTS || state.total_bytes > MAX_TOTAL_BYTES {
            let Some(removed) = state.entries.pop_front() else {
                break;
            };
            state.total_bytes = state.total_bytes.saturating_sub(removed.content.len());
        }
    }
}

fn load_session_results(path: &PathBuf) -> StoreState {
    let Ok(bytes) = fs::read(path) else {
        return StoreState::default();
    };
    let mut state = StoreState::default();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if record.get("kind").and_then(Value::as_str) != Some("result_stored") {
            continue;
        }
        let Some(handle) = record.get("handle").and_then(Value::as_str) else {
            continue;
        };
        let Some(content) = record.get("content").and_then(Value::as_str) else {
            continue;
        };
        let numeric_id = handle
            .strip_prefix("result-")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        state.next_id = state.next_id.max(numeric_id);
        state.total_bytes = state.total_bytes.saturating_add(content.len());
        state.entries.push_back(StoredEntry {
            content: content.to_string(),
        });
    }
    state
}

fn append_session_result(
    session: &SessionBinding,
    handle: &str,
    content: &str,
    source_bytes: usize,
    source_truncated: bool,
) -> Result<(), ToolError> {
    let _guard = session.append_lock.lock().unwrap();
    let bytes = fs::read(&session.path).map_err(|error| {
        ToolError(format!(
            "cannot read session before storing tool result: {error}"
        ))
    })?;
    let next_seq = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|record| record.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut record = json!({
        "kind": "result_stored",
        "handle": handle,
        "content": content,
        "source_bytes": source_bytes,
        "source_truncated": source_truncated,
        "timestamp_ms": crate::session::timestamp_ms(),
    });
    record
        .as_object_mut()
        .expect("result record is an object")
        .insert("seq".to_string(), json!(next_seq));
    let encoded = serde_json::to_vec(&record)
        .map_err(|error| ToolError(format!("cannot encode stored tool result: {error}")))?;
    if encoded.len() > crate::session::MAX_RECORD_BYTES {
        return Err(ToolError(
            "stored tool result exceeds session record limit".to_string(),
        ));
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(&session.path)
        .map_err(|error| ToolError(format!("cannot open session for tool result: {error}")))?;
    file.write_all(&encoded)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_data())
        .map_err(|error| ToolError(format!("cannot persist tool result: {error}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionRequest;
    use crate::session::SessionStore;
    use crate::test_support::{HOME_LOCK, remove_test_root, test_root};
    use mini_agent_protocol::Message;

    #[test]
    fn oversized_results_report_cache_truncation() {
        let store = ResultStore::default();
        let result = store
            .store(
                "x".repeat(MAX_RESULT_BYTES + 1),
                MAX_RESULT_BYTES + 1,
                false,
            )
            .unwrap();
        assert!(result.source_truncated);
        assert_eq!(result.stored_bytes, MAX_RESULT_BYTES);
    }

    #[test]
    fn session_result_store_reloads_from_append_log() {
        let _home_lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let context = Message::Context {
            text: "seed".to_string(),
        };
        opened
            .store
            .record_context(&context, std::slice::from_ref(&context))
            .unwrap();
        let store = opened.store.result_store();
        store
            .store("persisted result".to_string(), 16, false)
            .unwrap();
        drop(store);
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let restored = resumed.store.result_store();
        let next = restored
            .store("next result".to_string(), 11, false)
            .unwrap();
        assert_eq!(next.handle, "result-2");
        drop(resumed);
        remove_test_root(&root);
    }

    #[test]
    fn session_result_store_bounds_persisted_content() {
        let _home_lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let store = opened.store.result_store();
        let stored = store
            .store("\n".repeat(300 * 1024), 300 * 1024, false)
            .unwrap();
        assert!(stored.source_truncated);
        assert!(stored.stored_bytes <= MAX_PERSISTED_RESULT_BYTES);
        drop(store);
        drop(opened);
        remove_test_root(&root);
    }
}
