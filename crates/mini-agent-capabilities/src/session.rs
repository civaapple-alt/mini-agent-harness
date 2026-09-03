use mini_agent_core::SessionState;
use mini_agent_protocol::Message;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[path = "session/storage.rs"]
mod storage;
use storage::{
    acquire_lock, copy_attachments, load_records, validate_session_id, write_json_atomic,
    write_prompt_context,
};
pub use storage::{resolve_session_file, session_directory};

const SCHEMA_VERSION: u64 = 1;
const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_WORKSPACE_KEY: usize = 240;
const SESSION_FILE_NAME: &str = "session.jsonl";
const SESSION_LOCK_NAME: &str = "session";
pub const SUMMARY_FILE_NAME: &str = "summary.json";
pub const SIGNALS_FILE_NAME: &str = "signals.json";
pub const PROMPT_CONTEXT_FILE_NAME: &str = "prompt_context.json";
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub enum SessionRequest {
    Disabled,
    New,
    Named(String),
    Resume(String),
    Fork(String),
}

pub struct OpenedSession {
    pub store: SessionStore,
    pub state: SessionState,
    pub resumed: bool,
}

pub struct SessionStore {
    session_id: String,
    thread_id: String,
    session_dir: PathBuf,
    path: PathBuf,
    file: File,
    bytes: u64,
    next_seq: u64,
    checkpoint_seq: u64,
    turn_count: usize,
    thread_turn_count: usize,
    items: Vec<SessionItem>,
    created_at_ms: u64,
    pub(crate) append_lock: Arc<Mutex<()>>,
    _lock: SessionLock,
}

#[derive(Clone, Copy)]
pub enum TurnStatus {
    Completed,
    StepLimit,
    Steered,
    Cancelled,
    Failed,
}

pub struct TurnCommit<'a> {
    pub started_at_ms: u64,
    pub prompt: &'a str,
    pub status: TurnStatus,
    pub steps: usize,
    pub error: Option<&'a str>,
    pub messages: &'a [Message],
    pub checkpoint: &'a [Message],
}

/// One durable message record that can be projected into a public ThreadItem.
/// The session JSONL remains authoritative; this value is only the bounded
/// in-process index used by the App Server item listing.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionItem {
    pub item_id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub message: Message,
}

struct SessionLock(PathBuf);

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl SessionStore {
    pub fn open(workspace: &Path, request: SessionRequest) -> Result<OpenedSession, String> {
        match request {
            SessionRequest::Disabled => Err("session persistence is disabled".to_string()),
            SessionRequest::New => Self::create(workspace),
            SessionRequest::Named(session_id) => Self::create_named(workspace, &session_id),
            SessionRequest::Resume(session_id) => Self::resume(workspace, &session_id),
            SessionRequest::Fork(session_id) => Self::fork(workspace, &session_id),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    pub fn thread_turn_count(&self) -> usize {
        self.thread_turn_count
    }

    pub fn items(&self) -> &[SessionItem] {
        &self.items
    }

    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn result_store(&self) -> crate::result_store::ResultStore {
        crate::result_store::ResultStore::for_session(
            self.path.clone(),
            Arc::clone(&self.append_lock),
        )
    }

    pub fn start_thread(&mut self) -> Result<(), String> {
        let thread_id = new_id("t");
        self.append_records(vec![json!({
            "kind": "thread_started",
            "thread_id": thread_id,
            "timestamp_ms": timestamp_ms(),
        })])?;
        self.thread_id = thread_id;
        self.thread_turn_count = 0;
        Ok(())
    }

    pub fn record_context(
        &mut self,
        context: &Message,
        checkpoint: &[Message],
    ) -> Result<(), String> {
        self.append_records(vec![
            self.item_record(/*turn_id*/ None, context),
            self.checkpoint_record(checkpoint),
        ])?;
        self.checkpoint_seq = self.next_seq.saturating_sub(1);
        Ok(())
    }

    pub fn record_turn_with_id(
        &mut self,
        turn_id: &str,
        turn: TurnCommit<'_>,
    ) -> Result<(), String> {
        let mut records = vec![json!({
            "kind": "turn_started",
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "timestamp_ms": turn.started_at_ms,
            "prompt": turn.prompt,
        })];
        let items = turn
            .messages
            .iter()
            .map(|message| {
                let item_id = item_id_for_message(message);
                records.push(self.item_record_with_id(Some(turn_id), message, &item_id));
                SessionItem {
                    item_id,
                    thread_id: self.thread_id.clone(),
                    turn_id: Some(turn_id.to_string()),
                    message: message.clone(),
                }
            })
            .collect::<Vec<_>>();
        records.push(json!({
            "kind": "turn_settled",
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "timestamp_ms": timestamp_ms(),
            "status": turn.status.name(),
            "steps": turn.steps,
            "error": turn.error,
        }));
        records.push(self.checkpoint_record(turn.checkpoint));
        self.append_records(records)?;
        self.items.extend(items);
        self.checkpoint_seq = self.next_seq.saturating_sub(1);
        self.turn_count = self.turn_count.saturating_add(1);
        self.thread_turn_count = self.thread_turn_count.saturating_add(1);
        self.update_summary_and_signals(turn.prompt, turn.steps, turn.error);
        Ok(())
    }

    fn update_summary_and_signals(&self, last_prompt: &str, steps: usize, error: Option<&str>) {
        let now = timestamp_ms();
        let summary_path = self.session_dir.join(SUMMARY_FILE_NAME);
        let summary_val = json!({
            "id": self.session_id,
            "created_at_ms": self.created_at_ms,
            "updated_at_ms": now,
            "turn_count": self.turn_count,
            "bytes": self.bytes,
            "last_prompt": last_prompt,
            "last_status": if error.is_some() { "error" } else { "completed" },
        });
        let _ = write_json_atomic(&summary_path, &summary_val);

        let signals_path = self.session_dir.join(SIGNALS_FILE_NAME);
        let signals = if let Ok(data) = fs::read_to_string(&signals_path) {
            serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };
        let prev_steps = signals
            .get("step_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let prev_tool_calls = signals
            .get("tool_call_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let new_signals = json!({
            "turn_count": self.turn_count,
            "step_count": prev_steps + steps as u64,
            "tool_call_count": prev_tool_calls + steps.saturating_sub(1) as u64,
            "updated_at_ms": now,
        });
        let _ = write_json_atomic(&signals_path, &new_signals);
    }

    fn create(workspace: &Path) -> Result<OpenedSession, String> {
        let base_dir = session_directory(workspace)?;
        fs::create_dir_all(&base_dir)
            .map_err(|error| format!("cannot create session directory: {error}"))?;
        for _ in 0..16 {
            let session_id = new_id("s");
            let session_dir = base_dir.join(&session_id);
            if session_dir.exists() {
                continue;
            }
            fs::create_dir(&session_dir)
                .map_err(|error| format!("cannot create session directory: {error}"))?;
            let lock = match acquire_lock(&session_dir, SESSION_LOCK_NAME) {
                Ok(lock) => lock,
                Err(error) => {
                    let _ = fs::remove_dir(&session_dir);
                    return Err(error);
                }
            };
            let path = session_dir.join(SESSION_FILE_NAME);
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) => {
                    let _ = fs::remove_dir_all(&session_dir);
                    return Err(format!("cannot create session file: {error}"));
                }
            };
            let store = Self::initialize_new(
                workspace,
                &session_id,
                session_dir,
                path,
                file,
                lock,
                &[],
                None,
            )?;
            return Ok(OpenedSession {
                store,
                state: SessionState::new(),
                resumed: false,
            });
        }
        Err("cannot allocate a unique session id".to_string())
    }

    fn create_named(workspace: &Path, session_id: &str) -> Result<OpenedSession, String> {
        validate_session_id(session_id)?;
        let base_dir = session_directory(workspace)?;
        fs::create_dir_all(&base_dir)
            .map_err(|error| format!("cannot create session directory: {error}"))?;
        let session_dir = base_dir.join(session_id);
        if session_dir.exists() {
            return Self::resume(workspace, session_id);
        }
        fs::create_dir(&session_dir)
            .map_err(|error| format!("cannot create session directory: {error}"))?;
        let lock = match acquire_lock(&session_dir, SESSION_LOCK_NAME) {
            Ok(lock) => lock,
            Err(error) => {
                let _ = fs::remove_dir(&session_dir);
                return Err(error);
            }
        };
        let path = session_dir.join(SESSION_FILE_NAME);
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_dir_all(&session_dir);
                return Err(format!("cannot create session file: {error}"));
            }
        };
        let store = Self::initialize_new(
            workspace,
            session_id,
            session_dir,
            path,
            file,
            lock,
            &[],
            None,
        )?;
        Ok(OpenedSession {
            store,
            state: SessionState::new(),
            resumed: false,
        })
    }

    fn resume(workspace: &Path, session_id: &str) -> Result<OpenedSession, String> {
        validate_session_id(session_id)?;
        let (session_dir, path) = resolve_session_file(workspace, session_id)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot open session {session_id}: {error}"))?;
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(format!("session exceeds {MAX_SESSION_BYTES} byte limit"));
        }
        let lock = acquire_lock(&session_dir, SESSION_LOCK_NAME)?;
        let bytes = fs::read(&path).map_err(|error| format!("cannot read session: {error}"))?;
        let loaded = load_records(session_id, &bytes)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("cannot resume session: {error}"))?;
        if loaded.valid_bytes < bytes.len() {
            file.set_len(loaded.valid_bytes as u64)
                .map_err(|error| format!("cannot remove torn session tail: {error}"))?;
        }
        file.seek(SeekFrom::End(0))
            .map_err(|error| format!("cannot seek session append position: {error}"))?;
        let store = Self {
            session_id: session_id.to_string(),
            thread_id: loaded.thread_id,
            session_dir,
            path,
            file,
            bytes: loaded.valid_bytes as u64,
            next_seq: loaded.next_seq,
            checkpoint_seq: loaded.checkpoint_seq,
            turn_count: loaded.turn_count,
            thread_turn_count: loaded.thread_turn_count,
            items: loaded.items,
            created_at_ms: loaded.created_at_ms,
            append_lock: Arc::new(Mutex::new(())),
            _lock: lock,
        };
        Ok(OpenedSession {
            store,
            state: SessionState::from_messages(loaded.messages),
            resumed: true,
        })
    }

    fn fork(workspace: &Path, parent_session_id: &str) -> Result<OpenedSession, String> {
        validate_session_id(parent_session_id)?;
        let (parent_dir, parent_path) = resolve_session_file(workspace, parent_session_id)?;
        let metadata = fs::metadata(&parent_path)
            .map_err(|error| format!("cannot open parent session {parent_session_id}: {error}"))?;
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(format!(
                "parent session exceeds {MAX_SESSION_BYTES} byte limit"
            ));
        }
        let bytes = fs::read(&parent_path)
            .map_err(|error| format!("cannot read parent session: {error}"))?;
        let loaded = load_records(parent_session_id, &bytes)?;
        let parent_checkpoint_seq = loaded.checkpoint_seq;
        let parent_messages = loaded.messages;

        let project_dir = session_directory(workspace)?;
        for _ in 0..16 {
            let session_id = new_id("s");
            let session_dir = project_dir.join(&session_id);
            fs::create_dir_all(&session_dir)
                .map_err(|error| format!("cannot create session directory: {error}"))?;
            let path = session_dir.join(SESSION_FILE_NAME);
            let file = match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("cannot create session: {error}")),
            };
            let lock = acquire_lock(&session_dir, SESSION_LOCK_NAME)?;
            let store = Self::initialize_new(
                workspace,
                &session_id,
                session_dir.clone(),
                path,
                file,
                lock,
                &parent_messages,
                Some((parent_session_id, parent_checkpoint_seq)),
            )?;
            copy_attachments(
                &parent_dir.join("attachments"),
                &session_dir.join("attachments"),
            );
            return Ok(OpenedSession {
                store,
                state: SessionState::from_messages(parent_messages),
                resumed: true,
            });
        }
        Err("cannot allocate a unique session id".to_string())
    }

    #[allow(clippy::too_many_arguments)]
    fn initialize_new(
        workspace: &Path,
        session_id: &str,
        session_dir: PathBuf,
        path: PathBuf,
        file: File,
        lock: SessionLock,
        checkpoint: &[Message],
        forked_from: Option<(&str, u64)>,
    ) -> Result<Self, String> {
        let thread_id = new_id("t");
        let now = timestamp_ms();
        let mut store = Self {
            session_id: session_id.to_string(),
            thread_id: thread_id.clone(),
            session_dir,
            path,
            file,
            bytes: 0,
            next_seq: 1,
            checkpoint_seq: 0,
            turn_count: 0,
            thread_turn_count: 0,
            items: Vec::new(),
            created_at_ms: now,
            append_lock: Arc::new(Mutex::new(())),
            _lock: lock,
        };
        let mut header = json!({
            "kind": "session_created",
            "schema_version": SCHEMA_VERSION,
            "session_id": session_id,
            "workspace": workspace,
            "timestamp_ms": now,
        });
        if let Some((parent_session_id, parent_checkpoint_seq)) = forked_from {
            header["forked_from"] = json!({
                "parent_session_id": parent_session_id,
                "parent_checkpoint_seq": parent_checkpoint_seq,
            });
        }
        store.append_records(vec![
            header,
            json!({
                "kind": "thread_started",
                "thread_id": thread_id,
                "timestamp_ms": now,
            }),
            store.checkpoint_record(checkpoint),
        ])?;
        store.checkpoint_seq = store.next_seq.saturating_sub(1);
        write_prompt_context(&store.session_dir, workspace, session_id);
        store.update_summary_and_signals("", 0, None);
        Ok(store)
    }

    fn item_record(&self, turn_id: Option<&str>, message: &Message) -> Value {
        let item_id = item_id_for_message(message);
        self.item_record_with_id(turn_id, message, &item_id)
    }

    fn item_record_with_id(
        &self,
        turn_id: Option<&str>,
        message: &Message,
        item_id: &str,
    ) -> Value {
        json!({
            "kind": "item",
            "item_id": item_id,
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "item_kind": message_kind(message),
            "timestamp_ms": timestamp_ms(),
            "message": message,
        })
    }

    fn checkpoint_record(&self, messages: &[Message]) -> Value {
        json!({
            "kind": "checkpoint",
            "thread_id": self.thread_id,
            "timestamp_ms": timestamp_ms(),
            "messages": messages,
        })
    }

    fn append_records(&mut self, mut records: Vec<Value>) -> Result<(), String> {
        let append_lock = Arc::clone(&self.append_lock);
        let _append_guard = append_lock.lock().unwrap();
        self.refresh_append_position()?;
        let original_bytes = self.bytes;
        let original_next_seq = self.next_seq;
        let mut encoded = Vec::new();
        let mut next_seq = self.next_seq;
        for record in &mut records {
            let object = record
                .as_object_mut()
                .ok_or_else(|| "session record must be an object".to_string())?;
            object.insert("seq".to_string(), json!(next_seq));
            next_seq = next_seq.saturating_add(1);
            let line = serde_json::to_vec(record)
                .map_err(|error| format!("cannot encode session record: {error}"))?;
            if line.len() > MAX_RECORD_BYTES {
                return Err(format!(
                    "session record exceeds {MAX_RECORD_BYTES} byte limit"
                ));
            }
            encoded.extend_from_slice(&line);
            encoded.push(b'\n');
        }
        let next_bytes = self.bytes.saturating_add(encoded.len() as u64);
        if next_bytes > MAX_SESSION_BYTES {
            return Err(format!("session exceeds {MAX_SESSION_BYTES} byte limit"));
        }
        let write_result = self
            .file
            .write_all(&encoded)
            .and_then(|()| self.file.flush())
            .and_then(|()| self.file.sync_data());
        if let Err(error) = write_result {
            let rollback_result = self
                .file
                .set_len(original_bytes)
                .and_then(|()| self.file.seek(SeekFrom::End(0)).map(|_| ()))
                .and_then(|()| self.file.sync_data());
            self.bytes = original_bytes;
            self.next_seq = original_next_seq;
            return match rollback_result {
                Ok(()) => Err(format!("cannot persist session: {error}")),
                Err(rollback) => Err(format!(
                    "cannot persist session: {error}; rollback failed: {rollback}"
                )),
            };
        }
        self.bytes = next_bytes;
        self.next_seq = next_seq;
        Ok(())
    }

    fn refresh_append_position(&mut self) -> Result<(), String> {
        let bytes = fs::read(&self.path)
            .map_err(|error| format!("cannot read session append position: {error}"))?;
        self.bytes = bytes.len() as u64;
        self.next_seq = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .filter_map(|record| record.get("seq").and_then(Value::as_u64))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Ok(())
    }
}

impl TurnStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::StepLimit => "step_limit",
            Self::Steered => "steered",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

fn message_kind(message: &Message) -> &'static str {
    match message {
        Message::Context { .. } => "context",
        Message::User { .. } => "user",
        Message::Assistant { .. } => "assistant",
        Message::Tool { .. } => "tool_settlement",
    }
}

fn item_id_for_message(message: &Message) -> String {
    match message {
        Message::Tool { call_id, .. } if !call_id.is_empty() => call_id.clone(),
        _ => new_id("i"),
    }
}

fn new_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}-{:x}-{:x}-{:x}",
        timestamp_ms(),
        std::process::id(),
        counter
    )
}

pub(crate) fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_index_survives_session_resume() {
        let root = std::env::temp_dir().join(format!(
            "mini-agent-session-items-{}-{}",
            std::process::id(),
            timestamp_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let messages = vec![
            Message::User {
                text: "hello".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "done".to_string(),
                tool_calls: Vec::new(),
            },
        ];
        opened
            .store
            .record_turn_with_id(
                "turn-1",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "hello",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &messages,
                    checkpoint: &messages,
                },
            )
            .unwrap();
        assert_eq!(opened.store.items().len(), 2);
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert_eq!(resumed.store.items().len(), 2);
        assert_eq!(resumed.store.items()[0].turn_id.as_deref(), Some("turn-1"));
        drop(resumed);
        std::fs::remove_dir_all(root).unwrap();
    }
}
