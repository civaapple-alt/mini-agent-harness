use mini_agent_protocol::ToolError;
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

const MAX_RESULTS: usize = 8;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
const PREVIEW_BYTES: usize = 4 * 1024;
const SIDECAR_DIRECTORY: &str = "tool_outputs";

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
    output_dir: PathBuf,
    namespace: String,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Default)]
struct StoreState {
    next_id: u64,
    total_bytes: usize,
    entries: VecDeque<StoredEntry>,
}

struct StoredEntry {
    handle: String,
    content: String,
    sidecar_path: Option<PathBuf>,
    source_bytes: usize,
    source_truncated: bool,
    metadata: Option<Value>,
}

pub struct StoredResult {
    pub handle: String,
    pub preview: String,
    pub stored_bytes: usize,
    pub source_bytes: usize,
    pub source_truncated: bool,
}

pub struct StoredPage {
    pub content: String,
    pub next_cursor: Option<usize>,
    pub source_bytes: usize,
    pub source_truncated: bool,
    pub metadata: Option<Value>,
}

impl ResultStore {
    pub(crate) fn for_session(path: PathBuf, append_lock: Arc<Mutex<()>>) -> Self {
        let namespace = session_namespace(&path);
        let output_dir = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(SIDECAR_DIRECTORY);
        let store = Self {
            inner: Arc::new(Mutex::new(load_session_results(
                &path,
                &output_dir,
                &namespace,
            ))),
            session: Some(SessionBinding {
                path,
                output_dir,
                namespace,
                append_lock,
            }),
        };
        store.trim_to_limits();
        store.remove_orphan_sidecars();
        store
    }

    pub fn store(
        &self,
        content: String,
        source_bytes: usize,
        source_truncated: bool,
    ) -> Result<StoredResult, ToolError> {
        self.store_with_metadata(content, source_bytes, source_truncated, None)
    }

    pub fn store_with_metadata(
        &self,
        content: String,
        source_bytes: usize,
        source_truncated: bool,
        metadata: Option<Value>,
    ) -> Result<StoredResult, ToolError> {
        let source_truncated = source_truncated || content.len() > MAX_RESULT_BYTES;
        let content = retain_head_and_tail(content, MAX_RESULT_BYTES);
        let preview = retain_head_and_tail(content.clone(), PREVIEW_BYTES);
        let mut state = self.inner.lock().unwrap();
        state.next_id = state.next_id.saturating_add(1);
        let handle = self.session.as_ref().map_or_else(
            || format!("result-{}", state.next_id),
            |session| format!("result-{}-{}", session.namespace, state.next_id),
        );
        let sidecar_path = if let Some(session) = &self.session {
            Some(persist_session_result(
                session,
                &handle,
                &content,
                source_bytes,
                source_truncated,
                metadata.as_ref(),
            )?)
        } else {
            None
        };
        state.total_bytes = state.total_bytes.saturating_add(content.len());
        let stored_bytes = content.len();
        state.entries.push_back(StoredEntry {
            handle: handle.clone(),
            content,
            sidecar_path,
            source_bytes,
            source_truncated,
            metadata,
        });
        trim_entries(&mut state);
        if let Some(session) = &self.session {
            remove_unreferenced_sidecars(session, &state.entries);
        }
        Ok(StoredResult {
            handle,
            preview,
            stored_bytes,
            source_bytes,
            source_truncated,
        })
    }

    /// Reads a bounded UTF-8 page from a result in this Session's cache.
    pub fn read_page(
        &self,
        handle: &str,
        cursor: usize,
        max_bytes: usize,
    ) -> Result<StoredPage, ToolError> {
        if max_bytes == 0 || max_bytes > 16 * 1024 {
            return Err(ToolError(
                "result page limit must be between 1 and 16384 bytes".into(),
            ));
        }
        let state = self.inner.lock().unwrap();
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.handle == handle)
            .ok_or_else(|| ToolError("result handle is missing or expired".into()))?;
        if cursor > entry.content.len() || !entry.content.is_char_boundary(cursor) {
            return Err(ToolError("result cursor is invalid".into()));
        }
        let mut end = cursor.saturating_add(max_bytes).min(entry.content.len());
        while end > cursor && !entry.content.is_char_boundary(end) {
            end -= 1;
        }
        Ok(StoredPage {
            content: entry.content[cursor..end].to_string(),
            next_cursor: (end < entry.content.len()).then_some(end),
            source_bytes: entry.source_bytes,
            source_truncated: entry.source_truncated,
            metadata: entry.metadata.clone(),
        })
    }

    fn trim_to_limits(&self) {
        let mut state = self.inner.lock().unwrap();
        trim_entries(&mut state);
        if let Some(session) = &self.session {
            remove_unreferenced_sidecars(session, &state.entries);
        }
    }

    fn remove_orphan_sidecars(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let state = self.inner.lock().unwrap();
        remove_unreferenced_sidecars(session, &state.entries);
    }
}

fn trim_entries(state: &mut StoreState) {
    while state.entries.len() > MAX_RESULTS || state.total_bytes > MAX_TOTAL_BYTES {
        let Some(removed) = state.entries.pop_front() else {
            break;
        };
        state.total_bytes = state.total_bytes.saturating_sub(removed.content.len());
    }
}

fn load_session_results(path: &Path, output_dir: &Path, namespace: &str) -> StoreState {
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
        let (content, sidecar_path) =
            if let Some(content) = record.get("content").and_then(Value::as_str) {
                (content.to_string(), None)
            } else {
                let Some(sidecar_path) = session_sidecar_path(output_dir, namespace, handle) else {
                    continue;
                };
                let Ok(file) = fs::File::open(&sidecar_path) else {
                    continue;
                };
                let mut content = String::new();
                if Read::take(file, MAX_RESULT_BYTES as u64 + 1)
                    .read_to_string(&mut content)
                    .is_err()
                    || content.len() > MAX_RESULT_BYTES
                {
                    continue;
                }
                (content, Some(sidecar_path))
            };
        let source_bytes = record
            .get("source_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(content.len() as u64) as usize;
        let source_truncated = record
            .get("source_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let metadata = record
            .get("metadata")
            .filter(|value| !value.is_null())
            .cloned();
        let numeric_id = handle
            .rsplit_once('-')
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .unwrap_or(0);
        state.next_id = state.next_id.max(numeric_id);
        state.total_bytes = state.total_bytes.saturating_add(content.len());
        state.entries.push_back(StoredEntry {
            handle: handle.to_string(),
            content,
            sidecar_path,
            source_bytes,
            source_truncated,
            metadata,
        });
    }
    state
}

fn persist_session_result(
    session: &SessionBinding,
    handle: &str,
    content: &str,
    source_bytes: usize,
    source_truncated: bool,
    metadata: Option<&Value>,
) -> Result<PathBuf, ToolError> {
    let _guard = session.append_lock.lock().unwrap();
    fs::create_dir_all(&session.output_dir).map_err(|error| {
        ToolError(format!(
            "cannot create Session tool output directory: {error}"
        ))
    })?;
    let sidecar_path = session_sidecar_path(&session.output_dir, &session.namespace, handle)
        .ok_or_else(|| ToolError("invalid tool output handle".to_string()))?;
    let temporary_path = sidecar_path.with_extension("tmp");
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|error| ToolError(format!("cannot create tool output sidecar: {error}")))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_data())
            .map_err(|error| ToolError(format!("cannot persist tool output sidecar: {error}")))?;
        fs::rename(&temporary_path, &sidecar_path)
            .map_err(|error| ToolError(format!("cannot finalize tool output sidecar: {error}")))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    let metadata_result = (|| {
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
            "storage": "sidecar",
            "source_bytes": source_bytes,
            "source_truncated": source_truncated,
            "metadata": metadata,
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
            .map_err(|error| ToolError(format!("cannot persist tool result metadata: {error}")))
    })();
    if let Err(error) = metadata_result {
        let _ = fs::remove_file(&sidecar_path);
        return Err(error);
    }
    Ok(sidecar_path)
}

fn session_namespace(path: &Path) -> String {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let hex = b"0123456789abcdef";
    let mut namespace = String::with_capacity(32);
    for byte in &digest[..16] {
        namespace.push(hex[usize::from(byte >> 4)] as char);
        namespace.push(hex[usize::from(byte & 0x0f)] as char);
    }
    namespace
}

fn session_sidecar_path(output_dir: &Path, namespace: &str, handle: &str) -> Option<PathBuf> {
    let prefix = format!("result-{namespace}-");
    let identifier = handle.strip_prefix(&prefix)?;
    if identifier.is_empty() || !identifier.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(output_dir.join(format!("{handle}.txt")))
}

fn remove_unreferenced_sidecars(session: &SessionBinding, entries: &VecDeque<StoredEntry>) {
    let Ok(files) = fs::read_dir(&session.output_dir) else {
        return;
    };
    let active = entries
        .iter()
        .filter_map(|entry| entry.sidecar_path.as_ref())
        .collect::<std::collections::HashSet<_>>();
    for file in files.flatten() {
        let path = file.path();
        if !active.contains(&path) {
            let _ = fs::remove_file(path);
        }
    }
}

impl ResultStore {
    /// Returns a bounded head-and-tail preview suitable for a model-facing notice.
    pub fn bounded_preview(content: &str, max_bytes: usize) -> String {
        retain_head_and_tail(content.to_string(), max_bytes)
    }
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
    fn result_pages_keep_utf8_boundaries_and_restore_metadata() {
        let _home_lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let context = Message::Context {
            text: "seed".into(),
        };
        opened
            .store
            .record_context(&context, std::slice::from_ref(&context))
            .unwrap();
        let store = opened.store.result_store();
        let result = store
            .store_with_metadata(
                "一二三四".into(),
                12,
                false,
                Some(json!({
                    "kind": "web_fetch",
                    "url": "https://example.com",
                    "title": "Example"
                })),
            )
            .unwrap();
        let first = store.read_page(&result.handle, 0, 4).unwrap();
        assert_eq!(first.content, "一");
        let cursor = first.next_cursor.unwrap();
        drop(store);
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let restored = resumed.store.result_store();
        let next = restored.read_page(&result.handle, cursor, 4).unwrap();
        assert_eq!(next.content, "二");
        assert_eq!(next.metadata.as_ref().unwrap()["kind"], "web_fetch");
        assert_eq!(next.metadata.unwrap()["url"], "https://example.com");
        drop(resumed);
        remove_test_root(&root);
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
        assert!(next.handle.ends_with("-2"));
        drop(resumed);
        remove_test_root(&root);
    }

    #[test]
    fn session_result_store_keeps_large_content_in_a_sidecar() {
        let _home_lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let session_path = opened.store.path().to_path_buf();
        let store = opened.store.result_store();
        let stored = store
            .store_with_metadata(
                "\n".repeat(300 * 1024),
                300 * 1024,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();
        assert!(!stored.source_truncated);
        assert_eq!(stored.stored_bytes, 300 * 1024);
        let metadata = fs::read_to_string(&session_path).unwrap();
        let record: Value = serde_json::from_str(metadata.lines().last().unwrap()).unwrap();
        assert_eq!(record["storage"], "sidecar");
        assert!(record.get("content").is_none());
        drop(store);
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let restored = resumed.store.result_store();
        let page = restored.read_page(&stored.handle, 0, 12 * 1024).unwrap();
        assert_eq!(page.content.len(), 12 * 1024);
        assert_eq!(page.next_cursor, Some(12 * 1024));
        assert!(!page.source_truncated);
        drop(resumed);
        remove_test_root(&root);
    }

    #[test]
    fn session_tool_output_handles_are_scoped_and_evicted_with_the_session_quota() {
        let first_root = test_root();
        let second_root = test_root();
        let first_path = first_root.join("session.jsonl");
        let second_path = second_root.join("session.jsonl");
        fs::write(&first_path, "").unwrap();
        fs::write(&second_path, "").unwrap();
        let first_store = ResultStore::for_session(first_path, Arc::new(Mutex::new(())));
        let second_store = ResultStore::for_session(second_path, Arc::new(Mutex::new(())));
        let first = first_store
            .store_with_metadata(
                "first session output".to_string(),
                20,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();
        let second = second_store
            .store_with_metadata(
                "second session output".to_string(),
                21,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();

        assert_ne!(first.handle, second.handle);
        assert!(second_store.read_page(&first.handle, 0, 1024).is_err());
        assert_eq!(
            first_store
                .read_page(&first.handle, 0, 1024)
                .unwrap()
                .content,
            "first session output"
        );

        let mut retained = Vec::new();
        for index in 0..=MAX_RESULTS {
            retained.push(
                first_store
                    .store_with_metadata(
                        format!("output-{index}"),
                        9,
                        false,
                        Some(json!({"kind": "tool_output"})),
                    )
                    .unwrap()
                    .handle,
            );
        }
        assert!(first_store.read_page(&first.handle, 0, 1024).is_err());
        assert!(first_store.read_page(&retained[0], 0, 1024).is_err());
        assert_eq!(
            first_store
                .read_page(retained.last().unwrap(), 0, 1024)
                .unwrap()
                .content,
            format!("output-{MAX_RESULTS}")
        );

        let large_first = first_store
            .store_with_metadata(
                "a".repeat(MAX_RESULT_BYTES),
                MAX_RESULT_BYTES,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();
        let large_second = first_store
            .store_with_metadata(
                "b".repeat(MAX_RESULT_BYTES),
                MAX_RESULT_BYTES,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();
        let large_third = first_store
            .store_with_metadata(
                "c".repeat(MAX_RESULT_BYTES),
                MAX_RESULT_BYTES,
                false,
                Some(json!({"kind": "tool_output"})),
            )
            .unwrap();
        assert!(first_store.read_page(&large_first.handle, 0, 1).is_err());
        assert!(first_store.read_page(&large_second.handle, 0, 1).is_ok());
        assert!(first_store.read_page(&large_third.handle, 0, 1).is_ok());
        let retained_sidecars = fs::read_dir(first_root.join(SIDECAR_DIRECTORY))
            .unwrap()
            .count();
        assert_eq!(retained_sidecars, 2);

        drop(first_store);
        drop(second_store);
        remove_test_root(&first_root);
        remove_test_root(&second_root);
    }
}
