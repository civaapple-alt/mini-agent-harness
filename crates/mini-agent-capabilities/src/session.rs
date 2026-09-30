use crate::skills::MAX_SELECTED_SKILLS;
use mini_agent_core::ExecutionCheckpoint;
use mini_agent_core::ExecutionJournalEntry;
use mini_agent_core::ExecutionJournalSink;
use mini_agent_core::ExecutionPhase;
use mini_agent_core::ExecutionToolBatch;
use mini_agent_core::ExecutionToolCall;
use mini_agent_core::SessionState;
use mini_agent_protocol::{
    ChildTaskAttemptKind, ContextByteBreakdown, ContextInjectionRecord, Message, ModelSelection,
    ModelUsage, ReasoningSelection, TurnId, TurnSource, TurnWorkflow,
};
use serde::{Deserialize, Serialize};
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

#[path = "session/diagnostics.rs"]
mod diagnostics;
#[path = "session/storage.rs"]
mod storage;
pub use diagnostics::{
    SESSION_DOCTOR_SCHEMA_VERSION, SessionDiagnosticCounts, SessionDiagnosticFinding,
    SessionDiagnosticIssue, SessionDiagnosticReport, SessionInspectionCounts,
    SessionInspectionStatus, SessionIntegrityCounts, SessionIntegrityStatus, SessionRecoveryCounts,
    SessionRecoveryStatus, SessionRepairResult,
};
use storage::{
    acquire_lock, copy_attachments, load_records, validate_session_id, write_json_atomic,
    write_prompt_context,
};
pub use storage::{resolve_session_file, session_directory};

const SCHEMA_VERSION: u64 = 1;
const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const MAX_WORKSPACE_KEY: usize = 240;
const MAX_OPERATION_ID_BYTES: usize = 128;
const MAX_OPERATION_KIND_BYTES: usize = 64;
const MAX_OPERATION_ERROR_BYTES: usize = 4096;
const MAX_OPERATION_RESULT_BYTES: usize = 16 * 1024;
const MAX_OPERATION_PROMPT_BYTES: usize = 32 * 1024;
const MAX_CHILD_REPORT_BYTES: usize = 4 * 1024;
const MAX_SESSION_CONTROL_BYTES: u64 = 16 * 1024;
const MAX_CHILD_REPORT_RECEIPTS: usize = 4096;
const MAX_CHILD_REPORT_RECEIPTS_BYTES: u64 = 1024 * 1024;
/// A single Turn can activate a bounded number of explicit skills, and may
/// additionally load a small number of skills on demand. Keep the replay
/// projection bounded independently of the raw event stream.
pub const MAX_TURN_PRESENTATION_ACTIVITIES: usize = 32;
const MAX_TURN_PRESENTATION_VALUE_CHARS: usize = 256;
const SESSION_FILE_NAME: &str = "session.jsonl";
const SESSION_LOCK_NAME: &str = "session";
pub const SUMMARY_FILE_NAME: &str = "summary.json";
pub const SIGNALS_FILE_NAME: &str = "signals.json";
pub const PROMPT_CONTEXT_FILE_NAME: &str = "prompt_context.json";
pub const THREAD_INDEX_FILE_NAME: &str = "thread_index.json";
pub const THREAD_SETTINGS_FILE_NAME: &str = "thread_settings.json";
pub const SESSION_CONTROL_FILE_NAME: &str = "session_control.json";
pub const CHILD_REPORT_RECEIPTS_FILE_NAME: &str = "child_report_receipts.json";
const CHILD_REPORT_RECEIPTS_LOCK_NAME: &str = "child_report_receipts";
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
    turn_sources: HashMap<String, TurnSource>,
    is_forked: bool,
    created_at_ms: u64,
    continuation_mode: Option<String>,
    model_selection: Option<ModelSelection>,
    reasoning_selection: Option<ReasoningSelection>,
    pub(crate) append_lock: Arc<Mutex<()>>,
    execution_state: Arc<Mutex<Option<SessionExecutionState>>>,
    _lock: SessionLock,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionExecutionStatus {
    Running,
    WaitingForContinue,
    NeedsReconciliation,
    Settled,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionResumeReservation {
    Accepted(Box<SessionExecutionState>),
    AlreadyAccepted,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExecutionState {
    pub checkpoint: ExecutionCheckpoint,
    pub checkpoint_seq: u64,
    pub status: SessionExecutionStatus,
    pub phase: ExecutionPhase,
    pub last_heartbeat_ms: Option<u64>,
    pub last_progress_ms: Option<u64>,
    pub reason: Option<String>,
    pub pending_batch: Option<ExecutionToolBatch>,
    #[serde(default)]
    pub resume_requests: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct SessionExecutionJournal {
    session_id: String,
    thread_id: String,
    path: PathBuf,
    append_lock: Arc<Mutex<()>>,
    execution_state: Arc<Mutex<Option<SessionExecutionState>>>,
    writer_state: Arc<Mutex<ExecutionJournalWriterState>>,
}

#[derive(Default)]
struct ExecutionJournalWriterState {
    turn_id: Option<TurnId>,
    messages: Vec<Message>,
    checkpoint_seq: u64,
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
    /// Host-owned, bounded display metadata. This is not Core conversation
    /// state; it lets a client replay the same workflow/skill milestones after
    /// the App Server process has restarted.
    pub presentation: Option<&'a TurnPresentation>,
    pub checkpoint: &'a [Message],
}

/// Per-turn display metadata derived from the App Server event stream.
///
/// The SessionStore keeps this projection next to the Turn rather than
/// retaining arbitrary observer events. `after_assistant_segments` preserves
/// the visible placement of each skill milestone without making the durable
/// session log a second event stream.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_source: Option<TurnSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow: Option<TurnWorkflow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    activities: Vec<TurnPresentationActivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_usage: Option<TurnContextUsage>,
}

/// The last model request observed during a Turn, with byte data suitable for
/// clearly-labeled proportional token estimates in the client.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnContextUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_bytes: Option<ContextByteBreakdown>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPresentationActivityKind {
    SkillGroupActivated,
    SkillsLoaded,
    SkillsLoadFailed,
    ContextInjected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPresentationActivity {
    after_assistant_segments: u32,
    kind: TurnPresentationActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    context_injections: Vec<ContextInjectionRecord>,
}

impl TurnPresentation {
    pub fn from_workflow(workflow: Option<&TurnWorkflow>) -> Self {
        Self {
            turn_source: None,
            workflow: workflow.map(bounded_workflow),
            activities: Vec::new(),
            context_usage: None,
        }
    }

    pub fn with_turn_source(mut self, source: Option<TurnSource>) -> Self {
        self.turn_source = source;
        self
    }

    pub fn push(&mut self, activity: TurnPresentationActivity) {
        if self.activities.len() < MAX_TURN_PRESENTATION_ACTIVITIES {
            self.activities.push(activity);
        }
    }

    pub fn set_context_usage(
        &mut self,
        usage: Option<ModelUsage>,
        context_bytes: Option<ContextByteBreakdown>,
    ) {
        self.context_usage = Some(TurnContextUsage {
            usage,
            context_bytes,
        });
    }
}

impl TurnPresentationActivity {
    pub fn skill_group_activated(after_assistant_segments: u32, group: &str, source: &str) -> Self {
        Self {
            after_assistant_segments,
            kind: TurnPresentationActivityKind::SkillGroupActivated,
            group: Some(bounded_presentation_value(group)),
            source: Some(bounded_presentation_value(source)),
            phase: None,
            activation: None,
            skills: Vec::new(),
            reason_code: None,
            context_injections: Vec::new(),
        }
    }

    pub fn skills_loaded(
        after_assistant_segments: u32,
        phase: &str,
        activation: Option<&str>,
        skills: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            after_assistant_segments,
            kind: TurnPresentationActivityKind::SkillsLoaded,
            group: None,
            source: None,
            phase: Some(bounded_presentation_value(phase)),
            activation: activation.map(bounded_presentation_value),
            skills: bounded_skill_names(skills),
            reason_code: None,
            context_injections: Vec::new(),
        }
    }

    pub fn skills_load_failed(
        after_assistant_segments: u32,
        activation: Option<&str>,
        skills: impl IntoIterator<Item = String>,
        reason_code: &str,
    ) -> Self {
        Self {
            after_assistant_segments,
            kind: TurnPresentationActivityKind::SkillsLoadFailed,
            group: None,
            source: None,
            phase: None,
            activation: activation.map(bounded_presentation_value),
            skills: bounded_skill_names(skills),
            reason_code: Some(bounded_presentation_value(reason_code)),
            context_injections: Vec::new(),
        }
    }

    pub fn context_injected(
        after_assistant_segments: u32,
        records: impl IntoIterator<Item = ContextInjectionRecord>,
    ) -> Self {
        Self {
            after_assistant_segments,
            kind: TurnPresentationActivityKind::ContextInjected,
            group: None,
            source: None,
            phase: None,
            activation: None,
            skills: Vec::new(),
            reason_code: None,
            context_injections: records
                .into_iter()
                .take(32)
                .map(bounded_context_injection)
                .collect(),
        }
    }
}

fn bounded_context_injection(mut record: ContextInjectionRecord) -> ContextInjectionRecord {
    record.id = bounded_presentation_value(&record.id);
    record.source = bounded_presentation_value(&record.source);
    record.workspace = record
        .workspace
        .map(|value| bounded_presentation_value(&value));
    record.path = record.path.map(|value| bounded_presentation_value(&value));
    record.scope = bounded_presentation_value(&record.scope);
    record.fingerprint = bounded_presentation_value(&record.fingerprint);
    record.supersedes = record
        .supersedes
        .map(|value| bounded_presentation_value(&value));
    record
}

fn bounded_workflow(workflow: &TurnWorkflow) -> TurnWorkflow {
    TurnWorkflow {
        kind: workflow.kind,
        id: bounded_presentation_value(&workflow.id),
        mode: workflow.mode,
    }
}

fn bounded_skill_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .take(MAX_SELECTED_SKILLS)
        .map(|name| bounded_presentation_value(&name))
        .collect()
}

fn bounded_presentation_value(value: &str) -> String {
    value
        .chars()
        .take(MAX_TURN_PRESENTATION_VALUE_CHARS)
        .collect()
}

/// A bounded Host-owned lifecycle record for work that outlives one Turn.
///
/// This is deliberately a Session record rather than Core state. It lets an
/// App Server rebuild child-task status after a process restart without adding
/// a second scheduler or mutating the Core run loop.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SessionOperation {
    pub operation_id: String,
    #[serde(rename = "operation_kind")]
    pub kind: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_kind: Option<ChildTaskAttemptKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_source: Option<ChildTaskControlSource>,
    #[serde(
        rename = "operation_group_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    #[serde(
        rename = "group_sequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sequence: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildTaskControlSource {
    MainAgent,
    UserPanel,
    ParentFreeze,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionControlStatus {
    Running,
    Freezing,
    Frozen,
    Resuming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionControlAction {
    Freeze,
    FreezeSettled,
    Resume,
    ResumeSettled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionControlState {
    pub session_id: String,
    pub status: SessionControlStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildReportReceipt {
    pub child_thread_id: String,
    pub operation_id: String,
    pub attempt: u32,
    pub cursor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildTaskContext {
    pub parent_thread_id: String,
    pub operation_id: String,
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildTaskMutationResult {
    pub status: String,
    pub cursor: u64,
    pub attempt: u32,
    pub attempt_kind: Option<ChildTaskAttemptKind>,
    pub turn_id: Option<String>,
    pub duplicate: bool,
    pub request_action: Option<ChildControlRequestAction>,
    pub timestamp_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildControlRequestAction {
    Steer,
    QueueFollowUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildSteerRequestStep {
    Reserve,
    Accept,
    NotAccepted,
}

impl SessionOperation {
    pub fn new(
        operation_id: impl Into<String>,
        kind: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            kind: kind.into(),
            status: status.into(),
            parent_thread_id: None,
            turn_id: None,
            attempt: 1,
            attempt_kind: None,
            control_request_id: None,
            control_action: None,
            control_source: None,
            group_id: None,
            execution_mode: None,
            sequence: None,
            prompt: None,
            result: None,
            error: None,
            timestamp_ms: timestamp_ms(),
        }
    }

    /// Bounds model-produced results to the size accepted by the durable
    /// operation record. Line breaks and tabs are preserved; other control
    /// characters are replaced so one unusual response cannot strand the
    /// operation in a nonterminal state.
    pub fn bounded_result(value: &str) -> String {
        let mut result = String::with_capacity(value.len().min(MAX_OPERATION_RESULT_BYTES));
        for character in value.chars() {
            let character = if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            };
            if result.len() + character.len_utf8() > MAX_OPERATION_RESULT_BYTES {
                break;
            }
            result.push(character);
        }
        result
    }

    fn validate(&self) -> Result<(), String> {
        validate_operation_text(&self.operation_id, MAX_OPERATION_ID_BYTES, "operation id")?;
        validate_operation_text(&self.kind, MAX_OPERATION_KIND_BYTES, "operation kind")?;
        if !matches!(
            self.status.as_str(),
            "queued"
                | "running"
                | "awaiting_approval"
                | "pausing"
                | "cancelling"
                | "paused"
                | "completed"
                | "failed"
                | "cancelled"
        ) {
            return Err("invalid operation status".to_string());
        }
        if self.attempt == 0 {
            return Err("operation attempt must be positive".to_string());
        }
        if let Some(request_id) = self.control_request_id.as_deref() {
            validate_operation_text(request_id, 192, "control request id")?;
        }
        if let Some(group_id) = self.group_id.as_deref() {
            validate_operation_text(group_id, MAX_OPERATION_ID_BYTES, "operation group id")?;
        }
        if let Some(execution_mode) = self.execution_mode.as_deref()
            && !matches!(execution_mode, "parallel" | "sequential")
        {
            return Err("invalid operation execution mode".to_string());
        }
        for (value, limit, label) in [
            (
                self.parent_thread_id.as_deref(),
                MAX_OPERATION_ID_BYTES,
                "parent thread id",
            ),
            (self.turn_id.as_deref(), MAX_OPERATION_ID_BYTES, "turn id"),
            (
                self.error.as_deref(),
                MAX_OPERATION_ERROR_BYTES,
                "operation error",
            ),
        ] {
            if let Some(value) = value {
                validate_operation_text(value, limit, label)?;
            }
        }
        if let Some(result) = self.result.as_deref() {
            validate_operation_result(result)?;
        }
        if let Some(prompt) = self.prompt.as_deref() {
            validate_operation_prompt(prompt)?;
        }
        Ok(())
    }
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

    pub fn turn_source(&self, turn_id: &str) -> Option<TurnSource> {
        self.turn_sources.get(turn_id).copied()
    }

    pub fn is_forked(&self) -> bool {
        self.is_forked
    }

    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    pub fn execution_state(&self) -> Option<SessionExecutionState> {
        self.execution_state.lock().unwrap().clone()
    }

    /// Reserves one explicit recovery request before any execution is started.
    pub fn reserve_execution_resume(
        &self,
        turn_id: &TurnId,
        checkpoint_seq: u64,
        request_id: &str,
    ) -> Result<ExecutionResumeReservation, String> {
        if request_id.trim().is_empty() || request_id.len() > 128 {
            return Err("resume request id must be non-empty and at most 128 bytes".to_string());
        }
        let state = self
            .execution_state()
            .ok_or_else(|| "no execution checkpoint is available".to_string())?;
        if state.checkpoint.turn_id != *turn_id {
            return Err("resume request does not match the current execution Turn".to_string());
        }
        if state.checkpoint_seq != checkpoint_seq {
            return Err(format!(
                "execution checkpoint is stale: expected {}, current {}",
                checkpoint_seq, state.checkpoint_seq
            ));
        }
        if let Some(previous_seq) = state.resume_requests.get(request_id) {
            if *previous_seq == checkpoint_seq {
                if state.status != SessionExecutionStatus::WaitingForContinue {
                    return Ok(ExecutionResumeReservation::AlreadyAccepted);
                }
                // The App Server may have crashed after durably admitting this
                // request but before the worker began. Startup changes Running
                // back to WaitingForContinue; the same id must then be able to
                // start the original Turn exactly once again.
                let mut journal = self.execution_journal(&state.checkpoint.messages);
                journal.append(ExecutionJournalEntry::Resumed {
                    turn_id: turn_id.clone(),
                    request_id: request_id.to_string(),
                    checkpoint_seq,
                })?;
                return Ok(ExecutionResumeReservation::Accepted(Box::new(state)));
            }
            return Err("resume request id was already used for another checkpoint".to_string());
        }
        if state.status != SessionExecutionStatus::WaitingForContinue {
            return Err(format!(
                "execution cannot resume while status is {:?}",
                state.status
            ));
        }
        let mut journal = self.execution_journal(&state.checkpoint.messages);
        journal.append(ExecutionJournalEntry::Resumed {
            turn_id: turn_id.clone(),
            request_id: request_id.to_string(),
            checkpoint_seq,
        })?;
        Ok(ExecutionResumeReservation::Accepted(Box::new(state)))
    }

    pub fn execution_journal(&self, base_messages: &[Message]) -> SessionExecutionJournal {
        let state = self.execution_state();
        SessionExecutionJournal {
            session_id: self.session_id.clone(),
            thread_id: self.thread_id.clone(),
            path: self.path.clone(),
            append_lock: Arc::clone(&self.append_lock),
            execution_state: Arc::clone(&self.execution_state),
            writer_state: Arc::new(Mutex::new(ExecutionJournalWriterState {
                turn_id: state.as_ref().map(|state| state.checkpoint.turn_id.clone()),
                messages: state
                    .as_ref()
                    .map(|state| state.checkpoint.messages.clone())
                    .unwrap_or_else(|| base_messages.to_vec()),
                checkpoint_seq: state.map_or(0, |state| state.checkpoint_seq),
            })),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session_control(&self) -> Result<SessionControlState, String> {
        read_session_control(&self.session_dir, &self.session_id)
    }

    pub fn transition_session_control(
        &mut self,
        action: SessionControlAction,
        request_id: &str,
    ) -> Result<SessionControlState, String> {
        validate_operation_text(request_id, 192, "session control request id")?;
        let mut state = self.session_control()?;
        match action {
            SessionControlAction::Freeze => {
                if matches!(
                    state.status,
                    SessionControlStatus::Freezing | SessionControlStatus::Frozen
                ) {
                    return Ok(state);
                }
                state.status = SessionControlStatus::Freezing;
                state.request_id = Some(request_id.to_string());
            }
            SessionControlAction::FreezeSettled => {
                if state.status == SessionControlStatus::Frozen {
                    return Ok(state);
                }
                if state.status != SessionControlStatus::Freezing
                    || state.request_id.as_deref() != Some(request_id)
                {
                    return Err("session freeze request is stale".to_string());
                }
                state.status = SessionControlStatus::Frozen;
            }
            SessionControlAction::Resume => {
                if matches!(state.status, SessionControlStatus::Running) {
                    return Ok(state);
                }
                if state.status == SessionControlStatus::Resuming {
                    return Ok(state);
                }
                if state.status != SessionControlStatus::Frozen {
                    return Err("Session freeze has not settled yet".to_string());
                }
                state.status = SessionControlStatus::Resuming;
                state.request_id = Some(request_id.to_string());
            }
            SessionControlAction::ResumeSettled => {
                if state.status == SessionControlStatus::Running {
                    return Ok(state);
                }
                if state.status != SessionControlStatus::Resuming
                    || state.request_id.as_deref() != Some(request_id)
                {
                    return Err("session resume request is stale".to_string());
                }
                state.status = SessionControlStatus::Running;
            }
        }
        state.updated_at_ms = timestamp_ms();
        persist_session_control(&self.session_dir, &state)?;
        Ok(state)
    }

    pub fn read_child_report_receipts(
        session_dir: &Path,
    ) -> Result<Vec<ChildReportReceipt>, String> {
        read_child_report_receipts(session_dir)
    }

    pub fn record_child_report_receipt(
        session_dir: &Path,
        receipt: ChildReportReceipt,
    ) -> Result<(), String> {
        validate_child_report_receipt(&receipt)?;
        let session_id = session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "parent Session identity is unavailable".to_string())?;
        validate_session_id(session_id)?;
        let _lock = acquire_lock(session_dir, CHILD_REPORT_RECEIPTS_LOCK_NAME)?;
        let mut receipts = read_child_report_receipts(session_dir)?;
        if let Some(existing) = receipts.iter_mut().find(|existing| {
            existing.child_thread_id == receipt.child_thread_id
                && existing.operation_id == receipt.operation_id
                && existing.attempt == receipt.attempt
        }) {
            existing.cursor = existing.cursor.max(receipt.cursor);
        } else {
            if receipts.len() >= MAX_CHILD_REPORT_RECEIPTS {
                return Err("child report receipt limit reached".to_string());
            }
            receipts.push(receipt);
        }
        let value = json!({
            "version": 1,
            "session_id": session_id,
            "receipts": receipts,
        });
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| format!("cannot encode child report receipts: {error}"))?;
        if encoded.len() as u64 > MAX_CHILD_REPORT_RECEIPTS_BYTES {
            return Err("child report receipts exceed their storage limit".to_string());
        }
        write_json_atomic(&session_dir.join(CHILD_REPORT_RECEIPTS_FILE_NAME), &value)
    }

    /// Reads the latest committed checkpoint without acquiring the Session's
    /// live writer lock.
    ///
    /// An active runtime may still be appending a Turn, so the file can end in
    /// an incomplete record. `load_records` deliberately ignores that trailing
    /// record and returns the last complete checkpoint. This read-only seam is
    /// what lets an App Server create an exact child Session while the parent
    /// Turn is still running; it never reads mutable Core state.
    pub fn read_settled_checkpoint(
        workspace: &Path,
        session_id: &str,
    ) -> Result<(u64, Vec<Message>), String> {
        validate_session_id(session_id)?;
        let (_, path) = resolve_session_file(workspace, session_id)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot open Session {session_id}: {error}"))?;
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(format!(
                "Session {session_id} exceeds {MAX_SESSION_BYTES} byte limit"
            ));
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read Session {session_id}: {error}"))?;
        let loaded = load_records(session_id, &bytes)?;
        Ok((loaded.checkpoint_seq, loaded.messages))
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
        self.continuation_mode = Some(mode.to_string());
        self.persist_thread_settings()?;
        Ok(())
    }

    pub fn model_selection(&self) -> Option<&ModelSelection> {
        self.model_selection.as_ref()
    }

    pub fn reasoning_selection(&self) -> Option<&ReasoningSelection> {
        self.reasoning_selection.as_ref()
    }

    pub fn set_model_settings(
        &mut self,
        selection: Option<ModelSelection>,
        reasoning_selection: Option<ReasoningSelection>,
    ) -> Result<(), String> {
        self.model_selection = selection;
        self.reasoning_selection = reasoning_selection;
        self.persist_thread_settings()
    }

    fn persist_thread_settings(&self) -> Result<(), String> {
        let settings = json!({
            "version": 1,
            "thread_id": self.thread_id.as_str(),
            "continuation_mode": self.continuation_mode,
            "model_selection": self.model_selection,
            "reasoning_selection": self.reasoning_selection,
        });
        write_json_atomic(&self.session_dir.join(THREAD_SETTINGS_FILE_NAME), &settings)
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
        self.model_selection = None;
        self.reasoning_selection = None;
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

    /// Appends one bounded lifecycle snapshot for a Host-owned operation.
    ///
    /// Operation records are append-only so the latest valid record is the
    /// recoverable state after a restart. They are ignored by Core's message
    /// reconstruction and therefore cannot change the model conversation.
    pub fn record_operation(&mut self, operation: SessionOperation) -> Result<(), String> {
        let mut operation = operation;
        let previous = if operation.kind == "child_task" {
            latest_operation(&self.path, &operation.operation_id)?
        } else {
            None
        };
        if let Some(previous) = previous.as_ref() {
            operation.parent_thread_id = operation
                .parent_thread_id
                .or_else(|| previous.parent_thread_id.clone());
            operation.group_id = operation.group_id.or_else(|| previous.group_id.clone());
            operation.execution_mode = operation
                .execution_mode
                .or_else(|| previous.execution_mode.clone());
            operation.sequence = operation.sequence.or(previous.sequence);
            operation.prompt = operation.prompt.or_else(|| previous.prompt.clone());
            operation.attempt_kind = operation.attempt_kind.or(previous.attempt_kind);
            operation.control_request_id = operation
                .control_request_id
                .or_else(|| previous.control_request_id.clone());
            operation.control_action = operation
                .control_action
                .or_else(|| previous.control_action.clone());
            operation.control_source = operation.control_source.or(previous.control_source);
            if operation.status == "cancelled"
                && previous.status == "pausing"
                && previous.attempt == operation.attempt
                && previous.turn_id == operation.turn_id
            {
                operation.status = "paused".to_string();
                operation.turn_id = None;
            }
        }
        if operation.status == "running" {
            operation.error = None;
        }

        operation.validate()?;
        let mut records = vec![operation_record_value(&operation)];

        if operation.kind == "child_task"
            && matches!(
                operation.status.as_str(),
                "completed" | "failed" | "cancelled"
            )
            && let Some(request) = latest_pending_follow_up(
                &self.path,
                &operation.operation_id,
                operation.parent_thread_id.as_deref(),
            )?
        {
            let request_status = if operation.status == "completed" {
                "started"
            } else if operation.status == "failed" {
                "blocked"
            } else {
                "cancelled"
            };
            records.push(control_request_transition(
                &request,
                request_status,
                request_status,
                timestamp_ms(),
            ));
            if operation.status == "completed" {
                let prompt = request
                    .get("prompt")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "queued follow-up prompt was lost".to_string())?;
                let request_id = request
                    .get("request_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "queued follow-up request id was lost".to_string())?;
                let mut follow_up = operation.clone();
                follow_up.attempt = follow_up
                    .attempt
                    .checked_add(1)
                    .ok_or_else(|| "child task attempt limit reached".to_string())?;
                follow_up.status = "queued".to_string();
                follow_up.turn_id = None;
                follow_up.prompt = Some(prompt.to_string());
                follow_up.result = None;
                follow_up.error = None;
                follow_up.attempt_kind = Some(ChildTaskAttemptKind::FollowUp);
                follow_up.control_request_id = Some(request_id.to_string());
                follow_up.control_action = Some("queue_follow_up".to_string());
                follow_up.timestamp_ms = timestamp_ms();
                follow_up.validate()?;
                records.push(operation_record_value(&follow_up));
            }
        }
        self.append_records(records)
    }

    pub fn operation(&self, operation_id: &str) -> Result<Option<SessionOperation>, String> {
        latest_operation(&self.path, operation_id)
    }

    pub fn operation_for_turn(&self, turn_id: &str) -> Result<Option<SessionOperation>, String> {
        Ok(session_values(&self.path)?
            .into_iter()
            .rev()
            .find_map(|record| {
                (record.get("kind").and_then(Value::as_str) == Some("operation")
                    && record.get("turn_id").and_then(Value::as_str) == Some(turn_id))
                .then(|| serde_json::from_value(record).ok())
                .flatten()
            }))
    }

    pub fn child_task_context(&self) -> Result<Option<ChildTaskContext>, String> {
        let records = session_values(&self.path)?;
        if !records.iter().any(|record| {
            record.get("kind").and_then(Value::as_str) == Some("session_created")
                && record.get("forked_from").is_some()
        }) {
            return Ok(None);
        }
        let operation = records.iter().rev().find(|record| {
            record.get("kind").and_then(Value::as_str) == Some("operation")
                && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
        });
        Ok(operation.and_then(|record| {
            Some(ChildTaskContext {
                parent_thread_id: record.get("parent_thread_id")?.as_str()?.to_string(),
                operation_id: record.get("operation_id")?.as_str()?.to_string(),
                attempt: record.get("attempt")?.as_u64()?.try_into().ok()?,
            })
        }))
    }

    pub fn record_child_report(
        &mut self,
        context: &ChildTaskContext,
        report_id: &str,
        report: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        if report_id.is_empty() || report_id.len() > MAX_OPERATION_ID_BYTES {
            return Err("report_id must be bounded and non-empty".to_string());
        }
        if report.trim().is_empty()
            || report.len() > MAX_CHILD_REPORT_BYTES
            || report
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err("report must be non-empty and at most 4 KiB".to_string());
        }
        let operation = validate_child_operation(self, context, None)?;
        for record in session_values(&self.path)? {
            if record.get("kind").and_then(Value::as_str) == Some("child_report")
                && record.get("operation_id").and_then(Value::as_str)
                    == Some(context.operation_id.as_str())
                && record.get("attempt").and_then(Value::as_u64)
                    == Some(u64::from(operation.attempt))
                && record.get("report_id").and_then(Value::as_str) == Some(report_id)
            {
                return Ok(ChildTaskMutationResult {
                    status: "reported".to_string(),
                    cursor: record
                        .get("seq")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    attempt: operation.attempt,
                    attempt_kind: operation.attempt_kind,
                    turn_id: operation.turn_id.clone(),
                    duplicate: true,
                    request_action: None,
                    timestamp_ms: record
                        .get("timestamp_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                });
            }
        }
        let timestamp_ms = timestamp_ms();
        self.append_records(vec![json!({
            "kind": "child_report",
            "operation_id": context.operation_id,
            "parent_thread_id": context.parent_thread_id,
            "report_id": report_id,
            "attempt": operation.attempt,
            "timestamp_ms": timestamp_ms,
            "report": report,
        })])?;
        Ok(ChildTaskMutationResult {
            status: "reported".to_string(),
            cursor: self.next_seq.saturating_sub(1),
            attempt: operation.attempt,
            attempt_kind: operation.attempt_kind,
            turn_id: operation.turn_id.clone(),
            duplicate: false,
            request_action: None,
            timestamp_ms,
        })
    }

    pub fn queue_child_follow_up(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        prompt: String,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        if prompt.trim().is_empty() {
            return Err("prompt must be non-empty and bounded".to_string());
        }
        validate_operation_prompt(&prompt)?;
        self.validate_child_control_owner(context)?;
        if let Some(existing) = child_control_request_by_id(
            &self.path,
            &context.operation_id,
            &context.parent_thread_id,
            "queue_follow_up",
            request_id,
        )? {
            return Ok(child_mutation_from_request(existing, context.attempt));
        }
        if latest_pending_follow_up(
            &self.path,
            &context.operation_id,
            Some(&context.parent_thread_id),
        )?
        .is_some()
        {
            return Err("a child task can have only one pending follow-up".to_string());
        }
        let mut operation = validate_child_operation(self, context, None)?;
        if !matches!(
            operation.status.as_str(),
            "running" | "in_progress" | "awaiting_approval" | "paused" | "completed" | "failed"
        ) {
            return Err(format!(
                "child task status is {}, cannot queue a follow-up",
                operation.status
            ));
        }
        let active = matches!(
            operation.status.as_str(),
            "running" | "in_progress" | "awaiting_approval" | "paused"
        );
        let request_status = if active {
            "accepted"
        } else if operation.status == "failed" {
            "blocked"
        } else {
            "started"
        };
        let timestamp_ms = timestamp_ms();
        let request = json!({
            "kind": "child_control_request",
            "action": "queue_follow_up",
            "request_id": request_id,
            "request_status": request_status,
            "status": if request_status == "blocked" { "blocked" } else { "queued" },
            "parent_thread_id": context.parent_thread_id,
            "operation_id": context.operation_id,
            "attempt": operation.attempt,
            "attempt_kind": operation.attempt_kind,
            "prompt": prompt,
            "timestamp_ms": timestamp_ms,
        });
        let mut records = vec![request];
        if operation.status == "completed" {
            operation.attempt = operation
                .attempt
                .checked_add(1)
                .ok_or_else(|| "child task attempt limit reached".to_string())?;
            operation.status = "queued".to_string();
            operation.turn_id = None;
            operation.prompt = Some(prompt);
            operation.result = None;
            operation.error = None;
            operation.attempt_kind = Some(ChildTaskAttemptKind::FollowUp);
            operation.control_request_id = Some(request_id.to_string());
            operation.control_action = Some("queue_follow_up".to_string());
            operation.timestamp_ms = timestamp_ms;
            operation.validate()?;
            records.push(operation_record_value(&operation));
        }
        self.append_records(records)?;
        Ok(ChildTaskMutationResult {
            status: if active {
                "queued".to_string()
            } else if request_status == "blocked" {
                "blocked".to_string()
            } else {
                operation.status.clone()
            },
            cursor: self.next_seq.saturating_sub(1),
            attempt: if request_status == "started" {
                operation.attempt
            } else {
                context.attempt
            },
            attempt_kind: operation.attempt_kind,
            turn_id: if request_status == "started" {
                None
            } else {
                operation.turn_id
            },
            duplicate: false,
            request_action: Some(ChildControlRequestAction::QueueFollowUp),
            timestamp_ms,
        })
    }

    pub fn pause_child_task(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        self.pause_child_task_from(
            context,
            request_id,
            turn_id,
            ChildTaskControlSource::MainAgent,
        )
    }

    pub fn pause_child_task_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        self.record_active_child_control(context, request_id, turn_id, "pause", false, source)
    }

    pub fn cancel_active_child_task(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        self.cancel_active_child_task_from(
            context,
            request_id,
            turn_id,
            ChildTaskControlSource::MainAgent,
        )
    }

    pub fn cancel_active_child_task_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        self.record_active_child_control(
            context,
            request_id,
            turn_id,
            "cancel_active",
            true,
            source,
        )
    }

    pub fn resume_child_task(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        self.resume_child_task_from(context, request_id, ChildTaskControlSource::MainAgent)
    }

    pub fn resume_child_task_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        if let Some(existing) = self.child_control_replay(context, "resume", request_id)? {
            return Ok(existing);
        }
        let mut operation = validate_child_operation(self, context, Some(&["paused"]))?;
        operation.status = "queued".to_string();
        operation.turn_id = None;
        operation.control_request_id = Some(request_id.to_string());
        operation.control_action = Some("resume".to_string());
        operation.control_source = Some(source);
        self.record_control_operation(operation, false, None)
    }

    /// Reattaches a child operation to its original Turn before resuming a
    /// durable execution checkpoint. This is deliberately distinct from
    /// resuming a paused operation into the ordinary child queue.
    pub fn resume_child_execution_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        validate_operation_text(turn_id, MAX_OPERATION_ID_BYTES, "turn id")?;
        if let Some(existing) = self.child_control_replay(context, "resume", request_id)? {
            return Ok(existing);
        }
        let execution = self
            .execution_state()
            .ok_or_else(|| "no execution checkpoint is available".to_string())?;
        if execution.checkpoint.turn_id.as_str() != turn_id
            || execution.status != SessionExecutionStatus::WaitingForContinue
        {
            return Err("execution checkpoint changed; refresh before continuing".to_string());
        }
        let mut operation = validate_child_operation(
            self,
            context,
            Some(&[
                "paused",
                "queued",
                "running",
                "in_progress",
                "awaiting_approval",
                "failed",
            ]),
        )?;
        if operation
            .turn_id
            .as_deref()
            .is_some_and(|current| current != turn_id)
        {
            return Err("child task Turn identity mismatch".to_string());
        }
        operation.status = "running".to_string();
        operation.turn_id = Some(turn_id.to_string());
        operation.result = None;
        operation.error = None;
        operation.control_request_id = Some(request_id.to_string());
        operation.control_action = Some("resume".to_string());
        operation.control_source = Some(source);
        self.record_control_operation(operation, false, None)
    }

    fn record_active_child_control(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
        action: &str,
        cancel_follow_up: bool,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        validate_operation_text(turn_id, MAX_OPERATION_ID_BYTES, "turn id")?;
        if let Some(existing) = self.child_control_replay(context, action, request_id)? {
            return Ok(existing);
        }
        let mut operation = validate_child_operation(
            self,
            context,
            Some(&["running", "in_progress", "awaiting_approval", "pausing"]),
        )?;
        if operation.turn_id.as_deref() != Some(turn_id) {
            return Err("child task turn identity mismatch".to_string());
        }
        let status = if action == "pause" {
            "pausing"
        } else {
            "cancelling"
        };
        operation.status = status.to_string();
        operation.control_request_id = Some(request_id.to_string());
        operation.control_action = Some(action.to_string());
        operation.control_source = Some(source);
        operation.timestamp_ms = timestamp_ms();
        let mut records = vec![operation_record_value(&operation)];
        if cancel_follow_up
            && let Some(follow_up) = latest_pending_follow_up(
                &self.path,
                &context.operation_id,
                Some(&context.parent_thread_id),
            )?
        {
            records.push(control_request_transition(
                &follow_up,
                "cancelled",
                "cancelled",
                operation.timestamp_ms,
            ));
        }
        self.append_records(records)?;
        Ok(child_mutation_from_operation(
            &operation,
            self.next_seq.saturating_sub(1),
            false,
            None,
        ))
    }

    pub fn child_steer_request(
        &self,
        context: &ChildTaskContext,
        request_id: &str,
    ) -> Result<Option<ChildTaskMutationResult>, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        self.validate_child_control_owner(context)?;
        let records = session_values(&self.path)?;
        let record = records.iter().rev().find(|record| {
            record.get("operation_id").and_then(Value::as_str)
                == Some(context.operation_id.as_str())
                && record.get("parent_thread_id").and_then(Value::as_str)
                    == Some(context.parent_thread_id.as_str())
                && ((record.get("kind").and_then(Value::as_str) == Some("child_control_request")
                    && record.get("action").and_then(Value::as_str) == Some("steer")
                    && record.get("request_id").and_then(Value::as_str) == Some(request_id))
                    || (record.get("kind").and_then(Value::as_str) == Some("operation")
                        && record.get("operation_kind").and_then(Value::as_str)
                            == Some("child_task")
                        && record.get("control_request_id").and_then(Value::as_str)
                            == Some(request_id)
                        && record.get("control_action").and_then(Value::as_str)
                            == Some("queue_follow_up")))
        });
        let Some(record) = record else {
            return Ok(None);
        };
        if record.get("kind").and_then(Value::as_str) == Some("child_control_request") {
            let request_status = record
                .get("request_status")
                .and_then(Value::as_str)
                .unwrap_or("accepted");
            if request_status == "not_accepted" {
                return Ok(None);
            }
            return Ok(Some(ChildTaskMutationResult {
                status: record
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("steered")
                    .to_string(),
                cursor: record
                    .get("seq")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                attempt: record
                    .get("attempt")
                    .and_then(Value::as_u64)
                    .and_then(|value| value.try_into().ok())
                    .unwrap_or(context.attempt),
                attempt_kind: record
                    .get("attempt_kind")
                    .and_then(Value::as_str)
                    .and_then(|value| serde_json::from_value(json!(value)).ok()),
                turn_id: record
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                duplicate: true,
                request_action: Some(ChildControlRequestAction::Steer),
                timestamp_ms: record
                    .get("timestamp_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            }));
        }
        let operation: SessionOperation = serde_json::from_value(record.clone())
            .map_err(|error| format!("cannot decode child operation: {error}"))?;
        Ok(Some(ChildTaskMutationResult {
            status: operation.status,
            cursor: record
                .get("seq")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            attempt: operation.attempt,
            attempt_kind: operation.attempt_kind,
            turn_id: operation.turn_id,
            duplicate: true,
            request_action: Some(ChildControlRequestAction::QueueFollowUp),
            timestamp_ms: operation.timestamp_ms,
        }))
    }

    pub fn transition_child_steer_request(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        turn_id: &str,
        step: ChildSteerRequestStep,
        accepted_status: Option<&str>,
    ) -> Result<Option<ChildTaskMutationResult>, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        validate_operation_text(turn_id, MAX_OPERATION_ID_BYTES, "turn id")?;
        self.validate_child_control_owner(context)?;
        if step == ChildSteerRequestStep::Reserve {
            if let Some(existing) = self.child_steer_request(context, request_id)? {
                return Ok(Some(existing));
            }
            let operation = latest_operation(&self.path, &context.operation_id)?
                .ok_or_else(|| "child operation not found".to_string())?;
            if !matches!(
                operation.status.as_str(),
                "running" | "in_progress" | "awaiting_approval"
            ) {
                return Ok(Some(ChildTaskMutationResult {
                    status: "not_submitted".to_string(),
                    cursor: self.next_seq.saturating_sub(1),
                    attempt: operation.attempt,
                    attempt_kind: operation.attempt_kind,
                    turn_id: operation.turn_id,
                    duplicate: false,
                    request_action: Some(ChildControlRequestAction::Steer),
                    timestamp_ms: timestamp_ms(),
                }));
            }
            if operation.attempt != context.attempt {
                return Err("child task attempt identity mismatch".to_string());
            }
            if operation.turn_id.as_deref() != Some(turn_id) {
                return Err("child task turn identity mismatch".to_string());
            }
            return self
                .append_child_steer_request(
                    context,
                    request_id,
                    operation.attempt,
                    operation.attempt_kind,
                    turn_id,
                    "pending",
                    "pending",
                )
                .map(Some);
        }

        let current = session_values(&self.path)?
            .into_iter()
            .rev()
            .find(|record| {
                record.get("kind").and_then(Value::as_str) == Some("child_control_request")
                    && record.get("action").and_then(Value::as_str) == Some("steer")
                    && record.get("request_id").and_then(Value::as_str) == Some(request_id)
                    && record.get("operation_id").and_then(Value::as_str)
                        == Some(context.operation_id.as_str())
                    && record.get("parent_thread_id").and_then(Value::as_str)
                        == Some(context.parent_thread_id.as_str())
                    && record.get("request_status").and_then(Value::as_str) == Some("pending")
            })
            .ok_or_else(|| "child steer request reservation is not pending".to_string())?;
        if current.get("turn_id").and_then(Value::as_str) != Some(turn_id) {
            return Err("child task turn identity mismatch".to_string());
        }
        let attempt = current
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|value| value.try_into().ok())
            .unwrap_or(context.attempt);
        let attempt_kind = current
            .get("attempt_kind")
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_value(json!(value)).ok());
        let (request_status, status) = match step {
            ChildSteerRequestStep::Accept => ("accepted", accepted_status.unwrap_or("steered")),
            ChildSteerRequestStep::NotAccepted => ("not_accepted", "not_submitted"),
            ChildSteerRequestStep::Reserve => unreachable!(),
        };
        self.append_child_steer_request(
            context,
            request_id,
            attempt,
            attempt_kind,
            turn_id,
            request_status,
            status,
        )
        .map(Some)
    }

    fn validate_child_control_owner(&self, context: &ChildTaskContext) -> Result<(), String> {
        let owner = self
            .child_task_context()?
            .ok_or_else(|| "session is not owned by a child task".to_string())?;
        if owner.operation_id != context.operation_id
            || owner.parent_thread_id != context.parent_thread_id
        {
            return Err("child task ownership mismatch".to_string());
        }
        let operation = latest_operation(&self.path, &context.operation_id)?
            .ok_or_else(|| "child operation not found".to_string())?;
        if operation.kind != "child_task"
            || operation.parent_thread_id.as_deref() != Some(context.parent_thread_id.as_str())
        {
            return Err("child task ownership mismatch".to_string());
        }
        Ok(())
    }

    fn child_control_replay(
        &self,
        context: &ChildTaskContext,
        action: &str,
        request_id: &str,
    ) -> Result<Option<ChildTaskMutationResult>, String> {
        self.validate_child_control_owner(context)?;
        let record = session_values(&self.path)?
            .into_iter()
            .rev()
            .find(|record| {
                record.get("kind").and_then(Value::as_str) == Some("operation")
                    && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
                    && record.get("operation_id").and_then(Value::as_str)
                        == Some(context.operation_id.as_str())
                    && record.get("parent_thread_id").and_then(Value::as_str)
                        == Some(context.parent_thread_id.as_str())
                    && record.get("control_request_id").and_then(Value::as_str) == Some(request_id)
                    && record.get("control_action").and_then(Value::as_str) == Some(action)
            });
        let Some(record) = record else {
            return Ok(None);
        };
        let operation: SessionOperation = serde_json::from_value(record.clone())
            .map_err(|error| format!("cannot decode child operation: {error}"))?;
        Ok(Some(child_mutation_from_operation(
            &operation,
            record
                .get("seq")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            true,
            None,
        )))
    }

    fn record_control_operation(
        &mut self,
        mut operation: SessionOperation,
        duplicate: bool,
        request_action: Option<ChildControlRequestAction>,
    ) -> Result<ChildTaskMutationResult, String> {
        operation.timestamp_ms = timestamp_ms();
        operation.validate()?;
        self.append_records(vec![operation_record_value(&operation)])?;
        Ok(child_mutation_from_operation(
            &operation,
            self.next_seq.saturating_sub(1),
            duplicate,
            request_action,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn append_child_steer_request(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        attempt: u32,
        attempt_kind: Option<ChildTaskAttemptKind>,
        turn_id: &str,
        request_status: &str,
        status: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        let timestamp_ms = timestamp_ms();
        self.append_records(vec![json!({
            "kind": "child_control_request",
            "action": "steer",
            "request_id": request_id,
            "request_status": request_status,
            "status": status,
            "parent_thread_id": context.parent_thread_id,
            "operation_id": context.operation_id,
            "attempt": attempt,
            "attempt_kind": attempt_kind,
            "turn_id": turn_id,
            "timestamp_ms": timestamp_ms,
        })])?;
        Ok(ChildTaskMutationResult {
            status: status.to_string(),
            cursor: self.next_seq.saturating_sub(1),
            attempt,
            attempt_kind,
            turn_id: Some(turn_id.to_string()),
            duplicate: false,
            request_action: Some(ChildControlRequestAction::Steer),
            timestamp_ms,
        })
    }

    pub fn update_queued_child_task(
        &mut self,
        context: &ChildTaskContext,
        prompt: String,
        request_id: Option<&str>,
    ) -> Result<ChildTaskMutationResult, String> {
        if prompt.trim().is_empty() || prompt.len() > MAX_OPERATION_PROMPT_BYTES {
            return Err("prompt must be non-empty and bounded".to_string());
        }
        self.mutate_queued_child_task(context, Some(prompt), request_id, None)
    }

    pub fn cancel_queued_child_task(
        &mut self,
        context: &ChildTaskContext,
        request_id: Option<&str>,
    ) -> Result<ChildTaskMutationResult, String> {
        self.cancel_queued_child_task_from(context, request_id, ChildTaskControlSource::MainAgent)
    }

    pub fn cancel_queued_child_task_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: Option<&str>,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        self.mutate_queued_child_task(context, None, request_id, Some(source))
    }

    pub fn retry_child_task(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        self.retry_child_task_from(context, request_id, ChildTaskControlSource::MainAgent)
    }

    pub fn retry_child_task_from(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        source: ChildTaskControlSource,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        if let Some(existing) = self.child_control_replay(context, "retry", request_id)? {
            return Ok(existing);
        }
        let mut operation =
            validate_child_operation(self, context, Some(&["failed", "cancelled"]))?;
        operation.attempt = operation
            .attempt
            .checked_add(1)
            .ok_or_else(|| "child task attempt limit reached".to_string())?;
        operation.status = "queued".to_string();
        operation.turn_id = None;
        operation.result = None;
        operation.error = None;
        operation.attempt_kind = Some(ChildTaskAttemptKind::Retry);
        operation.control_request_id = Some(request_id.to_string());
        operation.control_action = Some("retry".to_string());
        operation.control_source = Some(source);
        self.record_control_operation(operation, false, None)
    }

    pub fn record_child_start_failure(
        &mut self,
        context: &ChildTaskContext,
        request_id: &str,
        error: &str,
    ) -> Result<ChildTaskMutationResult, String> {
        validate_operation_text(request_id, 192, "control request id")?;
        validate_operation_text(error, MAX_OPERATION_ERROR_BYTES, "start error")?;
        if let Some(existing) = self.child_control_replay(context, "start_failure", request_id)? {
            return Ok(existing);
        }
        let mut operation = validate_child_operation(self, context, Some(&["queued"]))?;
        operation.error = Some(error.to_string());
        operation.control_request_id = Some(request_id.to_string());
        operation.control_action = Some("start_failure".to_string());
        self.record_control_operation(operation, false, None)
    }

    fn mutate_queued_child_task(
        &mut self,
        context: &ChildTaskContext,
        prompt: Option<String>,
        request_id: Option<&str>,
        source: Option<ChildTaskControlSource>,
    ) -> Result<ChildTaskMutationResult, String> {
        let action = if prompt.is_some() {
            "update_queued"
        } else {
            "cancel_queued"
        };
        if let Some(request_id) = request_id {
            validate_operation_text(request_id, 192, "control request id")?;
            if let Some(existing) = self.child_control_replay(context, action, request_id)? {
                return Ok(existing);
            }
        }
        let statuses: &[&str] = if prompt.is_some() {
            &["queued"]
        } else {
            &["queued", "paused"]
        };
        let mut operation = validate_child_operation(self, context, Some(statuses))?;
        if let Some(prompt) = prompt {
            operation.prompt = Some(prompt);
        } else {
            operation.status = "cancelled".to_string();
        }
        if let Some(request_id) = request_id {
            operation.control_request_id = Some(request_id.to_string());
            operation.control_action = Some(action.to_string());
            operation.control_source = source;
        }
        operation.timestamp_ms = timestamp_ms();
        let mut records = Vec::new();
        if operation.status == "cancelled"
            && let Some(follow_up) = latest_pending_follow_up(
                &self.path,
                &context.operation_id,
                Some(&context.parent_thread_id),
            )?
        {
            records.push(control_request_transition(
                &follow_up,
                "cancelled",
                "cancelled",
                operation.timestamp_ms,
            ));
        }
        records.push(operation_record_value(&operation));
        self.append_records(records)?;
        Ok(child_mutation_from_operation(
            &operation,
            self.next_seq.saturating_sub(1),
            false,
            None,
        ))
    }

    pub fn record_turn_with_id(
        &mut self,
        turn_id: &str,
        turn: TurnCommit<'_>,
    ) -> Result<(), String> {
        self.record_turn_with_id_and_resume(turn_id, turn, false)
    }

    pub fn record_resumed_turn_with_id(
        &mut self,
        turn_id: &str,
        turn: TurnCommit<'_>,
    ) -> Result<(), String> {
        self.record_turn_with_id_and_resume(turn_id, turn, true)
    }

    fn record_turn_with_id_and_resume(
        &mut self,
        turn_id: &str,
        turn: TurnCommit<'_>,
        execution_resume: bool,
    ) -> Result<(), String> {
        let mut turn_started = json!({
            "kind": "turn_started",
            "thread_id": self.thread_id,
            "turn_id": turn_id,
            "timestamp_ms": turn.started_at_ms,
            "prompt": turn.prompt,
            "execution_resume": execution_resume,
        });
        if let Some(presentation) = turn.presentation
            && let Some(record) = turn_started.as_object_mut()
        {
            record.insert("presentation".to_string(), json!(presentation));
        }
        let mut records = vec![turn_started];
        let items = turn
            .messages
            .iter()
            .filter_map(|message| {
                let item_id = item_id_for_message(message);
                let arguments = match message {
                    Message::Tool { call_id, .. } => turn
                        .tool_arguments
                        .iter()
                        .find(|(id, _)| id == call_id)
                        .map(|(_, arguments)| arguments.clone()),
                    _ => None,
                };
                let already_persisted = execution_resume
                    && self.items.iter().any(|item| {
                        item.turn_id.as_deref() == Some(turn_id) && item.message == *message
                    });
                if !already_persisted {
                    records.push(self.item_record_with_id(
                        Some(turn_id),
                        message,
                        &item_id,
                        arguments.as_ref(),
                    ));
                    Some(SessionItem {
                        item_id,
                        thread_id: self.thread_id.clone(),
                        turn_id: Some(turn_id.to_string()),
                        message: message.clone(),
                        arguments,
                    })
                } else {
                    None
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
        if let Some(source) = turn
            .presentation
            .and_then(|presentation| presentation.turn_source)
        {
            self.turn_sources.insert(turn_id.to_string(), source);
        }
        self.items.extend(items);
        self.checkpoint_seq = self.next_seq.saturating_sub(1);
        if !execution_resume {
            self.turn_count = self.turn_count.saturating_add(1);
            self.thread_turn_count = self.thread_turn_count.saturating_add(1);
        }
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
        let (continuation_mode, model_selection, reasoning_selection) =
            load_thread_settings(&session_dir, &loaded.thread_id);
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
            turn_sources: loaded.turn_sources,
            is_forked: loaded.is_forked,
            created_at_ms: loaded.created_at_ms,
            continuation_mode,
            model_selection,
            reasoning_selection,
            append_lock: Arc::new(Mutex::new(())),
            execution_state: Arc::new(Mutex::new(loaded.execution_state)),
            _lock: lock,
        };
        if let Some(state) = store.execution_state()
            && state.status == SessionExecutionStatus::Running
        {
            let unresolved_started_call = state.pending_batch.as_ref().is_some_and(|batch| {
                batch
                    .calls
                    .iter()
                    .any(|call| call.started && call.outcome.is_none())
            });
            let mut journal = store.execution_journal(&loaded.messages);
            let entry = if unresolved_started_call {
                ExecutionJournalEntry::NeedsReconciliation {
                    turn_id: state.checkpoint.turn_id,
                    reason: "process_restart_during_tool_call".to_string(),
                }
            } else {
                ExecutionJournalEntry::WaitingForContinue {
                    turn_id: state.checkpoint.turn_id,
                    reason: "process_restart".to_string(),
                }
            };
            journal.append(entry)?;
        }
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
        Self::fork_from_checkpoint_with_operation(
            workspace,
            parent_session_id,
            parent_checkpoint_seq,
            child_thread_id,
            checkpoint,
            fork_metadata,
            None,
        )
    }

    /// Persists a fork and, when requested, its initial queued operation in
    /// the same child Session before returning to the caller.
    pub fn fork_from_checkpoint_with_operation(
        workspace: &Path,
        parent_session_id: &str,
        parent_checkpoint_seq: u64,
        child_thread_id: &str,
        checkpoint: &[Message],
        fork_metadata: SessionForkMetadata,
        operation: Option<SessionOperation>,
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
        let parent_bytes = fs::read(&parent_path).map_err(|error| {
            SessionForkError::Storage(format!("cannot read parent session: {error}"))
        })?;
        let parent_loaded = load_records(parent_session_id, &parent_bytes)
            .map_err(|error| SessionForkError::Storage(error.to_string()))?;
        let (_, parent_model_selection, parent_reasoning_selection) =
            load_thread_settings(&parent_dir, &parent_loaded.thread_id);

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
                Ok(mut store) => {
                    if (parent_model_selection.is_some() || parent_reasoning_selection.is_some())
                        && let Err(error) = store.set_model_settings(
                            parent_model_selection.clone(),
                            parent_reasoning_selection.clone(),
                        )
                    {
                        drop(store);
                        let _ = fs::remove_dir_all(&session_dir);
                        return Err(SessionForkError::Storage(error));
                    }
                    if let Some(operation) = operation.clone()
                        && let Err(error) = store.record_operation(operation)
                    {
                        drop(store);
                        let _ = fs::remove_dir_all(&session_dir);
                        return Err(SessionForkError::Storage(error));
                    }
                    store
                }
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
        let is_forked = forked_from.is_some();
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
            turn_sources: HashMap::new(),
            is_forked,
            created_at_ms: now,
            continuation_mode: None,
            model_selection: None,
            reasoning_selection: None,
            append_lock: Arc::new(Mutex::new(())),
            execution_state: Arc::new(Mutex::new(None)),
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
        self.bytes = self
            .file
            .seek(SeekFrom::End(0))
            .map_err(|error| format!("cannot seek session append position: {error}"))?;
        Ok(())
    }
}

fn load_thread_settings(
    session_dir: &Path,
    thread_id: &str,
) -> (
    Option<String>,
    Option<ModelSelection>,
    Option<ReasoningSelection>,
) {
    let Some(value) = fs::read_to_string(session_dir.join(THREAD_SETTINGS_FILE_NAME))
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
    else {
        return (None, None, None);
    };
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("thread_id").and_then(Value::as_str) != Some(thread_id)
    {
        return (None, None, None);
    }
    let continuation_mode = match value.get("continuation_mode").and_then(Value::as_str) {
        Some("manual") | Some("continuous") => value
            .get("continuation_mode")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    let model_selection = value
        .get("model_selection")
        .cloned()
        .and_then(|selection| serde_json::from_value::<ModelSelection>(selection).ok())
        .filter(|selection| {
            valid_model_identifier(&selection.provider_id)
                && valid_model_identifier(&selection.model_id)
        });
    let reasoning_selection = value
        .get("reasoning_selection")
        .cloned()
        .and_then(|value| serde_json::from_value::<ReasoningSelection>(value).ok())
        .filter(valid_reasoning_selection)
        .or_else(|| {
            value
                .get("reasoning_effort")
                .and_then(Value::as_str)
                .filter(|effort| matches!(*effort, "low" | "medium" | "high" | "xhigh" | "max"))
                .map(|effort| ReasoningSelection::Level(effort.to_string()))
        });
    (continuation_mode, model_selection, reasoning_selection)
}

fn valid_reasoning_selection(selection: &ReasoningSelection) -> bool {
    match selection {
        ReasoningSelection::ApiDefault => true,
        ReasoningSelection::Level(value) => {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        }
    }
}

fn valid_model_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
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

fn read_session_control(
    session_dir: &Path,
    session_id: &str,
) -> Result<SessionControlState, String> {
    let path = session_dir.join(SESSION_CONTROL_FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionControlState {
                session_id: session_id.to_string(),
                status: SessionControlStatus::Running,
                request_id: None,
                updated_at_ms: 0,
            });
        }
        Err(error) => return Err(format!("cannot read Session control state: {error}")),
    };
    if bytes.len() as u64 > MAX_SESSION_CONTROL_BYTES {
        return Err("Session control state exceeds its storage limit".to_string());
    }
    let state: SessionControlState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Session control state: {error}"))?;
    if state.session_id != session_id {
        return Err("Session control identity does not match its directory".to_string());
    }
    if let Some(request_id) = state.request_id.as_deref() {
        validate_operation_text(request_id, 192, "session control request id")?;
    }
    Ok(state)
}

fn persist_session_control(session_dir: &Path, state: &SessionControlState) -> Result<(), String> {
    let value = json!({
        "version": 1,
        "sessionId": state.session_id,
        "status": state.status,
        "requestId": state.request_id,
        "updatedAtMs": state.updated_at_ms,
    });
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| format!("cannot encode Session control state: {error}"))?;
    if encoded.len() as u64 > MAX_SESSION_CONTROL_BYTES {
        return Err("Session control state exceeds its storage limit".to_string());
    }
    write_json_atomic(&session_dir.join(SESSION_CONTROL_FILE_NAME), &value)
}

fn read_child_report_receipts(session_dir: &Path) -> Result<Vec<ChildReportReceipt>, String> {
    let path = session_dir.join(CHILD_REPORT_RECEIPTS_FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read child report receipts: {error}")),
    };
    if bytes.len() as u64 > MAX_CHILD_REPORT_RECEIPTS_BYTES {
        return Err("child report receipts exceed their storage limit".to_string());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid child report receipts: {error}"))?;
    let session_id = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "parent Session identity is unavailable".to_string())?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("session_id").and_then(Value::as_str) != Some(session_id)
    {
        return Err("child report receipt identity does not match its Session".to_string());
    }
    let receipts = serde_json::from_value::<Vec<ChildReportReceipt>>(
        value
            .get("receipts")
            .cloned()
            .ok_or_else(|| "child report receipts are missing".to_string())?,
    )
    .map_err(|error| format!("invalid child report receipts: {error}"))?;
    if receipts.len() > MAX_CHILD_REPORT_RECEIPTS {
        return Err("child report receipt count exceeds its limit".to_string());
    }
    for receipt in &receipts {
        validate_child_report_receipt(receipt)?;
    }
    Ok(receipts)
}

fn validate_child_report_receipt(receipt: &ChildReportReceipt) -> Result<(), String> {
    if receipt.child_thread_id.is_empty()
        || receipt.child_thread_id.len() > 64
        || !receipt
            .child_thread_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("child report receipt has an invalid Thread id".to_string());
    }
    validate_operation_text(
        &receipt.operation_id,
        MAX_OPERATION_ID_BYTES,
        "operation id",
    )?;
    if receipt.attempt == 0 || receipt.cursor == 0 {
        return Err("child report receipt attempt and cursor must be positive".to_string());
    }
    Ok(())
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

fn validate_operation_text(value: &str, max_bytes: usize, label: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(format!(
            "{label} is empty, oversized, or contains control characters"
        ));
    }
    Ok(())
}

fn validate_operation_result(value: &str) -> Result<(), String> {
    if value.len() > MAX_OPERATION_RESULT_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(
            "operation result is oversized or contains unsupported control characters".to_string(),
        );
    }
    Ok(())
}

fn validate_operation_prompt(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > MAX_OPERATION_PROMPT_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(
            "operation prompt is empty, oversized, or contains control characters".to_string(),
        );
    }
    Ok(())
}

fn session_values(path: &Path) -> Result<Vec<Value>, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read Session: {error}"))?;
    Ok(bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .collect())
}

fn latest_operation(path: &Path, operation_id: &str) -> Result<Option<SessionOperation>, String> {
    Ok(session_values(path)?.iter().rev().find_map(|record| {
        (record.get("kind").and_then(Value::as_str) == Some("operation")
            && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
            && record.get("operation_id").and_then(Value::as_str) == Some(operation_id))
        .then(|| serde_json::from_value(record.clone()).ok())
        .flatten()
    }))
}

fn operation_record_value(operation: &SessionOperation) -> Value {
    let mut record = serde_json::to_value(operation).expect("operation is serializable");
    record["kind"] = json!("operation");
    record
}

fn latest_child_control_request(
    path: &Path,
    operation_id: &str,
    parent_thread_id: Option<&str>,
    action: &str,
) -> Result<Option<Value>, String> {
    Ok(session_values(path)?.into_iter().rev().find(|record| {
        record.get("kind").and_then(Value::as_str) == Some("child_control_request")
            && record.get("operation_id").and_then(Value::as_str) == Some(operation_id)
            && parent_thread_id.is_none_or(|parent| {
                record.get("parent_thread_id").and_then(Value::as_str) == Some(parent)
            })
            && record.get("action").and_then(Value::as_str) == Some(action)
    }))
}

fn latest_pending_follow_up(
    path: &Path,
    operation_id: &str,
    parent_thread_id: Option<&str>,
) -> Result<Option<Value>, String> {
    let request =
        latest_child_control_request(path, operation_id, parent_thread_id, "queue_follow_up")?;
    Ok(request.filter(|record| {
        matches!(
            record.get("request_status").and_then(Value::as_str),
            Some("accepted" | "blocked")
        )
    }))
}

fn child_control_request_by_id(
    path: &Path,
    operation_id: &str,
    parent_thread_id: &str,
    action: &str,
    request_id: &str,
) -> Result<Option<Value>, String> {
    Ok(session_values(path)?.into_iter().rev().find(|record| {
        record.get("kind").and_then(Value::as_str) == Some("child_control_request")
            && record.get("operation_id").and_then(Value::as_str) == Some(operation_id)
            && record.get("parent_thread_id").and_then(Value::as_str) == Some(parent_thread_id)
            && record.get("action").and_then(Value::as_str) == Some(action)
            && record.get("request_id").and_then(Value::as_str) == Some(request_id)
    }))
}

fn control_request_transition(
    current: &Value,
    request_status: &str,
    status: &str,
    timestamp_ms: u64,
) -> Value {
    let mut next = current.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("request_status".to_string(), json!(request_status));
        object.insert("status".to_string(), json!(status));
        object.insert("timestamp_ms".to_string(), json!(timestamp_ms));
        object.remove("seq");
    }
    next
}

fn child_mutation_from_request(request: Value, fallback_attempt: u32) -> ChildTaskMutationResult {
    ChildTaskMutationResult {
        status: request
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("accepted")
            .to_string(),
        cursor: request
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        attempt: request
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|value| value.try_into().ok())
            .unwrap_or(fallback_attempt),
        attempt_kind: request
            .get("attempt_kind")
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        turn_id: request
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        duplicate: true,
        request_action: (request.get("action").and_then(Value::as_str) == Some("queue_follow_up"))
            .then_some(ChildControlRequestAction::QueueFollowUp),
        timestamp_ms: request
            .get("timestamp_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn child_mutation_from_operation(
    operation: &SessionOperation,
    cursor: u64,
    duplicate: bool,
    request_action: Option<ChildControlRequestAction>,
) -> ChildTaskMutationResult {
    ChildTaskMutationResult {
        status: operation.status.clone(),
        cursor,
        attempt: operation.attempt,
        attempt_kind: operation.attempt_kind,
        turn_id: operation.turn_id.clone(),
        duplicate,
        request_action,
        timestamp_ms: operation.timestamp_ms,
    }
}

fn validate_child_operation(
    store: &SessionStore,
    context: &ChildTaskContext,
    statuses: Option<&[&str]>,
) -> Result<SessionOperation, String> {
    let records = session_values(&store.path)?;
    if !records.iter().any(|record| {
        record.get("kind").and_then(Value::as_str) == Some("session_created")
            && record.get("forked_from").is_some()
    }) {
        return Err("child task Session lineage was not found".to_string());
    }
    let operation = records
        .iter()
        .rev()
        .find(|record| {
            record.get("kind").and_then(Value::as_str) == Some("operation")
                && record.get("operation_kind").and_then(Value::as_str) == Some("child_task")
                && record.get("operation_id").and_then(Value::as_str)
                    == Some(context.operation_id.as_str())
                && (statuses.is_some()
                    || record.get("attempt").and_then(Value::as_u64)
                        == Some(u64::from(context.attempt)))
        })
        .cloned()
        .map(serde_json::from_value::<SessionOperation>)
        .transpose()
        .map_err(|error| format!("cannot decode child task operation: {error}"))?
        .ok_or_else(|| "child task operation was not found".to_string())?;
    if operation.kind != "child_task"
        || operation.parent_thread_id.as_deref() != Some(context.parent_thread_id.as_str())
        || operation.attempt != context.attempt
    {
        return Err("child task parent or operation identity mismatch".to_string());
    }
    if statuses.is_some_and(|statuses| !statuses.contains(&operation.status.as_str())) {
        return Err(format!(
            "child task status is {}, expected {}",
            operation.status,
            statuses.unwrap_or_default().join(" or ")
        ));
    }
    Ok(operation)
}

pub(crate) fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl ExecutionJournalSink for SessionExecutionJournal {
    fn append(&mut self, entry: ExecutionJournalEntry) -> Result<u64, String> {
        let mut writer = self.writer_state.lock().unwrap();
        let timestamp = match &entry {
            ExecutionJournalEntry::Heartbeat { at_ms, .. } => *at_ms,
            _ => timestamp_ms(),
        };
        let mut record = match &entry {
            ExecutionJournalEntry::Checkpoint { checkpoint } => {
                if checkpoint.turn_id.as_str().is_empty() {
                    return Err("execution checkpoint turn id must not be empty".to_string());
                }
                let can_append = writer.messages.len() <= checkpoint.messages.len()
                    && writer
                        .messages
                        .iter()
                        .zip(&checkpoint.messages)
                        .all(|(old, new)| old == new);
                let (message_mode, messages) = if can_append {
                    ("append", &checkpoint.messages[writer.messages.len()..])
                } else {
                    ("replace", checkpoint.messages.as_slice())
                };
                json!({
                    "kind": "execution_checkpoint",
                    "turn_id": checkpoint.turn_id.as_str(),
                    "thread_id": self.thread_id,
                    "timestamp_ms": timestamp,
                    "input": checkpoint.input,
                    "next_model_step": checkpoint.next_model_step,
                    "final_text": checkpoint.final_text,
                    "phase": checkpoint.phase,
                    "message_mode": message_mode,
                    "messages": messages,
                })
            }
            _ => {
                let mut value = serde_json::to_value(&entry)
                    .map_err(|error| format!("cannot encode execution journal: {error}"))?;
                let kind = value
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "execution journal record is missing kind".to_string())?;
                value["kind"] = json!(format!("execution_{kind}"));
                value["thread_id"] = json!(self.thread_id);
                value["timestamp_ms"] = json!(timestamp);
                value
            }
        };
        let turn_id = execution_entry_turn_id(&entry);
        if let Some(current_turn) = writer.turn_id.as_ref()
            && current_turn != &turn_id
            && !matches!(entry, ExecutionJournalEntry::Checkpoint { .. })
        {
            return Err("execution journal entry belongs to another Turn".to_string());
        }
        if turn_id.as_str().is_empty() {
            return Err("execution journal turn id must not be empty".to_string());
        }
        record["session_id"] = json!(self.session_id);
        let seq = append_execution_record(&self.path, &self.append_lock, &mut record)?;
        if let ExecutionJournalEntry::Checkpoint { checkpoint } = &entry {
            writer.turn_id = Some(checkpoint.turn_id.clone());
            writer.messages.clone_from(&checkpoint.messages);
            writer.checkpoint_seq = seq;
        }
        storage::apply_execution_journal_entry(
            &mut self.execution_state.lock().unwrap(),
            seq,
            timestamp,
            entry,
        );
        Ok(seq)
    }
}

fn execution_entry_turn_id(entry: &ExecutionJournalEntry) -> TurnId {
    match entry {
        ExecutionJournalEntry::Checkpoint { checkpoint } => checkpoint.turn_id.clone(),
        ExecutionJournalEntry::ToolBatchStarted { batch } => batch.turn_id.clone(),
        ExecutionJournalEntry::ToolCallStarted { turn_id, .. }
        | ExecutionJournalEntry::ToolCallFinished { turn_id, .. }
        | ExecutionJournalEntry::ToolBatchSettled { turn_id, .. }
        | ExecutionJournalEntry::WaitingForContinue { turn_id, .. }
        | ExecutionJournalEntry::Resumed { turn_id, .. }
        | ExecutionJournalEntry::Heartbeat { turn_id, .. }
        | ExecutionJournalEntry::NeedsReconciliation { turn_id, .. }
        | ExecutionJournalEntry::Settled { turn_id } => turn_id.clone(),
    }
}

fn append_execution_record(
    path: &Path,
    append_lock: &Arc<Mutex<()>>,
    record: &mut Value,
) -> Result<u64, String> {
    let _guard = append_lock.lock().unwrap();
    let bytes = fs::read(path).map_err(|error| format!("cannot read Session journal: {error}"))?;
    let valid_bytes = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut next_seq = 1u64;
    for line in bytes[..valid_bytes]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let existing: Value = serde_json::from_slice(line)
            .map_err(|error| format!("invalid Session journal record: {error}"))?;
        let seq = existing
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| "Session journal record is missing seq".to_string())?;
        next_seq = seq.saturating_add(1);
    }
    let seq = next_seq;
    record["seq"] = json!(seq);
    let encoded = serde_json::to_vec(record)
        .map_err(|error| format!("cannot encode execution journal: {error}"))?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "execution checkpoint exceeds {MAX_RECORD_BYTES} byte limit"
        ));
    }
    let total_bytes = (valid_bytes as u64)
        .saturating_add(encoded.len() as u64)
        .saturating_add(1);
    if total_bytes > MAX_SESSION_BYTES {
        return Err(format!("Session exceeds {MAX_SESSION_BYTES} byte limit"));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("cannot open Session journal: {error}"))?;
    if valid_bytes < bytes.len() {
        file.set_len(valid_bytes as u64)
            .map_err(|error| format!("cannot discard incomplete Session tail: {error}"))?;
    }
    file.seek(SeekFrom::End(0))
        .map_err(|error| format!("cannot seek Session journal: {error}"))?;
    if let Err(error) = file
        .write_all(&encoded)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_data())
    {
        let rollback = file
            .set_len(valid_bytes as u64)
            .and_then(|()| file.sync_data());
        return match rollback {
            Ok(()) => Err(format!("cannot persist execution journal: {error}")),
            Err(rollback) => Err(format!(
                "cannot persist execution journal: {error}; rollback failed: {rollback}"
            )),
        };
    }
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::{TurnInput, TurnInputMode};

    fn exact_fork_metadata() -> SessionForkMetadata {
        SessionForkMetadata {
            context_policy: "exact".to_string(),
            context_before_bytes: 128,
            context_after_bytes: 128,
            compacted: false,
            method: "exact".to_string(),
        }
    }

    fn fork_child(
        root: &Path,
        parent: &OpenedSession,
        thread_id: &str,
        operation: SessionOperation,
    ) -> OpenedSession {
        let child = SessionStore::fork_from_checkpoint_with_operation(
            root,
            parent.store.session_id(),
            0,
            thread_id,
            &[],
            exact_fork_metadata(),
            Some(operation),
        )
        .unwrap();
        SessionStore::open(root, SessionRequest::Resume(child.session_id)).unwrap()
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
    fn session_control_freeze_and_resume_are_durable_and_request_bound() {
        let root = crate::test_support::test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let mut store = opened.store;

        let freezing = store
            .transition_session_control(SessionControlAction::Freeze, "freeze-1")
            .unwrap();
        assert_eq!(freezing.status, SessionControlStatus::Freezing);
        assert_eq!(freezing.request_id.as_deref(), Some("freeze-1"));
        drop(store);

        let mut resumed = SessionStore::open(&root, SessionRequest::Resume(session_id.clone()))
            .unwrap()
            .store;
        assert_eq!(resumed.session_control().unwrap(), freezing);
        assert!(
            resumed
                .transition_session_control(SessionControlAction::FreezeSettled, "stale-freeze")
                .is_err()
        );
        let frozen = resumed
            .transition_session_control(SessionControlAction::FreezeSettled, "freeze-1")
            .unwrap();
        assert_eq!(frozen.status, SessionControlStatus::Frozen);

        let resuming = resumed
            .transition_session_control(SessionControlAction::Resume, "resume-1")
            .unwrap();
        assert_eq!(resuming.status, SessionControlStatus::Resuming);
        assert_eq!(resuming.request_id.as_deref(), Some("resume-1"));
        let settled = resumed
            .transition_session_control(SessionControlAction::ResumeSettled, "resume-1")
            .unwrap();
        assert_eq!(settled.status, SessionControlStatus::Running);
        assert_eq!(
            resumed
                .transition_session_control(SessionControlAction::ResumeSettled, "resume-1")
                .unwrap(),
            settled,
            "settling the same resume request must be idempotent"
        );
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn child_report_receipts_are_idempotent_and_monotonic_across_reopen() {
        let root = crate::test_support::test_root();
        let opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_dir = opened.store.path().parent().unwrap().to_path_buf();
        let session_id = opened.store.session_id().to_string();
        let receipt = ChildReportReceipt {
            child_thread_id: "child-thread".to_string(),
            operation_id: "child:child-thread".to_string(),
            attempt: 1,
            cursor: 4,
        };
        SessionStore::record_child_report_receipt(&session_dir, receipt.clone()).unwrap();
        SessionStore::record_child_report_receipt(
            &session_dir,
            ChildReportReceipt {
                cursor: 2,
                ..receipt.clone()
            },
        )
        .unwrap();
        SessionStore::record_child_report_receipt(
            &session_dir,
            ChildReportReceipt {
                cursor: 7,
                ..receipt.clone()
            },
        )
        .unwrap();
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let receipts =
            SessionStore::read_child_report_receipts(resumed.store.path().parent().unwrap())
                .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].cursor, 7);
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn parent_freeze_control_source_survives_child_turn_settlement() {
        let root = crate::test_support::test_root();
        let parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let mut queued = SessionOperation::new("child:one", "child_task", "queued");
        queued.parent_thread_id = Some(parent.store.thread_id().to_string());
        queued.prompt = Some("inspect issue".to_string());
        let mut child = fork_child(&root, &parent, "child-thread", queued);
        let context = child.store.child_task_context().unwrap().unwrap();
        let mut running = SessionOperation::new("child:one", "child_task", "running");
        running.turn_id = Some("turn-active".to_string());
        child.store.record_operation(running).unwrap();

        let pausing = child
            .store
            .pause_child_task_from(
                &context,
                "parent-freeze-1",
                "turn-active",
                ChildTaskControlSource::ParentFreeze,
            )
            .unwrap();
        assert_eq!(pausing.status, "pausing");
        let mut cancelled = SessionOperation::new("child:one", "child_task", "cancelled");
        cancelled.turn_id = Some("turn-active".to_string());
        child.store.record_operation(cancelled).unwrap();
        let projected = child.store.operation("child:one").unwrap().unwrap();
        assert_eq!(projected.status, "paused");
        assert_eq!(
            projected.control_source,
            Some(ChildTaskControlSource::ParentFreeze)
        );

        drop(child);
        drop(parent);
        crate::test_support::remove_test_root(&root);
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
                    presentation: None,
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
                    presentation: None,
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
    fn operation_lifecycle_records_survive_session_resume() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let mut operation = SessionOperation::new("child:one", "child_task", "queued");
        operation.parent_thread_id = Some("parent".to_string());
        opened.store.record_operation(operation).unwrap();
        let mut completed = SessionOperation::new("child:one", "child_task", "completed");
        completed.turn_id = Some("turn-child".to_string());
        completed.result = Some("bounded result".to_string());
        opened.store.record_operation(completed).unwrap();
        let path = opened.store.path().to_path_buf();
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("\"operation_kind\":\"child_task\""));
        assert!(text.contains("\"status\":\"completed\""));
        assert_eq!(resumed.state.messages().len(), 0);
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn operation_results_preserve_formatting_and_fit_the_durable_bound() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let response = format!("\u{1}first line\n\t{}", "🤖".repeat(5_000));
        let result = SessionOperation::bounded_result(&response);
        assert!(result.len() <= MAX_OPERATION_RESULT_BYTES);
        assert!(result.starts_with(" first line\n\t"));

        let mut operation = SessionOperation::new("child:formatted", "child_task", "completed");
        operation.result = Some(result.clone());
        opened.store.record_operation(operation).unwrap();

        let record = fs::read_to_string(opened.store.path())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| {
                record.get("operation_id").and_then(Value::as_str) == Some("child:formatted")
            })
            .unwrap();
        assert_eq!(record["result"].as_str(), Some(result.as_str()));
        assert!(result.contains('\n'));
        assert!(result.contains('\t'));
        drop(opened);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn pause_settles_before_resume_and_retry_keeps_the_child_session() {
        let root = crate::test_support::test_root();
        let parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let mut queued = SessionOperation::new("child:one", "child_task", "queued");
        queued.parent_thread_id = Some(parent.store.thread_id().to_string());
        queued.execution_mode = Some("parallel".to_string());
        queued.prompt = Some("original task".to_string());
        let mut child = fork_child(&root, &parent, "child-thread", queued);
        let child_session_id = child.store.session_id().to_string();
        let mut running = SessionOperation::new("child:one", "child_task", "running");
        running.turn_id = Some("turn-one".to_string());
        child.store.record_operation(running).unwrap();
        let context = child.store.child_task_context().unwrap().unwrap();
        assert_eq!(
            child
                .store
                .pause_child_task(&context, "pause-1", "turn-one")
                .unwrap()
                .status,
            "pausing"
        );
        let mut interrupted = SessionOperation::new("child:one", "child_task", "cancelled");
        interrupted.turn_id = Some("turn-one".to_string());
        child.store.record_operation(interrupted).unwrap();
        let paused = child.store.operation("child:one").unwrap().unwrap();
        assert_eq!(paused.status, "paused");
        assert_eq!(paused.attempt, 1);
        let follow_up = child
            .store
            .queue_child_follow_up(
                &context,
                "follow-up-after-retry",
                "apply the remaining review feedback".to_string(),
            )
            .unwrap();
        assert_eq!(follow_up.status, "queued");
        assert_eq!(follow_up.attempt, 1);

        let resumed = child.store.resume_child_task(&context, "resume-1").unwrap();
        assert_eq!(resumed.status, "queued");
        assert_eq!(resumed.attempt, 1);
        let mut resumed_turn = SessionOperation::new("child:one", "child_task", "running");
        resumed_turn.turn_id = Some("turn-resumed".to_string());
        child.store.record_operation(resumed_turn).unwrap();
        let mut failed = SessionOperation::new("child:one", "child_task", "failed");
        failed.turn_id = Some("turn-resumed".to_string());
        failed.error = Some("model turn failed".to_string());
        child.store.record_operation(failed).unwrap();
        let retry = child.store.retry_child_task(&context, "retry-1").unwrap();
        assert_eq!(retry.status, "queued");
        assert_eq!(retry.attempt, 2);
        assert_eq!(retry.attempt_kind, Some(ChildTaskAttemptKind::Retry));
        let mut successful_retry = SessionOperation::new("child:one", "child_task", "running");
        successful_retry.attempt = 2;
        successful_retry.attempt_kind = Some(ChildTaskAttemptKind::Retry);
        successful_retry.turn_id = Some("turn-retry-2".to_string());
        child.store.record_operation(successful_retry).unwrap();
        let mut completed_retry = SessionOperation::new("child:one", "child_task", "completed");
        completed_retry.attempt = 2;
        completed_retry.attempt_kind = Some(ChildTaskAttemptKind::Retry);
        completed_retry.turn_id = Some("turn-retry-2".to_string());
        child.store.record_operation(completed_retry).unwrap();
        let promoted = child.store.operation("child:one").unwrap().unwrap();
        assert_eq!(promoted.status, "queued");
        assert_eq!(promoted.attempt, 3);
        assert_eq!(promoted.attempt_kind, Some(ChildTaskAttemptKind::FollowUp));
        assert_eq!(
            promoted.prompt.as_deref(),
            Some("apply the remaining review feedback")
        );
        assert_eq!(child.store.session_id(), child_session_id);
        drop(child);
        drop(parent);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn checkpoint_resume_keeps_child_operation_attached_to_original_turn() {
        let root = crate::test_support::test_root();
        let parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let mut queued = SessionOperation::new("child:one", "child_task", "queued");
        queued.parent_thread_id = Some(parent.store.thread_id().to_string());
        queued.prompt = Some("original task".to_string());
        let mut child = fork_child(&root, &parent, "child-thread", queued);
        let mut failed = SessionOperation::new("child:one", "child_task", "failed");
        failed.turn_id = Some("turn-resume".to_string());
        failed.error = Some("temporary model outage".to_string());
        child.store.record_operation(failed).unwrap();

        let turn_id = TurnId("turn-resume".to_string());
        let messages = vec![Message::User {
            text: "original task".to_string(),
        }];
        let mut journal = child.store.execution_journal(&messages);
        journal
            .append(ExecutionJournalEntry::Checkpoint {
                checkpoint: ExecutionCheckpoint {
                    turn_id: turn_id.clone(),
                    input: TurnInput {
                        mode: TurnInputMode::Start,
                        text: "original task".to_string(),
                        selected_skills: Vec::new(),
                        workflow: None,
                        model_selection: None,
                        reasoning_selection: None,
                        reasoning_effort: None,
                    },
                    messages: messages.clone(),
                    next_model_step: 1,
                    final_text: String::new(),
                    phase: ExecutionPhase::ModelRequest,
                },
            })
            .unwrap();
        journal
            .append(ExecutionJournalEntry::WaitingForContinue {
                turn_id: turn_id.clone(),
                reason: "temporary_model_error".to_string(),
            })
            .unwrap();

        let context = child.store.child_task_context().unwrap().unwrap();
        let resumed = child
            .store
            .resume_child_execution_from(
                &context,
                "resume-checkpoint-1",
                turn_id.as_str(),
                ChildTaskControlSource::UserPanel,
            )
            .unwrap();
        assert_eq!(resumed.status, "running");
        assert_eq!(resumed.attempt, 1);
        assert_eq!(resumed.turn_id.as_deref(), Some(turn_id.as_str()));

        let attached_operation = child
            .store
            .operation_for_turn(turn_id.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(attached_operation.operation_id, "child:one");
        assert_eq!(attached_operation.status, "running");
        assert_eq!(attached_operation.attempt, 1);

        let duplicate = child
            .store
            .resume_child_execution_from(
                &context,
                "resume-checkpoint-1",
                turn_id.as_str(),
                ChildTaskControlSource::UserPanel,
            )
            .unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.turn_id.as_deref(), Some(turn_id.as_str()));

        drop(child);
        drop(parent);
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn child_reports_are_idempotent_and_lifecycle_snapshots_keep_parent_identity() {
        let root = crate::test_support::test_root();
        let parent = SessionStore::open(&root, SessionRequest::New).unwrap();
        let mut queued = SessionOperation::new("child:one", "child_task", "queued");
        queued.parent_thread_id = Some(parent.store.thread_id().to_string());
        queued.group_id = Some("batch".to_string());
        queued.execution_mode = Some("parallel".to_string());
        queued.sequence = Some(2);
        queued.prompt = Some("inspect issue".to_string());
        let mut child = fork_child(&root, &parent, "child-thread", queued);
        let child_session_id = child.store.session_id().to_string();
        let mut running = SessionOperation::new("child:one", "child_task", "running");
        running.attempt = 1;
        running.turn_id = Some("turn-active".to_string());
        child.store.record_operation(running).unwrap();
        let context = child.store.child_task_context().unwrap().unwrap();
        assert_eq!(context.parent_thread_id, parent.store.thread_id());
        let first = child
            .store
            .record_child_report(&context, "call-1", "found the failing branch")
            .unwrap();
        let retry = child
            .store
            .record_child_report(&context, "call-1", "found the failing branch")
            .unwrap();
        assert_eq!(first.status, retry.status);
        assert_eq!(first.cursor, retry.cursor);
        assert!(retry.duplicate);
        let reservation = child
            .store
            .transition_child_steer_request(
                &context,
                "control-1",
                "turn-active",
                ChildSteerRequestStep::Reserve,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(reservation.status, "pending");
        drop(child);
        let mut child =
            SessionStore::open(&root, SessionRequest::Resume(child_session_id)).unwrap();
        let replay = child
            .store
            .transition_child_steer_request(
                &context,
                "control-1",
                "turn-active",
                ChildSteerRequestStep::Reserve,
                None,
            )
            .unwrap()
            .unwrap();
        assert!(replay.duplicate);
        assert_eq!(replay.status, "pending");
        let accepted = child
            .store
            .transition_child_steer_request(
                &context,
                "control-1",
                "turn-active",
                ChildSteerRequestStep::Accept,
                Some("steered"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(accepted.status, "steered");
        let mut next_attempt = SessionOperation::new("child:one", "child_task", "running");
        next_attempt.attempt = 2;
        child.store.record_operation(next_attempt).unwrap();
        let late_report = child
            .store
            .record_child_report(&context, "call-late", "attempt one had completed")
            .unwrap();
        assert_eq!(late_report.attempt, 1);
        let text = fs::read_to_string(child.store.path()).unwrap();
        assert_eq!(text.matches("\"kind\":\"child_report\"").count(), 2);
        assert!(text.contains("\"operation_group_id\":\"batch\""));
        assert!(
            child
                .store
                .record_child_report(
                    &ChildTaskContext {
                        parent_thread_id: "other".to_string(),
                        ..context
                    },
                    "call-2",
                    "bad lineage"
                )
                .is_err()
        );
        drop(child);
        drop(parent);
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
        let selection = ModelSelection {
            provider_id: "kimi".to_string(),
            model_id: "kimi-k2".to_string(),
        };
        parent
            .store
            .set_model_settings(
                Some(selection.clone()),
                Some(ReasoningSelection::Level("high".to_string())),
            )
            .unwrap();
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
                    presentation: None,
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
        let child = SessionStore::open(&root, SessionRequest::Resume(info.session_id.clone()))
            .expect("forked Session should resume");
        assert!(child.store.is_forked());
        assert!(child.store.items().is_empty());
        assert_eq!(child.store.model_selection(), Some(&selection));
        assert_eq!(
            child.store.reasoning_selection(),
            Some(&ReasoningSelection::Level("high".to_string()))
        );
        drop(child);
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
    fn settled_checkpoint_can_be_read_while_parent_session_is_open() {
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
                    presentation: None,
                    checkpoint: &messages,
                },
            )
            .unwrap();

        let (checkpoint_seq, checkpoint) =
            SessionStore::read_settled_checkpoint(&root, &parent_id).unwrap();
        assert_eq!(checkpoint_seq, parent.store.checkpoint_seq());
        assert_eq!(checkpoint, messages);

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
                    presentation: None,
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
                    presentation: None,
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
                    presentation: None,
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
        let mut presentation = TurnPresentation::from_workflow(Some(&TurnWorkflow {
            kind: mini_agent_protocol::TurnWorkflowKind::SkillGroup,
            id: "knowledge-work".to_string(),
            mode: mini_agent_protocol::TurnWorkflowMode::Auto,
        }));
        presentation.push(TurnPresentationActivity::skill_group_activated(
            0,
            "knowledge-work",
            "builtin",
        ));
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
                    presentation: Some(&presentation),
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
        assert!(session_text.contains("knowledge-work"));
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
    fn turn_source_is_persisted_and_restored_with_the_turn_presentation() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();
        let messages = vec![Message::User {
            text: "child update prompt".to_string(),
        }];
        let presentation =
            TurnPresentation::default().with_turn_source(Some(TurnSource::ChildWakeup));
        opened
            .store
            .record_turn_with_id(
                "turn-wakeup",
                TurnCommit {
                    started_at_ms: timestamp_ms(),
                    prompt: "child update prompt",
                    status: TurnStatus::Completed,
                    steps: 1,
                    error: None,
                    messages: &messages,
                    tool_arguments: &[],
                    presentation: Some(&presentation),
                    checkpoint: &messages,
                },
            )
            .unwrap();
        assert_eq!(
            opened.store.turn_source("turn-wakeup"),
            Some(TurnSource::ChildWakeup)
        );
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert!(!resumed.store.is_forked());
        assert_eq!(
            resumed.store.turn_source("turn-wakeup"),
            Some(TurnSource::ChildWakeup)
        );
        crate::test_support::remove_test_root(&root);
    }

    #[test]
    fn continuation_preference_survives_session_resume() {
        let root = crate::test_support::test_root();
        let mut opened = SessionStore::open(&root, SessionRequest::New).unwrap();
        let session_id = opened.store.session_id().to_string();

        assert_eq!(opened.store.continuation_mode(), None);
        let selection = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-r1".to_string(),
        };
        opened.store.set_continuation_mode("continuous").unwrap();
        opened
            .store
            .set_model_settings(
                Some(selection.clone()),
                Some(ReasoningSelection::ApiDefault),
            )
            .unwrap();
        assert_eq!(opened.store.continuation_mode(), Some("continuous"));
        drop(opened);

        let resumed = SessionStore::open(&root, SessionRequest::Resume(session_id)).unwrap();
        assert_eq!(resumed.store.continuation_mode(), Some("continuous"));
        assert_eq!(resumed.store.model_selection(), Some(&selection));
        assert_eq!(
            resumed.store.reasoning_selection(),
            Some(&ReasoningSelection::ApiDefault)
        );
        drop(resumed);
        crate::test_support::remove_test_root(&root);
    }
}
