//! Versioned client/server contracts for exposing a mini-agent Thread.
//!
//! These types are deliberately separate from the transport-neutral core
//! protocol. They describe JSON-RPC method names, request correlation, and
//! server notifications; they do not define Harness execution semantics.

use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::Message;
use mini_agent_protocol::StopReason;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStatus;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnStatus;
pub use mini_agent_protocol::{
    ActionGrantKey, ActionGrantScope, ApprovalOutcome, ApprovalPolicy, ToolApprovalResolution,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

mod thread_item;

pub use thread_item::ItemStatus;
pub use thread_item::ThreadItem;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u32 = 1;

pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_INITIALIZED: &str = "initialized";
pub const METHOD_THREAD_START: &str = "thread/start";
pub const METHOD_THREAD_LIST: &str = "thread/list";
pub const METHOD_THREAD_FORK: &str = "thread/fork";
pub const METHOD_THREAD_RESUME: &str = "thread/resume";
pub const METHOD_THREAD_READ: &str = "thread/read";
pub const METHOD_THREAD_CLOSE: &str = "thread/close";
pub const METHOD_THREAD_SETTINGS_UPDATE: &str = "thread/settings/update";
pub const METHOD_THREAD_SETTINGS_UPDATED: &str = "thread/settings/updated";
pub const METHOD_TURN_START: &str = "turn/start";
pub const METHOD_TURN_READ: &str = "turn/read";
pub const METHOD_TURN_STEER: &str = "turn/steer";
pub const METHOD_TURN_INTERRUPT: &str = "turn/interrupt";
pub const METHOD_TURN_EVENT: &str = "turn/event";
pub const METHOD_TURN_EVENTS: &str = "turn/events";
pub const METHOD_ITEM_STARTED: &str = "item/started";
pub const METHOD_ITEM_COMPLETED: &str = "item/completed";
pub const METHOD_THREAD_ITEMS_LIST: &str = "thread/items/list";
pub const METHOD_APPROVAL_REQUEST: &str = "approval/request";
pub const METHOD_APPROVAL_RESOLVED: &str = "approval/resolved";
pub const METHOD_APPROVAL_RESPOND: &str = "approval/respond";
pub const METHOD_THREAD_GOAL_SET: &str = "thread/goal/set";
pub const METHOD_THREAD_GOAL_GET: &str = "thread/goal/get";
pub const METHOD_THREAD_GOAL_CLEAR: &str = "thread/goal/clear";
pub const METHOD_THREAD_GOAL_UPDATED: &str = "thread/goal/updated";
pub const METHOD_THREAD_GOAL_CLEARED: &str = "thread/goal/cleared";
pub const METHOD_SESSION_INFO: &str = "session/info";
pub const METHOD_SESSION_FORK: &str = "session/fork";
pub const METHOD_SESSION_NOTEBOOK_READ: &str = "session/notebook/read";
pub const METHOD_SESSION_NOTEBOOK_WRITE: &str = "session/notebook/write";
pub const METHOD_SESSION_NOTEBOOK_FORGET: &str = "session/notebook/forget";
pub const METHOD_SESSION_NOTEBOOK_UPDATED: &str = "session/notebook/updated";
pub const METHOD_WORLD_STATE: &str = "world/state";
pub const METHOD_WORLD_REFRESH: &str = "world/refresh";
pub const METHOD_WORLD_SET_EXECUTION: &str = "world/set_execution";
pub const METHOD_MCP_STATUS: &str = "mcp/status";
pub const METHOD_MCP_RETRY: &str = "mcp/retry";
pub const METHOD_RUNTIME_STATUS: &str = "runtime/status";
pub const METHOD_RUNTIME_STATUS_UPDATED: &str = "runtime/status/updated";
pub const METHOD_CHECKPOINT_COMMITTED: &str = "checkpoint/committed";
pub const METHOD_GOAL_VERIFICATION_STARTED: &str = "goal/verification_started";
pub const METHOD_GOAL_VERIFICATION_COMPLETED: &str = "goal/verification_completed";
pub const METHOD_GOAL_VERIFICATION_FAILED: &str = "goal/verification_failed";
pub const METHOD_GOAL_CONTINUATION_QUEUED: &str = "goal/continuation_queued";
pub const METHOD_GOAL_CONTINUATION_STARTED: &str = "goal/continuation_started";
pub const METHOD_PLAN_UPDATED: &str = "plan/updated";
pub const METHOD_PLAN_CLEANUP_STARTED: &str = "plan/cleanup_started";
pub const METHOD_PLAN_CLEANUP_COMPLETED: &str = "plan/cleanup_completed";
pub const METHOD_PLAN_CLEANUP_FAILED: &str = "plan/cleanup_failed";

pub const SESSION_FORK_CONFLICT_CODE: i32 = -32001;

/// A JSON-RPC request or notification received by the app-server.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn request(id: impl Into<Value>, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: Some(JSONRPC_VERSION.to_string()),
            id: Some(id.into()),
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Some(JSONRPC_VERSION.to_string()),
            id: None,
            method: method.into(),
            params,
        }
    }

    pub fn decode_params<T: for<'de> Deserialize<'de>>(&self) -> Result<T, JsonRpcError> {
        self.params
            .clone()
            .ok_or_else(|| JsonRpcError::invalid_params("params are required"))
            .and_then(|params| {
                serde_json::from_value(params)
                    .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
            })
    }
}

/// A JSON-RPC response with either a result or an error.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// Machine-readable reason for refusing to reuse a child Thread identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionForkConflictData {
    ParentLineage {
        #[serde(rename = "childThreadId")]
        child_thread_id: String,
    },
    ContextPolicy {
        #[serde(rename = "childThreadId")]
        child_thread_id: String,
        #[serde(rename = "requestedContextPolicy")]
        requested_context_policy: String,
        #[serde(rename = "existingContextPolicy")]
        existing_context_policy: String,
    },
}

impl JsonRpcResponse {
    pub fn result(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC error object returned by the app-server.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Server-assigned identity and ordering for one accepted App Server action.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionMetadata {
    pub action_id: u64,
    pub action_sequence: u64,
    pub state_revision: u64,
}

/// Result envelope for an App Server action or query.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult<T> {
    pub value: T,
    pub action_id: u64,
    pub action_sequence: u64,
    pub state_revision: u64,
}

/// The bounded phase of the App Server's currently observable operation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    #[default]
    Idle,
    StartingTurn,
    Model,
    Tool,
    WaitingApproval,
    Stopping,
    Compaction,
    Persisting,
    GoalVerification,
    GoalContinuationQueued,
    Resuming,
    Completed,
    Failed,
}

/// A live, bounded snapshot of one App Server runtime operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub phase: RuntimePhase,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub operation_id: Option<String>,
    pub checkpoint_seq: Option<u64>,
    pub state_revision: u64,
    pub timestamp_ms: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusParams {
    pub thread_id: ThreadId,
}

/// A bounded workflow lifecycle record. The method name identifies the
/// transition; optional fields carry only the correlation data needed to
/// reconcile a live client with the canonical runtime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLifecycleNotification {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub operation_id: String,
    pub checkpoint_seq: Option<u64>,
    pub state_revision: u64,
    pub timestamp_ms: u64,
    pub error: Option<String>,
    pub goal_id: Option<String>,
    pub milestone: Option<usize>,
    pub total_milestones: Option<usize>,
    pub plan_active: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEventsParams {
    pub thread_id: ThreadId,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEventsResult {
    pub data: Vec<TurnEventNotification>,
    pub next_cursor: Option<u64>,
    pub oldest_sequence: Option<u64>,
    pub has_gap: bool,
}

impl JsonRpcError {
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(-32700, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(-32000, message)
    }

    pub fn session_fork_conflict(data: SessionForkConflictData) -> Self {
        Self {
            code: SESSION_FORK_CONFLICT_CODE,
            message: "session fork conflicts with existing child".to_string(),
            data: Some(serde_json::to_value(data).expect("fork conflict is serializable")),
        }
    }

    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub notifications: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_name: String,
    pub client_version: String,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<CapabilityProviderSelection>,
}

/// Optional, bounded provider IDs requested at service startup.
///
/// The IDs are resolved against the host's local capability registry. This
/// type carries selectors only; credentials, tools, and provider instances
/// never cross the JSON-RPC boundary.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProviderSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub steering: bool,
    #[serde(default)]
    pub thread_resume: bool,
    #[serde(default)]
    pub thread_fork: bool,
    #[serde(default)]
    pub session_fork: bool,
    #[serde(default)]
    pub thread_read: bool,
    #[serde(default)]
    pub thread_close: bool,
    #[serde(default)]
    pub thread_settings_update: bool,
    #[serde(default)]
    pub turn_read: bool,
    #[serde(default)]
    pub thread_list: bool,
    #[serde(default)]
    pub thread_items_list: bool,
    #[serde(default)]
    pub item_lifecycle_notifications: bool,
    #[serde(default)]
    pub approval_requests: bool,
    #[serde(default)]
    pub workflows: bool,
    #[serde(default)]
    pub runtime_management: bool,
    #[serde(default)]
    pub runtime_status: bool,
    #[serde(default)]
    pub event_replay: bool,
    #[serde(default)]
    pub workflow_lifecycle_notifications: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_name: String,
    pub server_version: String,
    pub capabilities: ServerCapabilities,
    pub capability_manifest: CapabilityManifest,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationMode {
    pub mode: CollaborationModeKind,
}

impl Default for CollaborationMode {
    fn default() -> Self {
        Self {
            mode: CollaborationModeKind::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CollaborationModeKind {
    Default,
    Plan,
}

/// Thread-owned continuation policy. Goal Runtime is deliberately not a
/// selectable value here: an active Goal temporarily owns its own milestone
/// loop and limits.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationMode {
    #[default]
    Manual,
    Continuous,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdateParams {
    pub thread_id: ThreadId,
    pub collaboration_mode: CollaborationMode,
    /// Optional replacement for the model-visible Builtin tool selection.
    /// Omission keeps the current Thread selection unchanged.
    pub builtin_tools: Option<Vec<String>>,
    /// Optional Thread loop setting. Omission preserves the current value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_mode: Option<ContinuationMode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdateResult {
    pub collaboration_mode: CollaborationMode,
    pub builtin_tools: Vec<String>,
    pub continuation_mode: ContinuationMode,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdatedNotification {
    pub thread_id: ThreadId,
    pub collaboration_mode: CollaborationMode,
    pub builtin_tools: Vec<String>,
    pub continuation_mode: ContinuationMode,
    pub state_revision: u64,
}

/// The public lifecycle state of one Thread-owned Goal.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadGoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

/// Bounded public projection of a Thread Goal.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoal {
    pub thread_id: ThreadId,
    pub objective: String,
    pub status: ThreadGoalStatus,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub current_milestone: usize,
    #[serde(default)]
    pub total_milestones: usize,
    #[serde(default)]
    pub loop_count: usize,
    #[serde(default)]
    pub last_verifier_score: Option<u32>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default = "default_verification_status")]
    pub verification_status: String,
}

fn default_verification_status() -> String {
    "idle".to_string()
}

/// Sets or replaces a Thread Goal. A running Goal cannot be replaced
/// implicitly; clear it first so the transition remains observable.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalSetParams {
    pub thread_id: ThreadId,
    pub objective: Option<String>,
    pub status: Option<ThreadGoalStatus>,
    #[serde(
        default,
        deserialize_with = "deserialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub token_budget: Option<Option<i64>>,
}

fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalSetResponse {
    pub goal: ThreadGoal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalGetParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalGetResponse {
    pub goal: Option<ThreadGoal>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalClearParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalClearResponse {
    pub cleared: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalUpdatedNotification {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub goal: ThreadGoal,
    pub state_revision: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalClearedNotification {
    pub thread_id: ThreadId,
    pub state_revision: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoResult {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub resumed: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkContextPolicy {
    #[default]
    Exact,
    Compact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkCompactionMethod {
    Exact,
    ModelSummary,
    Mechanical,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkParams {
    pub source_thread_id: ThreadId,
    pub new_thread_id: ThreadId,
    #[serde(default)]
    pub context_policy: ForkContextPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_sequence: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkResult {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub parent_session_id: String,
    pub parent_checkpoint_seq: u64,
    pub session_bytes: u64,
    pub context_before_bytes: usize,
    pub context_after_bytes: usize,
    pub compacted: bool,
    pub method: ForkCompactionMethod,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotebookReadParams {
    pub thread_id: ThreadId,
    #[serde(default)]
    pub scope: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotebookWriteParams {
    pub thread_id: ThreadId,
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub append: bool,
    #[serde(default)]
    pub importance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<Value>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotebookForgetParams {
    pub thread_id: ThreadId,
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldStateResult {
    pub workspace: String,
    pub status: Value,
    pub lines: Vec<String>,
    pub context: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldRefreshResult {
    pub changed: bool,
    pub state: WorldStateResult,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSetExecutionResult {
    pub changed: bool,
    pub state: WorldStateResult,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSetExecutionParams {
    pub access: AccessScope,
    pub policy: ApprovalPolicy,
}

/// Filesystem reach that the Host may consider for a tool call. This is an
/// access boundary, not an approval decision: FullMachine never turns a
/// denied or otherwise unsafe action into an allowed action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessScope {
    Project,
    FullMachine,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusResult {
    pub enabled_servers: Vec<String>,
    pub inactive_servers: Vec<String>,
    pub tool_count: usize,
    pub retry_available: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRetryResult {
    pub enabled_servers: Vec<String>,
    pub inactive_servers: Vec<String>,
    pub diagnostics: Vec<String>,
    pub tool_count: usize,
}

/// Bounded, secret-free capability metadata advertised at service startup.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    pub model_provider: String,
    pub tool_provider: String,
    pub extension_provider: String,
    pub policy_provider: String,
    pub enabled: Vec<String>,
    pub disabled: Vec<DisabledCapability>,
    pub extension_depth: String,
    pub selected_extensions: Vec<String>,
    pub prompt_sources: Vec<String>,
    pub rule_sources: Vec<String>,
    pub rule_source_status: Vec<RuleSourceStatus>,
    pub prompt_source_fingerprints: Vec<SourceFingerprint>,
    pub rule_source_fingerprints: Vec<SourceFingerprint>,
    pub prompt_rule_precedence: Vec<String>,
    pub rule_resolution: String,
    pub rule_conflicts: Vec<String>,
    pub rule_policy: RulePolicy,
    pub context_limits: ContextLimits,
    pub sandbox: String,
    pub security: String,
    #[serde(default)]
    pub builtin_skill_groups: Vec<BuiltinSkillGroup>,
    #[serde(default)]
    pub available_skills: Vec<AvailableSkill>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinSkillGroup {
    pub id: String,
    pub version: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableSkill {
    pub name: String,
    #[serde(default)]
    pub qualified_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub description: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLimits {
    pub max_context_bytes: usize,
    pub max_context_item_bytes: usize,
    pub max_user_input_bytes: usize,
    pub max_model_response_bytes: usize,
    pub max_tool_output_bytes: usize,
    pub max_tool_calls_per_step: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePolicy {
    pub workspace_write: bool,
    pub shell_execution: bool,
    pub workflow_scope: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSourceStatus {
    pub source: String,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFingerprint {
    pub source: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisabledCapability {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResult {
    pub data: Vec<ThreadId>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkParams {
    pub source_thread_id: ThreadId,
    pub new_thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: ThreadId,
    pub checkpoint: ThreadReadResult,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResult {
    pub thread_id: ThreadId,
    pub status: ThreadStatus,
    pub messages: Vec<Message>,
    pub context_revision: u64,
    pub next_turn_number: u64,
    pub last_turn_id: Option<TurnId>,
    pub next_event_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCloseParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: ThreadId,
    pub input: TurnInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_sequence: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadParams {
    pub turn_id: TurnId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub stop_reason: Option<StopReason>,
    pub final_text: Option<String>,
    pub steps: usize,
    pub messages: Vec<Message>,
    pub items: Vec<ThreadItem>,
    pub error: Option<String>,
}

/// A lifecycle notification for one ThreadItem becoming visible.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemStartedNotification {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub item: ThreadItem,
    pub started_at_ms: u64,
}

/// A lifecycle notification carrying the authoritative completed ThreadItem.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCompletedNotification {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub item: ThreadItem,
    pub completed_at_ms: u64,
}

/// A bounded invalidation notice for a Notebook mutation. The contents stay
/// behind the existing notebook read API and are never copied into the event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookUpdatedNotification {
    pub thread_id: ThreadId,
    pub revision: u64,
    pub changed_keys: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemSortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemsListParams {
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<ItemSortDirection>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemEntry {
    pub turn_id: TurnId,
    pub item: ThreadItem,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItemsListResult {
    pub data: Vec<ThreadItemEntry>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPathKind {
    Project,
    Machine,
    Explicit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPathScope {
    pub kind: ApprovalPathKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestNotification {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
    pub tool_name: Option<String>,
    pub action_class: String,
    pub action_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_key: Option<ActionGrantKey>,
    pub path_scope: ApprovalPathScope,
    pub access: AccessScope,
    pub policy: ApprovalPolicy,
    pub allowed_grant_scopes: Vec<ActionGrantScope>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResolvedNotification {
    pub request_id: String,
    pub outcome: ApprovalOutcome,
    pub grant_scope: Option<ActionGrantScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_revision: Option<u64>,
    pub session_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
    pub tool_name: Option<String>,
    pub action_class: String,
    pub action_summary: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRespondParams {
    pub request_id: String,
    pub decision: ApprovalDecision,
    pub grant_scope: Option<ActionGrantScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A server-to-client notification carrying an ordered core event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEventNotification {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub items: Vec<ThreadItem>,
    pub event: mini_agent_protocol::Event,
}

impl From<EventEnvelope> for TurnEventNotification {
    fn from(event: EventEnvelope) -> Self {
        let items = ThreadItem::from_event(&event);
        Self {
            thread_id: event.thread_id,
            turn_id: event.turn_id,
            sequence: event.sequence,
            item_id: event.item_id,
            items,
            event: event.event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::Event;

    #[test]
    fn request_and_response_round_trip() {
        let request = JsonRpcRequest::request(
            7,
            METHOD_TURN_START,
            serde_json::json!({"thread_id": "thread-1"}),
        );
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: JsonRpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);

        let response =
            JsonRpcResponse::result(Some(serde_json::json!(7)), serde_json::json!({"ok": true}));
        let decoded: JsonRpcResponse =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn notifications_omit_request_id() {
        let request = JsonRpcRequest::notification(METHOD_INITIALIZED, None);
        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("id").is_none());
    }

    #[test]
    fn notebook_update_notification_is_bounded_and_camel_case() {
        let value = serde_json::to_value(NotebookUpdatedNotification {
            thread_id: ThreadId::new("thread-1"),
            revision: 4,
            changed_keys: vec!["architecture_decision".to_string()],
        })
        .unwrap();
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["revision"], 4);
        assert_eq!(value["changedKeys"][0], "architecture_decision");
        assert!(value.get("content").is_none());
    }

    #[test]
    fn approval_notifications_preserve_turn_and_call_correlation() {
        let request = serde_json::to_value(ApprovalRequestNotification {
            request_id: "approval-1".to_string(),
            project_id: Some("project-1".to_string()),
            workspace_id: Some("workspace-1".to_string()),
            workspace_revision: Some(3),
            session_id: Some("session-1".to_string()),
            thread_id: Some(ThreadId::new("thread-1")),
            turn_id: Some(TurnId::new("turn-1")),
            call_id: Some("shell-call-1".to_string()),
            tool_name: Some("shell".to_string()),
            action_class: "shell_execute".to_string(),
            action_summary: "shell command `pwd`".to_string(),
            path_scope: ApprovalPathScope {
                kind: ApprovalPathKind::Machine,
                paths: Vec::new(),
            },
            access: AccessScope::FullMachine,
            action_key: None,
            policy: ApprovalPolicy::Interactive,
            allowed_grant_scopes: vec![ActionGrantScope::Once, ActionGrantScope::Project],
        })
        .unwrap();
        assert_eq!(request["requestId"], "approval-1");
        assert_eq!(request["workspaceRevision"], 3);
        assert_eq!(request["access"], "full_machine");
        assert_eq!(request["allowedGrantScopes"][0], "once");
        assert_eq!(request["turnId"], "turn-1");
        assert_eq!(request["callId"], "shell-call-1");

        let resolved = serde_json::to_value(ApprovalResolvedNotification {
            request_id: "approval-1".to_string(),
            outcome: ApprovalOutcome::Approved,
            grant_scope: Some(ActionGrantScope::Project),
            reason: Some("approved by test".to_string()),
            project_id: Some("project-1".to_string()),
            workspace_id: Some("workspace-1".to_string()),
            workspace_revision: Some(3),
            session_id: Some("session-1".to_string()),
            thread_id: Some(ThreadId::new("thread-1")),
            turn_id: Some(TurnId::new("turn-1")),
            call_id: Some("shell-call-1".to_string()),
            tool_name: Some("shell".to_string()),
            action_class: "shell_execute".to_string(),
            action_summary: "shell command `pwd`".to_string(),
        })
        .unwrap();
        assert_eq!(resolved["requestId"], "approval-1");
        assert_eq!(resolved["outcome"], "approved");
        assert_eq!(resolved["grantScope"], "project");
        assert_eq!(resolved["reason"], "approved by test");
        assert_eq!(resolved["turnId"], "turn-1");
        assert_eq!(resolved["callId"], "shell-call-1");
    }

    #[test]
    fn thread_goal_contract_uses_codex_shaped_wire_names() {
        let params = serde_json::to_value(ThreadGoalSetParams {
            thread_id: ThreadId::new("thread-1"),
            objective: Some("ship it".to_string()),
            status: Some(ThreadGoalStatus::Active),
            token_budget: Some(Some(1000)),
        })
        .unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["tokenBudget"], 1000);
        assert_eq!(
            serde_json::to_value(ThreadGoalStatus::UsageLimited).unwrap(),
            "usageLimited"
        );
        let cleared: ThreadGoalSetParams = serde_json::from_value(serde_json::json!({
            "threadId": "thread-1",
            "tokenBudget": null
        }))
        .unwrap();
        assert_eq!(cleared.token_budget, Some(None));
    }

    #[test]
    fn app_server_params_use_camel_case() {
        let value = serde_json::to_value(InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".to_string(),
            client_version: "0".to_string(),
            capabilities: ClientCapabilities::default(),
            providers: Some(CapabilityProviderSelection {
                model: Some("openai".to_string()),
                tools: Some("builtin".to_string()),
                extensions: Some("builtin".to_string()),
                policy: Some("builtin".to_string()),
            }),
        })
        .unwrap();
        assert!(value.get("protocolVersion").is_some());
        assert_eq!(value["providers"]["model"], "openai");
        assert_eq!(value["providers"]["tools"], "builtin");
        assert!(value.get("protocol_version").is_none());
    }

    #[test]
    fn session_fork_contract_keeps_policy_and_lineage_bounded() {
        let defaults: SessionForkParams = serde_json::from_value(serde_json::json!({
            "sourceThreadId": "source",
            "newThreadId": "child"
        }))
        .unwrap();
        assert_eq!(defaults.context_policy, ForkContextPolicy::Exact);

        let params = serde_json::to_value(SessionForkParams {
            source_thread_id: ThreadId::new("source"),
            new_thread_id: ThreadId::new("child"),
            context_policy: ForkContextPolicy::Compact,
            operation_id: None,
            operation_attempt: None,
            operation_prompt: None,
            operation_group_id: None,
            execution_mode: None,
            group_sequence: None,
        })
        .unwrap();
        assert_eq!(params["sourceThreadId"], "source");
        assert_eq!(params["newThreadId"], "child");
        assert_eq!(params["contextPolicy"], "compact");
        assert!(params.get("source_thread_id").is_none());

        let result = serde_json::to_value(SessionForkResult {
            session_id: "s-child".to_string(),
            thread_id: "child".to_string(),
            path: "sessions/s-child/session.jsonl".to_string(),
            parent_session_id: "s-parent".to_string(),
            parent_checkpoint_seq: 9,
            session_bytes: 1024,
            context_before_bytes: 8192,
            context_after_bytes: 4096,
            compacted: true,
            method: ForkCompactionMethod::ModelSummary,
        })
        .unwrap();
        assert_eq!(result["sessionId"], "s-child");
        assert_eq!(result["parentCheckpointSeq"], 9);
        assert_eq!(result["contextAfterBytes"], 4096);
        assert_eq!(result["method"], "model_summary");

        let conflict =
            JsonRpcError::session_fork_conflict(SessionForkConflictData::ContextPolicy {
                child_thread_id: "child".to_string(),
                requested_context_policy: "compact".to_string(),
                existing_context_policy: "exact".to_string(),
            });
        assert_eq!(conflict.code, SESSION_FORK_CONFLICT_CODE);
        let conflict_data = conflict.data.unwrap();
        assert_eq!(conflict_data["kind"], "contextPolicy");
        assert_eq!(conflict_data["childThreadId"], "child");
    }

    #[test]
    fn action_result_uses_camel_case_metadata() {
        let value = serde_json::to_value(ActionResult {
            value: serde_json::json!({"ok": true}),
            action_id: 3,
            action_sequence: 4,
            state_revision: 5,
        })
        .unwrap();
        assert_eq!(value["actionId"], 3);
        assert_eq!(value["actionSequence"], 4);
        assert_eq!(value["stateRevision"], 5);
        assert!(value.get("action_id").is_none());
    }

    #[test]
    fn event_projection_preserves_identity_and_sequence() {
        let envelope = EventEnvelope::new(
            ThreadId::new("thread-1"),
            Some(TurnId::new("turn-1")),
            4,
            Event::RunFinished {
                stop_reason: mini_agent_protocol::StopReason::Completed,
                steps: 2,
            },
        );
        let notification = TurnEventNotification::from(envelope);
        assert_eq!(notification.thread_id, ThreadId::new("thread-1"));
        assert_eq!(notification.turn_id, Some(TurnId::new("turn-1")));
        assert_eq!(notification.sequence, 4);
        assert!(notification.items.is_empty());
    }

    #[test]
    fn thread_items_contract_uses_bounded_camel_case_fields() {
        let params = serde_json::to_value(ThreadItemsListParams {
            thread_id: ThreadId::new("thread-1"),
            turn_id: Some(TurnId::new("turn-1")),
            cursor: Some("2".to_string()),
            limit: Some(8),
            sort_direction: Some(ItemSortDirection::Desc),
        })
        .unwrap();
        assert_eq!(params["threadId"], "thread-1");
        assert_eq!(params["turnId"], "turn-1");
        assert_eq!(params["sortDirection"], "desc");
        assert!(params.get("thread_id").is_none());

        let notification = serde_json::to_value(ItemCompletedNotification {
            thread_id: ThreadId::new("thread-1"),
            turn_id: TurnId::new("turn-1"),
            item: ThreadItem::AgentMessage {
                id: "item-1".to_string(),
                text: "done".to_string(),
            },
            completed_at_ms: 10,
        })
        .unwrap();
        assert_eq!(notification["turnId"], "turn-1");
        assert_eq!(notification["completedAtMs"], 10);
        assert_eq!(notification["item"]["type"], "agentMessage");
    }
}
