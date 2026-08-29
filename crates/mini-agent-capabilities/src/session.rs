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

#[path = "session_derived.rs"]
mod derived;
pub use derived::DerivedItem;

const SCHEMA_VERSION: u64 = 1;
const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_SESSIONS: usize = 128;
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

pub struct SessionSummary {
    pub id: String,
    pub bytes: u64,
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn result_store(&self) -> crate::result_store::ResultStore {
        crate::result_store::ResultStore::for_session(
            self.path.clone(),
            Arc::clone(&self.append_lock),
        )
    }

    #[cfg(test)]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
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

    pub fn record_turn(&mut self, turn: TurnCommit<'_>) -> Result<(), String> {
        let turn_id = new_id("turn");
        self.record_turn_with_id(&turn_id, turn)
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
        records.extend(
            turn.messages
                .iter()
                .map(|message| self.item_record(Some(turn_id), message)),
        );
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
            let thread_id = new_id("t");
            let now = timestamp_ms();
            let mut store = Self {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                session_dir: session_dir.clone(),
                path,
                file,
                bytes: 0,
                next_seq: 1,
                checkpoint_seq: 0,
                turn_count: 0,
                thread_turn_count: 0,
                created_at_ms: now,
                append_lock: Arc::new(Mutex::new(())),
                _lock: lock,
            };
            store.append_records(vec![
                json!({
                    "kind": "session_created",
                    "schema_version": SCHEMA_VERSION,
                    "session_id": session_id,
                    "workspace": workspace,
                    "timestamp_ms": now,
                }),
                json!({
                    "kind": "thread_started",
                    "thread_id": thread_id,
                    "timestamp_ms": now,
                }),
            ])?;
            write_prompt_context(&session_dir, workspace, &session_id);
            store.update_summary_and_signals("", 0, None);
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
        let thread_id = new_id("t");
        let now = timestamp_ms();
        let mut store = Self {
            session_id: session_id.to_string(),
            thread_id: thread_id.clone(),
            session_dir: session_dir.clone(),
            path,
            file,
            bytes: 0,
            next_seq: 1,
            checkpoint_seq: 0,
            turn_count: 0,
            thread_turn_count: 0,
            created_at_ms: now,
            append_lock: Arc::new(Mutex::new(())),
            _lock: lock,
        };
        store.append_records(vec![
            json!({
                "kind": "session_created",
                "schema_version": SCHEMA_VERSION,
                "session_id": session_id,
                "workspace": workspace,
                "timestamp_ms": now,
            }),
            json!({
                "kind": "thread_started",
                "thread_id": thread_id,
                "timestamp_ms": now,
            }),
        ])?;
        write_prompt_context(&session_dir, workspace, session_id);
        store.update_summary_and_signals("", 0, None);
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
            let thread_id = new_id("t");
            let now = timestamp_ms();
            let mut store = Self {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                session_dir: session_dir.clone(),
                path,
                file,
                bytes: 0,
                next_seq: 1,
                checkpoint_seq: 0,
                turn_count: 0,
                thread_turn_count: 0,
                created_at_ms: now,
                append_lock: Arc::new(Mutex::new(())),
                _lock: lock,
            };
            store.append_records(vec![
                json!({
                    "kind": "session_created",
                    "schema_version": SCHEMA_VERSION,
                    "session_id": session_id,
                    "workspace": workspace,
                    "timestamp_ms": now,
                    "forked_from": {
                        "parent_session_id": parent_session_id,
                        "parent_checkpoint_seq": parent_checkpoint_seq,
                    }
                }),
                json!({
                    "kind": "thread_started",
                    "thread_id": thread_id,
                    "timestamp_ms": now,
                }),
                store.checkpoint_record(&parent_messages),
            ])?;
            store.checkpoint_seq = store.next_seq.saturating_sub(1);
            copy_attachments(
                &parent_dir.join("attachments"),
                &session_dir.join("attachments"),
            );
            write_prompt_context(&session_dir, workspace, &session_id);
            store.update_summary_and_signals("", 0, None);
            return Ok(OpenedSession {
                store,
                state: SessionState::from_messages(parent_messages),
                resumed: true,
            });
        }
        Err("cannot allocate a unique session id".to_string())
    }

    fn item_record(&self, turn_id: Option<&str>, message: &Message) -> Value {
        json!({
            "kind": "item",
            "item_id": new_id("i"),
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
        self.file
            .write_all(&encoded)
            .and_then(|()| self.file.flush())
            .and_then(|()| self.file.sync_data())
            .map_err(|error| format!("cannot persist session: {error}"))?;
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

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let parent = path.parent().ok_or_else(|| "no parent dir".to_string())?;
    let temp_path = parent.join(format!(".tmp_{}", new_id("tmp")));
    let encoded =
        serde_json::to_vec_pretty(value).map_err(|e| format!("cannot encode json: {e}"))?;
    fs::write(&temp_path, &encoded).map_err(|e| format!("cannot write temp file: {e}"))?;
    fs::rename(&temp_path, path).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("cannot rename atomic file: {e}")
    })?;
    Ok(())
}

fn write_prompt_context(session_dir: &Path, workspace: &Path, session_id: &str) {
    let agents_md_path = workspace.join("AGENTS.md");
    let agents_md_content = if agents_md_path.is_file() {
        fs::read_to_string(&agents_md_path).ok()
    } else {
        None
    };
    let value = json!({
        "version": 1,
        "session_id": session_id,
        "created_at_ms": timestamp_ms(),
        "os_name": std::env::consts::OS,
        "shell_path": if cfg!(windows) { "pwsh" } else { "sh" },
        "workspace": workspace.to_string_lossy(),
        "agents_md_present": agents_md_content.is_some(),
        "agents_md_content": agents_md_content,
    });
    let _ = write_json_atomic(&session_dir.join(PROMPT_CONTEXT_FILE_NAME), &value);
}

pub fn list(workspace: &Path) -> Result<Vec<SessionSummary>, String> {
    let mut sessions_map = std::collections::HashMap::new();

    if let Ok(directory) = session_directory(workspace)
        && let Ok(entries) = fs::read_dir(&directory)
    {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir()
                && let Some(id) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(ToString::to_string)
                && validate_session_id(&id).is_ok()
            {
                let summary_file = path.join(SUMMARY_FILE_NAME);
                if let Ok(data) = fs::read_to_string(&summary_file)
                    && let Ok(meta) = serde_json::from_str::<Value>(&data)
                {
                    let bytes = meta.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
                    sessions_map.insert(id.clone(), SessionSummary { id, bytes });
                    continue;
                }
                let file = path.join(SESSION_FILE_NAME);
                if let Ok(meta) = fs::metadata(&file) {
                    sessions_map.insert(
                        id.clone(),
                        SessionSummary {
                            id,
                            bytes: meta.len(),
                        },
                    );
                }
            }
        }
    }

    let legacy_dir = workspace.join(".agents/sessions");
    if let Ok(entries) = fs::read_dir(&legacy_dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir()
                && let Some(id) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(ToString::to_string)
                && validate_session_id(&id).is_ok()
            {
                let summary_file = path.join(SUMMARY_FILE_NAME);
                if let Ok(data) = fs::read_to_string(&summary_file)
                    && let Ok(meta) = serde_json::from_str::<Value>(&data)
                {
                    let bytes = meta.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
                    sessions_map
                        .entry(id.clone())
                        .or_insert_with(|| SessionSummary { id, bytes });
                    continue;
                }
                let file = path.join(SESSION_FILE_NAME);
                if let Ok(meta) = fs::metadata(&file) {
                    sessions_map
                        .entry(id.clone())
                        .or_insert_with(|| SessionSummary {
                            id,
                            bytes: meta.len(),
                        });
                }
            } else if path.is_file()
                && path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(stem) = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(ToString::to_string)
                && validate_session_id(&stem).is_ok()
                && let Ok(meta) = fs::metadata(&path)
            {
                sessions_map
                    .entry(stem.clone())
                    .or_insert_with(|| SessionSummary {
                        id: stem,
                        bytes: meta.len(),
                    });
            }
        }
    }

    let mut sessions: Vec<_> = sessions_map.into_values().collect();
    if sessions.len() > MAX_SESSIONS {
        return Err(format!("session count exceeds {MAX_SESSIONS} limit"));
    }
    sessions.sort_by(|left, right| right.id.cmp(&left.id));
    Ok(sessions)
}

struct LoadedRecords {
    thread_id: String,
    messages: Vec<Message>,
    next_seq: u64,
    checkpoint_seq: u64,
    turn_count: usize,
    thread_turn_count: usize,
    created_at_ms: u64,
    valid_bytes: usize,
}

fn load_records(session_id: &str, bytes: &[u8]) -> Result<LoadedRecords, String> {
    let mut offset = 0usize;
    let mut valid_bytes = 0usize;
    let mut expected_seq = 1u64;
    let mut header_seen = false;
    let mut latest_checkpoint = None;
    let mut turn_count = 0usize;
    let mut thread_turn_counts: HashMap<String, usize> = HashMap::new();
    let mut created_at_ms = 0u64;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let Some(end) = remaining.iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let line = &remaining[..end];
        if line.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "session record exceeds {MAX_RECORD_BYTES} byte limit"
            ));
        }
        let record: Value = serde_json::from_slice(line)
            .map_err(|error| format!("invalid session record at byte {offset}: {error}"))?;
        let seq = record
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("session record at byte {offset} is missing seq"))?;
        if seq != expected_seq {
            return Err(format!(
                "session sequence mismatch: expected {expected_seq}, found {seq}"
            ));
        }
        expected_seq = expected_seq.saturating_add(1);
        match record.get("kind").and_then(Value::as_str) {
            Some("session_created") if !header_seen => {
                let stored_id = record
                    .get("session_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "session header is missing session_id".to_string())?;
                if stored_id != session_id {
                    return Err("session id does not match its file name".to_string());
                }
                if record.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
                    return Err("unsupported session schema version".to_string());
                }
                created_at_ms = record
                    .get("timestamp_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                header_seen = true;
            }
            Some("turn_started") if header_seen => {
                turn_count = turn_count.saturating_add(1);
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "turn_started is missing thread_id".to_string())?;
                let count = thread_turn_counts.entry(thread_id.to_string()).or_insert(0);
                *count = (*count).saturating_add(1);
            }
            Some("checkpoint") if header_seen => {
                let thread_id = record
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "checkpoint is missing thread_id".to_string())?
                    .to_string();
                let messages = serde_json::from_value(
                    record
                        .get("messages")
                        .cloned()
                        .ok_or_else(|| "checkpoint is missing messages".to_string())?,
                )
                .map_err(|error| format!("invalid checkpoint messages: {error}"))?;
                latest_checkpoint = Some((seq, thread_id, messages));
            }
            Some(_) if header_seen => {}
            Some(_) => return Err("session header must be the first record".to_string()),
            None => return Err("session record is missing kind".to_string()),
        }
        offset = offset.saturating_add(end + 1);
        valid_bytes = offset;
    }
    let (checkpoint_seq, thread_id, messages) = latest_checkpoint
        .ok_or_else(|| "session has no settled checkpoint to resume".to_string())?;
    let thread_turn_count = thread_turn_counts.get(&thread_id).copied().unwrap_or(0);
    Ok(LoadedRecords {
        thread_id,
        messages,
        next_seq: expected_seq,
        checkpoint_seq,
        turn_count,
        thread_turn_count,
        created_at_ms,
        valid_bytes,
    })
}

fn acquire_lock(directory: &Path, session_id: &str) -> Result<SessionLock, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create session directory: {error}"))?;
    let path = directory.join(format!("{session_id}.lock"));
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists && reclaim_stale_lock(&path) =>
        {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("cannot lock session {session_id}: {error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "session {session_id} is locked by another process or a stale lock"
            ));
        }
        Err(error) => return Err(format!("cannot lock session {session_id}: {error}")),
    };
    writeln!(
        file,
        "pid={} timestamp_ms={}",
        std::process::id(),
        timestamp_ms()
    )
    .and_then(|()| file.sync_data())
    .map_err(|error| format!("cannot write session lock: {error}"))?;
    Ok(SessionLock(path))
}

fn reclaim_stale_lock(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<u32>().ok())
    else {
        return false;
    };
    if process_exists(pid) {
        return false;
    }
    let stale_path = path.with_extension(format!("stale-{}", timestamp_ms()));
    fs::rename(path, &stale_path)
        .and_then(|()| fs::remove_file(stale_path))
        .is_ok()
}

fn process_exists(pid: u32) -> bool {
    let pid = pid.to_string();
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid))
            .unwrap_or(true)
    } else {
        Command::new("kill")
            .args(["-0", &pid])
            .status()
            .map(|status| status.success())
            .unwrap_or(true)
    }
}

fn copy_attachments(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(src) else {
        return;
    };
    let _ = fs::create_dir_all(dst);
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.is_file() {
            continue;
        }
        let Some(name) = from.file_name() else {
            continue;
        };
        let _ = fs::copy(&from, dst.join(name));
    }
}

pub fn session_directory(workspace: &Path) -> Result<PathBuf, String> {
    let home = mini_agent_home()
        .ok_or_else(|| "cannot resolve home directory for ~/.mini-agent/sessions".to_string())?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace for sessions: {error}"))?;
    let key = percent_encode_path(&display_workspace_path(&workspace));
    if key.is_empty() || key.len() > MAX_WORKSPACE_KEY {
        return Err("workspace path is too long to name a session directory".to_string());
    }
    Ok(home.join("sessions").join(key))
}

pub fn resolve_session_file(
    workspace: &Path,
    session_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let session_dir = session_directory(workspace)?.join(session_id);
    let path = session_dir.join(SESSION_FILE_NAME);
    if path.exists() {
        return Ok((session_dir, path));
    }
    let legacy1 = workspace
        .join(".agents/sessions")
        .join(session_id)
        .join(SESSION_FILE_NAME);
    if legacy1.exists() {
        return Ok((workspace.join(".agents/sessions").join(session_id), legacy1));
    }
    let legacy2 = workspace
        .join(".agents/sessions")
        .join(format!("{session_id}.jsonl"));
    if legacy2.exists() {
        return Ok((workspace.join(".agents/sessions"), legacy2));
    }
    Ok((session_dir, path))
}

fn mini_agent_home() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key)
        .or_else(|| {
            if cfg!(windows) {
                env::var_os("HOME")
            } else {
                None
            }
        })
        .map(|home| PathBuf::from(home).join(".mini-agent"))
}

fn display_workspace_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(&raw)
        .to_string()
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 64
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err("session id must contain 1..=64 ASCII letters, digits, '-' or '_'".to_string())
    } else {
        Ok(())
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

fn new_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}-{:x}-{:x}-{:x}",
        timestamp_ms(),
        std::process::id(),
        counter
    )
}

pub fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn try_load_session_events(
    lines: &[String],
) -> Result<Option<Vec<mini_agent_protocol::Event>>, String> {
    use mini_agent_protocol::Event;
    use mini_agent_protocol::RunFailure;
    use mini_agent_protocol::StopReason;
    use mini_agent_protocol::ToolCall;

    if lines.is_empty() {
        return Ok(None);
    }
    let first_val: Value = match serde_json::from_str(&lines[0]) {
        Ok(val) => val,
        Err(_) => return Ok(None),
    };
    if first_val.get("session_id").is_none() && first_val.get("kind").is_none() {
        return Ok(None);
    }

    let mut events = Vec::new();
    let mut step = 1usize;

    for (line_idx, line) in lines.iter().enumerate() {
        let is_last = line_idx == lines.len() - 1;
        let record: Value = match serde_json::from_str(line) {
            Ok(val) => val,
            Err(_) if is_last => break,
            Err(e) => {
                return Err(format!(
                    "error parsing session record at line {}: {e}",
                    line_idx + 1
                ));
            }
        };
        let kind = record.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        match kind {
            "session_created" => {
                let session_id = record
                    .get("session_id")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if events.is_empty() {
                    events.push(Event::RunStarted {
                        prompt: format!("session created: {session_id}"),
                    });
                }
            }
            "turn_started" => {
                if let Some(prompt) = record.get("prompt").and_then(|p| p.as_str()) {
                    if events.len() == 1
                        && matches!(&events[0], Event::RunStarted { prompt: p } if p.starts_with("session created:"))
                    {
                        events[0] = Event::RunStarted {
                            prompt: prompt.to_string(),
                        };
                    } else {
                        events.push(Event::RunStarted {
                            prompt: prompt.to_string(),
                        });
                    }
                }
            }
            "item" => {
                if let Some(msg_val) = record.get("message")
                    && let Ok(msg) = serde_json::from_value::<Message>(msg_val.clone())
                {
                    match msg {
                        Message::Assistant {
                            reasoning,
                            text,
                            tool_calls,
                        } => {
                            events.push(Event::ModelStarted { step });
                            if !reasoning.is_empty() {
                                events.push(Event::AssistantReasoningDelta {
                                    delta: reasoning.clone(),
                                });
                            }
                            if !text.is_empty() {
                                events.push(Event::AssistantTextDelta {
                                    delta: text.clone(),
                                });
                            }
                            events.push(Event::ModelResponded {
                                reasoning,
                                text,
                                tool_calls,
                                usage: None,
                            });
                            step = step.saturating_add(1);
                        }
                        Message::Tool {
                            call_id,
                            name,
                            content,
                            is_error,
                            outcome,
                        } => {
                            events.push(Event::ToolStarted {
                                call: ToolCall {
                                    id: call_id.clone(),
                                    name: name.clone(),
                                    arguments: serde_json::json!({}),
                                },
                            });
                            events.push(Event::ToolFinished {
                                call_id,
                                name,
                                content,
                                is_error,
                                truncated: false,
                                outcome,
                            });
                        }
                        _ => {}
                    }
                }
            }
            "turn_settled" => {
                let status = record
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("completed");
                let steps = record.get("steps").and_then(|s| s.as_u64()).unwrap_or(1) as usize;
                match status {
                    "failed" => {
                        events.push(Event::RunFailed {
                            reason: RunFailure::Model,
                        });
                    }
                    "step_limit" => {
                        events.push(Event::RunFinished {
                            stop_reason: StopReason::StepLimit,
                            steps,
                        });
                    }
                    "steered" => {
                        events.push(Event::RunFinished {
                            stop_reason: StopReason::Steered,
                            steps,
                        });
                    }
                    _ => {
                        events.push(Event::RunFinished {
                            stop_reason: StopReason::Completed,
                            steps,
                        });
                    }
                }
            }
            "turn_completed" => {
                let steps = record.get("steps").and_then(|s| s.as_u64()).unwrap_or(1) as usize;
                if let Some(messages) = record.get("messages").and_then(|m| m.as_array()) {
                    for msg in messages {
                        if let Ok(msg) = serde_json::from_value::<Message>(msg.clone()) {
                            match msg {
                                Message::Assistant {
                                    reasoning,
                                    text,
                                    tool_calls,
                                } => {
                                    events.push(Event::ModelStarted { step });
                                    if !reasoning.is_empty() {
                                        events.push(Event::AssistantReasoningDelta {
                                            delta: reasoning.clone(),
                                        });
                                    }
                                    if !text.is_empty() {
                                        events.push(Event::AssistantTextDelta {
                                            delta: text.clone(),
                                        });
                                    }
                                    events.push(Event::ModelResponded {
                                        reasoning,
                                        text,
                                        tool_calls,
                                        usage: None,
                                    });
                                    step = step.saturating_add(1);
                                }
                                Message::Tool {
                                    call_id,
                                    name,
                                    content,
                                    is_error,
                                    outcome,
                                } => {
                                    events.push(Event::ToolFinished {
                                        call_id,
                                        name,
                                        content,
                                        is_error,
                                        truncated: false,
                                        outcome,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                events.push(Event::RunFinished {
                    stop_reason: StopReason::Completed,
                    steps,
                });
            }
            "derived" => {
                let summary = record.get("summary").and_then(|s| s.as_str()).unwrap_or("");
                events.push(Event::RunStarted {
                    prompt: "mentor verification".to_string(),
                });
                events.push(Event::ModelStarted { step });
                events.push(Event::AssistantTextDelta {
                    delta: summary.to_string(),
                });
                events.push(Event::ModelResponded {
                    reasoning: String::new(),
                    text: summary.to_string(),
                    tool_calls: vec![],
                    usage: None,
                });
                events.push(Event::RunFinished {
                    stop_reason: StopReason::Completed,
                    steps: 1,
                });
                step = step.saturating_add(1);
            }
            _ => {}
        }
    }

    if events.is_empty() {
        Ok(None)
    } else {
        Ok(Some(events))
    }
}
