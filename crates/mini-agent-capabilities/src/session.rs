use mini_agent_core::SessionState;
use mini_agent_protocol::Message;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
pub const THREAD_INDEX_FILE_NAME: &str = "thread_index.json";
pub const THREAD_SETTINGS_FILE_NAME: &str = "thread_settings.json";
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

/// Bounded fork metadata persisted with a newly created child Session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionForkMetadata {
    pub context_policy: String,
    pub context_before_bytes: usize,
    pub context_after_bytes: usize,
    pub compacted: bool,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionForkConflict {
    ParentLineage {
        child_thread_id: String,
    },
    ContextPolicy {
        child_thread_id: String,
        requested_context_policy: String,
        existing_context_policy: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionForkError {
    Conflict(SessionForkConflict),
    Storage(String),
}

impl fmt::Display for SessionForkConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParentLineage { child_thread_id } => write!(
                formatter,
                "child thread id {child_thread_id} is already used by another Session"
            ),
            Self::ContextPolicy {
                child_thread_id,
                requested_context_policy,
                existing_context_policy,
            } => write!(
                formatter,
                "child thread id {child_thread_id} already uses fork context policy '{existing_context_policy}', requested '{requested_context_policy}'"
            ),
        }
    }
}

impl fmt::Display for SessionForkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(conflict) => conflict.fmt(formatter),
            Self::Storage(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for SessionForkError {}

impl SessionForkMetadata {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.context_policy.as_str(), "exact" | "compact") {
            return Err("invalid fork context policy".to_string());
        }
        if !matches!(
            self.method.as_str(),
            "exact" | "model_summary" | "mechanical"
        ) {
            return Err("invalid fork compaction method".to_string());
        }
        if self.context_policy == "exact" && self.compacted {
            return Err("exact fork cannot be marked compacted".to_string());
        }
        Ok(())
    }
}

/// Metadata for a newly persisted independent Session fork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionForkInfo {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub parent_session_id: String,
    pub parent_checkpoint_seq: u64,
    pub session_bytes: u64,
    pub metadata: Option<SessionForkMetadata>,
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
    continuation_mode: Option<String>,
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
    /// Bounded/redacted tool argument projections from the App Server event
    /// stream, keyed by tool call id. Raw model arguments are not persisted.
    pub tool_arguments: &'a [(String, Value)],
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
    /// Optional bounded/redacted arguments for a persisted tool item.
    pub arguments: Option<Value>,
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

    /// Returns the persisted continuation preference for the current Thread.
    ///
    /// The value is intentionally represented as a bounded storage string at
    /// this package boundary. App Server owns the public protocol enum and
    /// validates the value before applying it to the runtime.
    pub fn continuation_mode(&self) -> Option<&str> {
        self.continuation_mode.as_deref()
    }

    /// Persists one explicit Thread continuation preference atomically.
    pub fn set_continuation_mode(&mut self, mode: &str) -> Result<(), String> {
        if !matches!(mode, "manual" | "continuous") {
            return Err("invalid continuation mode".to_string());
        }
        let settings = json!({
            "version": 1,
            "thread_id": self.thread_id.as_str(),
            "continuation_mode": mode,
        });
        write_json_atomic(&self.session_dir.join(THREAD_SETTINGS_FILE_NAME), &settings)?;
        self.continuation_mode = Some(mode.to_string());
        Ok(())
    }

    pub fn result_store(&self) -> crate::result_store::ResultStore {
        crate::result_store::ResultStore::for_session(
            self.path.clone(),
            Arc::clone(&self.append_lock),
        )
    }

    pub fn start_thread(&mut self) -> Result<(), String> {
        let previous_thread_id = self.thread_id.clone();
        let thread_id = new_id("t");
        self.append_records(vec![json!({
            "kind": "thread_started",
            "thread_id": thread_id,
            "timestamp_ms": timestamp_ms(),
        })])?;
        self.thread_id = thread_id;
        self.thread_turn_count = 0;
        self.continuation_mode = None;
        let _ = self.update_thread_index(Some(&previous_thread_id));
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
                let arguments = match message {
                    Message::Tool { call_id, .. } => turn
                        .tool_arguments
                        .iter()
                        .find(|(id, _)| id == call_id)
                        .map(|(_, arguments)| arguments.clone()),
                    _ => None,
                };
                records.push(self.item_record_with_id(
                    Some(turn_id),
                    message,
                    &item_id,
                    arguments.as_ref(),
                ));
                SessionItem {
                    item_id,
                    thread_id: self.thread_id.clone(),
                    turn_id: Some(turn_id.to_string()),
                    message: message.clone(),
                    arguments,
                }
            })
            .collect::<Vec<_>>();
        records.push(json!({
            "kind": "turn_settled",
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "timestamp_ms": timestamp_ms(),
            "status": turn.status.name(),
            "stop_reason": turn.status.stop_reason(),
            "steps": turn.steps,
            "error": turn.error,
        }));
        records.push(self.checkpoint_record(turn.checkpoint));
        self.append_records(records)?;
        self.items.extend(items);
        self.checkpoint_seq = self.next_seq.saturating_sub(1);
        self.turn_count = self.turn_count.saturating_add(1);
        self.thread_turn_count = self.thread_turn_count.saturating_add(1);
        self.update_summary_and_signals(turn.prompt, turn.steps, turn.status, turn.error);
        Ok(())
    }

    fn update_summary_and_signals(
        &self,
        last_prompt: &str,
        steps: usize,
        status: TurnStatus,
        _error: Option<&str>,
    ) {
        let now = timestamp_ms();
        let summary_path = self.session_dir.join(SUMMARY_FILE_NAME);
        let stop_reason = status.stop_reason();
        let summary_val = json!({
            "id": self.session_id,
            "created_at_ms": self.created_at_ms,
            "updated_at_ms": now,
            "turn_count": self.turn_count,
            "bytes": self.bytes,
            "last_prompt": last_prompt,
            "last_status": status.name(),
            "last_stop_reason": stop_reason,
            "last_steps": steps,
            "result_complete": matches!(status, TurnStatus::Completed),
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
        let continuation_mode = load_continuation_mode(&session_dir, &loaded.thread_id);
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
            continuation_mode,
            append_lock: Arc::new(Mutex::new(())),
            _lock: lock,
        };
        let _ = store.update_thread_index(None);
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

    fn existing_fork(
        project_dir: &Path,
        parent_session_id: &str,
        parent_checkpoint_seq: u64,
        child_thread_id: &str,
        context_policy: &str,
    ) -> Result<Option<SessionForkInfo>, SessionForkError> {
        let index_path = project_dir.join(THREAD_INDEX_FILE_NAME);
        let Some(session_id) = fs::read_to_string(&index_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .and_then(|value| value.get("threads").cloned())
            .and_then(|value| value.get(child_thread_id).cloned())
            .and_then(|value| {
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        else {
            return Ok(None);
        };
        validate_session_id(&session_id).map_err(SessionForkError::Storage)?;
        let session_dir = project_dir.join(&session_id);
        let path = session_dir.join(SESSION_FILE_NAME);
        let file = File::open(&path).map_err(|error| {
            SessionForkError::Storage(format!(
                "cannot inspect indexed Session {session_id}: {error}"
            ))
        })?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|error| {
            SessionForkError::Storage(format!("cannot read Session {session_id} header: {error}"))
        })?;
        let header: Value = serde_json::from_str(&line).map_err(|error| {
            SessionForkError::Storage(format!("invalid Session {session_id} header: {error}"))
        })?;
        let forked_from = header.get("forked_from");
        let fork_metadata = forked_from
            .map(Self::read_fork_metadata)
            .transpose()
            .map_err(SessionForkError::Storage)?
            .flatten();
        let matches_parent = forked_from
            .and_then(|value| value.get("parent_session_id"))
            .and_then(Value::as_str)
            == Some(parent_session_id)
            && forked_from
                .and_then(|value| value.get("parent_checkpoint_seq"))
                .and_then(Value::as_u64)
                == Some(parent_checkpoint_seq);
        if !matches_parent {
            return Err(SessionForkError::Conflict(
                SessionForkConflict::ParentLineage {
                    child_thread_id: child_thread_id.to_string(),
                },
            ));
        }
        if let Some(metadata) = fork_metadata.as_ref()
            && metadata.context_policy != context_policy
        {
            return Err(SessionForkError::Conflict(
                SessionForkConflict::ContextPolicy {
                    child_thread_id: child_thread_id.to_string(),
                    requested_context_policy: context_policy.to_string(),
                    existing_context_policy: metadata.context_policy.clone(),
                },
            ));
        }
        line.clear();
        reader.read_line(&mut line).map_err(|error| {
            SessionForkError::Storage(format!("cannot read Session {session_id} thread: {error}"))
        })?;
        let started: Value = serde_json::from_str(&line).map_err(|error| {
            SessionForkError::Storage(format!(
                "invalid Session {session_id} thread record: {error}"
            ))
        })?;
        if started.get("thread_id").and_then(Value::as_str) != Some(child_thread_id) {
            return Err(SessionForkError::Storage(format!(
                "thread index for {child_thread_id} points to a different Session thread"
            )));
        }
        let session_bytes = fs::metadata(&path)
            .map_err(|error| {
                SessionForkError::Storage(format!("cannot stat Session {session_id}: {error}"))
            })?
            .len();
        Ok(Some(SessionForkInfo {
            session_id,
            thread_id: child_thread_id.to_string(),
            path: path.display().to_string(),
            parent_session_id: parent_session_id.to_string(),
            parent_checkpoint_seq,
            session_bytes,
            metadata: fork_metadata,
        }))
    }

    fn read_fork_metadata(forked_from: &Value) -> Result<Option<SessionForkMetadata>, String> {
        let Some(context_policy) = forked_from.get("context_policy") else {
            return Ok(None);
        };
        let context_policy = context_policy
            .as_str()
            .ok_or_else(|| "fork context policy is not a string".to_string())?;
        let context_before_bytes = forked_from
            .get("context_before_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| "fork metadata is missing context_before_bytes".to_string())?;
        let context_after_bytes = forked_from
            .get("context_after_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| "fork metadata is missing context_after_bytes".to_string())?;
        let compacted = forked_from
            .get("compacted")
            .and_then(Value::as_bool)
            .ok_or_else(|| "fork metadata is missing compacted".to_string())?;
        let method = forked_from
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| "fork metadata is missing method".to_string())?;
        let metadata = SessionForkMetadata {
            context_policy: context_policy.to_string(),
            context_before_bytes: usize::try_from(context_before_bytes)
                .map_err(|_| "fork context_before_bytes is too large".to_string())?,
            context_after_bytes: usize::try_from(context_after_bytes)
                .map_err(|_| "fork context_after_bytes is too large".to_string())?,
            compacted,
            method: method.to_string(),
        };
        metadata.validate()?;
        Ok(Some(metadata))
    }

    /// Finds a previously persisted fork without preparing model context.
    pub fn find_fork(
        workspace: &Path,
        parent_session_id: &str,
        parent_checkpoint_seq: u64,
        child_thread_id: &str,
        context_policy: &str,
    ) -> Result<Option<SessionForkInfo>, SessionForkError> {
        validate_session_id(parent_session_id).map_err(SessionForkError::Storage)?;
        validate_session_id(child_thread_id).map_err(SessionForkError::Storage)?;
        if !matches!(context_policy, "exact" | "compact") {
            return Err(SessionForkError::Storage(
                "invalid fork context policy".to_string(),
            ));
        }
        let project_dir = session_directory(workspace).map_err(SessionForkError::Storage)?;
        let _fork_lock =
            acquire_lock(&project_dir, "session-fork").map_err(SessionForkError::Storage)?;
        Ok(Self::existing_fork(
            &project_dir,
            parent_session_id,
            parent_checkpoint_seq,
            child_thread_id,
            context_policy,
        )?
        .filter(|info| info.metadata.is_some()))
    }

    /// Persists a bounded checkpoint as a new independent Session.
    ///
    /// The caller supplies the already-prepared checkpoint. This method only
    /// writes the child header, Thread record, checkpoint, and attachments. It
    /// never rewrites the parent file.
    pub fn fork_from_checkpoint(
        workspace: &Path,
        parent_session_id: &str,
        parent_checkpoint_seq: u64,
        child_thread_id: &str,
        checkpoint: &[Message],
        fork_metadata: SessionForkMetadata,
    ) -> Result<SessionForkInfo, SessionForkError> {
        validate_session_id(parent_session_id).map_err(SessionForkError::Storage)?;
        validate_session_id(child_thread_id).map_err(SessionForkError::Storage)?;
        fork_metadata
            .validate()
            .map_err(SessionForkError::Storage)?;
        let (parent_dir, parent_path) = resolve_session_file(workspace, parent_session_id)
            .map_err(SessionForkError::Storage)?;
        let metadata = fs::metadata(&parent_path).map_err(|error| {
            SessionForkError::Storage(format!(
                "cannot open parent session {parent_session_id}: {error}"
            ))
        })?;
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(SessionForkError::Storage(format!(
                "parent session exceeds {MAX_SESSION_BYTES} byte limit"
            )));
        }

        let project_dir = session_directory(workspace).map_err(SessionForkError::Storage)?;
        // A retry can arrive after the child file is committed but before the
        // caller binds its new client. Serialize the lookup and allocation so
        // the same child thread returns the existing fork instead of creating
        // another durable Session.
        let _fork_lock =
            acquire_lock(&project_dir, "session-fork").map_err(SessionForkError::Storage)?;
        if let Some(existing) = Self::existing_fork(
            &project_dir,
            parent_session_id,
            parent_checkpoint_seq,
            child_thread_id,
            &fork_metadata.context_policy,
        )? {
            return Ok(existing);
        }
        for _ in 0..16 {
            let session_id = new_id("s");
            let session_dir = project_dir.join(&session_id);
            fs::create_dir_all(&session_dir).map_err(|error| {
                SessionForkError::Storage(format!("cannot create session directory: {error}"))
            })?;
            let path = session_dir.join(SESSION_FILE_NAME);
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    let _ = fs::remove_dir_all(&session_dir);
                    return Err(SessionForkError::Storage(format!(
                        "cannot create session: {error}"
                    )));
                }
            };
            let lock = match acquire_lock(&session_dir, SESSION_LOCK_NAME) {
                Ok(lock) => lock,
                Err(error) => {
                    let _ = fs::remove_dir_all(&session_dir);
                    return Err(SessionForkError::Storage(error));
                }
            };
            let initialized = Self::initialize_new_for_thread(
                workspace,
                &session_id,
                session_dir.clone(),
                path,
                file,
                lock,
                child_thread_id,
                checkpoint,
                Some((parent_session_id, parent_checkpoint_seq)),
                Some(&fork_metadata),
            );
            let store = match initialized {
                Ok(store) => store,
                Err(error) => {
                    let _ = fs::remove_dir_all(&session_dir);
                    return Err(SessionForkError::Storage(error));
                }
            };
            copy_attachments(
                &parent_dir.join("attachments"),
                &session_dir.join("attachments"),
            );
            let info = SessionForkInfo {
                session_id: store.session_id.clone(),
                thread_id: store.thread_id.clone(),
                path: store.path.display().to_string(),
                parent_session_id: parent_session_id.to_string(),
                parent_checkpoint_seq,
                session_bytes: store.bytes,
                metadata: Some(fork_metadata.clone()),
            };
            drop(store);
            return Ok(info);
        }
        Err(SessionForkError::Storage(
            "cannot allocate a unique session id".to_string(),
        ))
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
        let thread_id = env::var("MINI_AGENT_THREAD_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| new_id("t"));
        Self::initialize_new_for_thread(
            workspace,
            session_id,
            session_dir,
            path,
            file,
            lock,
            &thread_id,
            checkpoint,
            forked_from,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn initialize_new_for_thread(
        workspace: &Path,
        session_id: &str,
        session_dir: PathBuf,
        path: PathBuf,
        file: File,
        lock: SessionLock,
        thread_id: &str,
        checkpoint: &[Message],
        forked_from: Option<(&str, u64)>,
        fork_metadata: Option<&SessionForkMetadata>,
    ) -> Result<Self, String> {
        let now = timestamp_ms();
        let mut store = Self {
            session_id: session_id.to_string(),
            thread_id: thread_id.to_string(),
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
            continuation_mode: None,
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
            let mut lineage = json!({
                "parent_session_id": parent_session_id,
                "parent_checkpoint_seq": parent_checkpoint_seq,
            });
            if let Some(metadata) = fork_metadata {
                lineage["context_policy"] = json!(metadata.context_policy.as_str());
                lineage["context_before_bytes"] = json!(metadata.context_before_bytes);
                lineage["context_after_bytes"] = json!(metadata.context_after_bytes);
                lineage["compacted"] = json!(metadata.compacted);
                lineage["method"] = json!(metadata.method.as_str());
            }
            header["forked_from"] = lineage;
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
        store.update_summary_and_signals("", 0, TurnStatus::Completed, None);
        let _ = store.update_thread_index(None);
        Ok(store)
    }

    fn update_thread_index(&self, previous_thread_id: Option<&str>) -> Result<(), String> {
        let base_dir = self
            .session_dir
            .parent()
            .ok_or_else(|| "session directory has no workspace parent".to_string())?;
        fs::create_dir_all(base_dir)
            .map_err(|error| format!("cannot create thread index directory: {error}"))?;
        let _lock = acquire_lock(base_dir, "thread-index")?;
        let index_path = base_dir.join(THREAD_INDEX_FILE_NAME);
        let mut threads = fs::read_to_string(&index_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .and_then(|value| value.get("threads").cloned())
            .and_then(|value| serde_json::from_value::<HashMap<String, Value>>(value).ok())
            .unwrap_or_default();
        if let Some(previous_thread_id) = previous_thread_id {
            let owned_by_this_session = threads
                .get(previous_thread_id)
                .and_then(|value| value.get("session_id"))
                .and_then(Value::as_str)
                == Some(self.session_id.as_str());
            if owned_by_this_session {
                threads.remove(previous_thread_id);
            }
        }
        threads.insert(
            self.thread_id.clone(),
            json!({
                "session_id": self.session_id.clone(),
                "updated_at_ms": timestamp_ms(),
            }),
        );
        write_json_atomic(
            &index_path,
            &json!({
                "version": 1,
                "threads": threads,
            }),
        )
    }

    fn item_record(&self, turn_id: Option<&str>, message: &Message) -> Value {
        let item_id = item_id_for_message(message);
        self.item_record_with_id(turn_id, message, &item_id, None)
    }

    fn item_record_with_id(
        &self,
        turn_id: Option<&str>,
        message: &Message,
        item_id: &str,
        arguments: Option<&Value>,
    ) -> Value {
        let mut record = json!({
            "kind": "item",
            "item_id": item_id,
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "item_kind": persisted_item_kind(turn_id, message),
            "timestamp_ms": timestamp_ms(),
            "message": message,
        });
        if let Some(arguments) = arguments
            && let Some(object) = record.as_object_mut()
        {
            object.insert("arguments".to_string(), arguments.clone());
        }
        record
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

fn load_continuation_mode(session_dir: &Path, thread_id: &str) -> Option<String> {
    let value = fs::read_to_string(session_dir.join(THREAD_SETTINGS_FILE_NAME))
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("thread_id").and_then(Value::as_str) != Some(thread_id)
    {
        return None;
    }
    match value.get("continuation_mode").and_then(Value::as_str) {
        Some("manual") | Some("continuous") => value
            .get("continuation_mode")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
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

    fn stop_reason(self) -> Option<&'static str> {
        match self {
            Self::Completed => None,
            Self::StepLimit => Some("step_limit"),
            Self::Steered => Some("steered"),
            Self::Cancelled => Some("cancelled"),
            Self::Failed => Some("failed"),
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

fn persisted_item_kind(turn_id: Option<&str>, message: &Message) -> &'static str {
    if matches!(message, Message::Context { .. }) && turn_id.is_some() {
        "context_compaction"
    } else {
        message_kind(message)
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

    fn exact_fork_metadata() -> SessionForkMetadata {
        SessionForkMetadata {
            context_policy: "exact".to_string(),
            context_before_bytes: 128,
            context_after_bytes: 128,
            compacted: false,
            method: "exact".to_string(),
        }
    }

    fn compact_fork_metadata() -> SessionForkMetadata {
        SessionForkMetadata {
            context_policy: "compact".to_string(),
            context_before_bytes: 128,
            context_after_bytes: 64,
            compacted: true,
            method: "mechanical".to_string(),
        }
    }

    #[test]
    fn item_index_survives_session_resume() {
        let root = crate::test_support::test_root();
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
                    tool_arguments: &[],
                    checkpoint: &messages,
                },
            )
            .unwrap();
        assert_eq!(opened.store.items().len(), 2);
        let index_path = opened
            .store
            .path()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(THREAD_INDEX_FILE_NAME);
        let index = fs::read_to_string(index_path).unwrap();
        assert!(index.contains(opened.store.thread_id()));
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert_eq!(resumed.store.items().len(), 2);
        assert_eq!(resumed.store.items()[0].turn_id.as_deref(), Some("turn-1"));
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn context_in_a_turn_is_persisted_as_a_compaction_with_turn_id() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let context = Message::Context {
            text: "compacted context".to_string(),
        };
        opened
            .store
            .record_turn_with_id(
                "turn-compaction",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "continue",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: std::slice::from_ref(&context),
                    tool_arguments: &[],
                    checkpoint: std::slice::from_ref(&context),
                },
            )
            .unwrap();

        assert_eq!(
            opened.store.items()[0].turn_id.as_deref(),
            Some("turn-compaction")
        );
        let session_text = fs::read_to_string(opened.store.path()).unwrap();
        assert!(session_text.contains("\"item_kind\":\"context_compaction\""));
        assert!(session_text.contains("\"turn_id\":\"turn-compaction\""));
        drop(opened);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn fork_from_checkpoint_creates_an_independent_session_and_preserves_parent() {
        let root = crate::test_support::test_root();
        let mut parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let parent_id = parent.store.session_id().to_string();
        let messages = vec![
            Message::User {
                text: "parent question".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "parent answer".to_string(),
                tool_calls: Vec::new(),
            },
        ];
        parent
            .store
            .record_turn_with_id(
                "turn-1",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "parent question",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &messages,
                    tool_arguments: &[],
                    checkpoint: &messages,
                },
            )
            .unwrap();
        let parent_bytes = fs::read(parent.store.path()).unwrap();
        let info = SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            parent.store.checkpoint_seq(),
            "child-thread",
            &messages,
            exact_fork_metadata(),
        )
        .unwrap();

        assert_ne!(info.session_id, parent_id);
        assert_eq!(info.thread_id, "child-thread");
        assert_eq!(info.parent_session_id, parent_id);
        assert_eq!(info.metadata, Some(exact_fork_metadata()));
        assert!(
            fs::read_to_string(&info.path)
                .unwrap()
                .contains("\"context_policy\":\"exact\"")
        );
        assert_eq!(fs::read(parent.store.path()).unwrap(), parent_bytes);
        let child = SessionStore::open(&root, SessionRequest::Resume(info.session_id)).unwrap();
        assert_eq!(child.store.thread_id(), "child-thread");
        assert_eq!(child.state, SessionState::from_messages(messages));
        drop(child);
        drop(parent);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn retrying_the_same_fork_returns_the_existing_session() {
        let root = crate::test_support::test_root();
        let mut parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let parent_id = parent.store.session_id().to_string();
        let messages = vec![Message::User {
            text: "parent question".to_string(),
        }];
        parent
            .store
            .record_turn_with_id(
                "turn-1",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "parent question",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &messages,
                    tool_arguments: &[],
                    checkpoint: &messages,
                },
            )
            .unwrap();
        let checkpoint_seq = parent.store.checkpoint_seq();
        let first = SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            checkpoint_seq,
            "child-thread",
            &messages,
            exact_fork_metadata(),
        )
        .unwrap();
        let retry = SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            checkpoint_seq,
            "child-thread",
            &messages,
            exact_fork_metadata(),
        )
        .unwrap();

        assert_eq!(retry, first);
        let conflict = SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            checkpoint_seq,
            "child-thread",
            &messages,
            compact_fork_metadata(),
        )
        .unwrap_err();
        assert_eq!(
            conflict,
            SessionForkError::Conflict(SessionForkConflict::ContextPolicy {
                child_thread_id: "child-thread".to_string(),
                requested_context_policy: "compact".to_string(),
                existing_context_policy: "exact".to_string(),
            })
        );
        let project_dir = session_directory(&root).unwrap();
        let child_sessions = fs::read_dir(project_dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy() == first.session_id)
            .count();
        assert_eq!(child_sessions, 1);
        drop(parent);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn fork_rejects_reusing_a_child_thread_for_another_checkpoint() {
        let root = crate::test_support::test_root();
        let mut parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let parent_id = parent.store.session_id().to_string();
        let first_messages = vec![Message::User {
            text: "first".to_string(),
        }];
        parent
            .store
            .record_turn_with_id(
                "turn-1",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "first",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &first_messages,
                    tool_arguments: &[],
                    checkpoint: &first_messages,
                },
            )
            .unwrap();
        let first_checkpoint = parent.store.checkpoint_seq();
        SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            first_checkpoint,
            "child-thread",
            &first_messages,
            exact_fork_metadata(),
        )
        .unwrap();
        let second_messages = vec![Message::User {
            text: "second".to_string(),
        }];
        parent
            .store
            .record_turn_with_id(
                "turn-2",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "second",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &second_messages,
                    tool_arguments: &[],
                    checkpoint: &second_messages,
                },
            )
            .unwrap();
        let error = SessionStore::fork_from_checkpoint(
            &root,
            &parent_id,
            parent.store.checkpoint_seq(),
            "child-thread",
            &second_messages,
            exact_fork_metadata(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            SessionForkError::Conflict(SessionForkConflict::ParentLineage {
                child_thread_id: "child-thread".to_string(),
            })
        );
        drop(parent);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn tool_argument_projection_survives_session_resume() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let messages = vec![
            Message::User {
                text: "inspect".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![mini_agent_protocol::ToolCall {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    arguments: serde_json::json!({"command": "Get-ChildItem"}),
                }],
            },
            Message::Tool {
                call_id: "call-1".to_string(),
                name: "shell".to_string(),
                content: "exit: 0\nstdout:\nfile.txt\nstderr:\n".to_string(),
                is_error: false,
                outcome: Some(mini_agent_protocol::ToolExecutionStatus::Completed),
            },
        ];
        let arguments = vec![(
            "call-1".to_string(),
            serde_json::json!({"command": "Get-ChildItem", "token": "[REDACTED]"}),
        )];
        opened
            .store
            .record_turn_with_id(
                "turn-1",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "inspect",
                    status: TurnStatus::Completed,
                    steps: 2,
                    error: None,
                    messages: &messages,
                    tool_arguments: &arguments,
                    checkpoint: &messages,
                },
            )
            .unwrap();
        assert_eq!(
            opened.store.items()[2].arguments,
            Some(arguments[0].1.clone())
        );
        let session_text = fs::read_to_string(opened.store.path()).unwrap();
        assert!(session_text.contains("Get-ChildItem"));
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert_eq!(
            resumed.store.items()[2].arguments,
            Some(arguments[0].1.clone())
        );
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn continuation_preference_survives_session_resume() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();

        assert_eq!(opened.store.continuation_mode(), None);
        opened.store.set_continuation_mode("continuous").unwrap();
        assert_eq!(opened.store.continuation_mode(), Some("continuous"));
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert_eq!(resumed.store.continuation_mode(), Some("continuous"));
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }
}
