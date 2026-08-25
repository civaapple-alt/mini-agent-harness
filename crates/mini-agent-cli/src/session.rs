use mini_agent_core::Message;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

#[path = "session_derived.rs"]
mod derived;
pub(crate) use derived::DerivedItem;

const SCHEMA_VERSION: u64 = 1;
const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_SESSIONS: usize = 128;
const MAX_WORKSPACE_KEY: usize = 240;
const SESSION_FILE_NAME: &str = "session.jsonl";
const SESSION_LOCK_NAME: &str = "session";
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) enum SessionRequest {
    Disabled,
    New,
    Resume(String),
}

pub(crate) struct OpenedSession {
    pub store: SessionStore,
    pub messages: Vec<Message>,
    pub resumed: bool,
}

pub(crate) struct SessionStore {
    session_id: String,
    thread_id: String,
    path: PathBuf,
    file: File,
    bytes: u64,
    next_seq: u64,
    checkpoint_seq: u64,
    _lock: SessionLock,
}

#[derive(Clone, Copy)]
pub(crate) enum TurnStatus {
    Completed,
    StepLimit,
    Failed,
}

pub(crate) struct TurnCommit<'a> {
    pub started_at_ms: u64,
    pub prompt: &'a str,
    pub status: TurnStatus,
    pub steps: usize,
    pub error: Option<&'a str>,
    pub messages: &'a [Message],
    pub checkpoint: &'a [Message],
}

pub(crate) struct SessionSummary {
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
    pub(crate) fn open(workspace: &Path, request: SessionRequest) -> Result<OpenedSession, String> {
        match request {
            SessionRequest::Disabled => Err("session persistence is disabled".to_string()),
            SessionRequest::New => Self::create(workspace),
            SessionRequest::Resume(session_id) => Self::resume(workspace, &session_id),
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn thread_id(&self) -> &str {
        &self.thread_id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn start_thread(&mut self) -> Result<(), String> {
        let thread_id = new_id("t");
        self.append_records(vec![json!({
            "kind": "thread_started",
            "thread_id": thread_id,
            "timestamp_ms": timestamp_ms(),
        })])?;
        self.thread_id = thread_id;
        Ok(())
    }

    pub(crate) fn record_context(
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

    pub(crate) fn record_turn(&mut self, turn: TurnCommit<'_>) -> Result<(), String> {
        let turn_id = new_id("turn");
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
                .map(|message| self.item_record(Some(&turn_id), message)),
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
        Ok(())
    }

    fn create(workspace: &Path) -> Result<OpenedSession, String> {
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
            let mut store = Self {
                session_id: session_id.clone(),
                thread_id: thread_id.clone(),
                path,
                file,
                bytes: 0,
                next_seq: 1,
                checkpoint_seq: 0,
                _lock: lock,
            };
            store.append_records(vec![
                json!({
                    "kind": "session_created",
                    "schema_version": SCHEMA_VERSION,
                    "session_id": session_id,
                    "workspace": workspace,
                    "timestamp_ms": timestamp_ms(),
                }),
                json!({
                    "kind": "thread_started",
                    "thread_id": thread_id,
                    "timestamp_ms": timestamp_ms(),
                }),
            ])?;
            return Ok(OpenedSession {
                store,
                messages: Vec::new(),
                resumed: false,
            });
        }
        Err("cannot allocate a unique session id".to_string())
    }

    fn resume(workspace: &Path, session_id: &str) -> Result<OpenedSession, String> {
        validate_session_id(session_id)?;
        let session_dir = session_directory(workspace)?.join(session_id);
        let path = session_dir.join(SESSION_FILE_NAME);
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
        Ok(OpenedSession {
            store: Self {
                session_id: session_id.to_string(),
                thread_id: loaded.thread_id,
                path,
                file,
                bytes: loaded.valid_bytes as u64,
                next_seq: loaded.next_seq,
                checkpoint_seq: loaded.checkpoint_seq,
                _lock: lock,
            },
            messages: loaded.messages,
            resumed: true,
        })
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
}

impl TurnStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::StepLimit => "step_limit",
            Self::Failed => "failed",
        }
    }
}

pub(crate) fn list(workspace: &Path) -> Result<Vec<SessionSummary>, String> {
    let directory = session_directory(workspace)?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list sessions: {error}")),
    };
    let mut sessions = entries
        .take(MAX_SESSIONS + 1)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let id = path.file_name()?.to_str()?.to_string();
            validate_session_id(&id).ok()?;
            let file = path.join(SESSION_FILE_NAME);
            Some(SessionSummary {
                id,
                bytes: fs::metadata(file).ok()?.len(),
            })
        })
        .collect::<Vec<_>>();
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
    valid_bytes: usize,
}

fn load_records(session_id: &str, bytes: &[u8]) -> Result<LoadedRecords, String> {
    let mut offset = 0usize;
    let mut valid_bytes = 0usize;
    let mut expected_seq = 1u64;
    let mut header_seen = false;
    let mut latest_checkpoint = None;
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
                header_seen = true;
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
    Ok(LoadedRecords {
        thread_id,
        messages,
        next_seq: expected_seq,
        checkpoint_seq,
        valid_bytes,
    })
}

fn acquire_lock(directory: &Path, session_id: &str) -> Result<SessionLock, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create session directory: {error}"))?;
    let path = directory.join(format!("{session_id}.lock"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("session {session_id} is locked by another process or a stale lock")
            } else {
                format!("cannot lock session {session_id}: {error}")
            }
        })?;
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

pub(crate) fn session_directory(workspace: &Path) -> Result<PathBuf, String> {
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

pub(crate) fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
