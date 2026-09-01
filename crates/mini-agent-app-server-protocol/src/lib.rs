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
pub const METHOD_TURN_START: &str = "turn/start";
pub const METHOD_TURN_READ: &str = "turn/read";
pub const METHOD_TURN_STEER: &str = "turn/steer";
pub const METHOD_TURN_INTERRUPT: &str = "turn/interrupt";
pub const METHOD_TURN_EVENT: &str = "turn/event";
pub const METHOD_APPROVAL_REQUEST: &str = "approval/request";
pub const METHOD_APPROVAL_RESOLVED: &str = "approval/resolved";
pub const METHOD_APPROVAL_RESPOND: &str = "approval/respond";
pub const METHOD_THREAD_GOAL_SET: &str = "thread/goal/set";
pub const METHOD_THREAD_GOAL_GET: &str = "thread/goal/get";
pub const METHOD_THREAD_GOAL_CLEAR: &str = "thread/goal/clear";
pub const METHOD_THREAD_GOAL_UPDATED: &str = "thread/goal/updated";
pub const METHOD_THREAD_GOAL_CLEARED: &str = "thread/goal/cleared";
pub const METHOD_WORKFLOW_STATE: &str = "workflow/state";
pub const METHOD_WORKFLOW_GOAL_START: &str = "workflow/goal/start";
pub const METHOD_WORKFLOW_GOAL_PAUSE: &str = "workflow/goal/pause";
pub const METHOD_WORKFLOW_GOAL_FAIL: &str = "workflow/goal/fail";
pub const METHOD_WORKFLOW_GOAL_CRITERIA: &str = "workflow/goal/criteria";
pub const METHOD_WORKFLOW_GOAL_ADVANCE: &str = "workflow/goal/advance";
pub const METHOD_WORKFLOW_GOAL_RECORD_VERDICT: &str = "workflow/goal/record_verdict";
pub const METHOD_SESSION_INFO: &str = "session/info";
pub const METHOD_WORLD_STATE: &str = "world/state";
pub const METHOD_WORLD_REFRESH: &str = "world/refresh";
pub const METHOD_WORLD_SET_EXECUTION: &str = "world/set_execution";
pub const METHOD_MCP_STATUS: &str = "mcp/status";
pub const METHOD_MCP_RETRY: &str = "mcp/retry";

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
    pub profile: Option<String>,
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
    pub approval_requests: bool,
    #[serde(default)]
    pub workflows: bool,
    #[serde(default)]
    pub runtime_management: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_name: String,
    pub server_version: String,
    pub capabilities: ServerCapabilities,
    pub profile: String,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdateParams {
    pub thread_id: ThreadId,
    pub collaboration_mode: CollaborationMode,
    /// Optional replacement for the model-visible Builtin tool selection.
    /// Omission keeps the current Thread selection unchanged.
    pub builtin_tools: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettingsUpdateResult {
    pub collaboration_mode: CollaborationMode,
    pub builtin_tools: Vec<String>,
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
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoalClearedNotification {
    pub thread_id: ThreadId,
}

/// Secret-free workflow state projected by the App Server.
///
/// Filesystem paths and Host implementation types stay private to the
/// workflow service. Clients only receive whether Plan mode is active and the
/// bounded Goal state needed to render progress or resume control.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowState {
    pub collaboration_mode: CollaborationMode,
    pub goal: Option<WorkflowGoalState>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGoalStartParams {
    pub objective: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGoalAdvanceParams {
    #[serde(default)]
    pub verdict: Option<WorkflowVerifierVerdict>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGoalRecordVerdictParams {
    pub checkpoint_seq: u64,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGoalCriteriaResult {
    pub criteria: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVerifierVerdict {
    pub outcome: WorkflowVerdictOutcome,
    #[serde(default)]
    pub score: Option<u32>,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowVerdictOutcome {
    Approved,
    Rejected,
    NeedsClarification,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGoalState {
    pub schema_version: u32,
    pub goal_id: String,
    pub status: WorkflowGoalStatus,
    pub current_milestone: usize,
    pub total_milestones: usize,
    pub loop_count: usize,
    pub max_loops: usize,
    pub milestone_step_budget: usize,
    pub milestone_timeout_secs: u64,
    pub verifier_model: Option<String>,
    pub last_verifier_score: Option<u32>,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowGoalStatus {
    Running,
    Converged,
    Failed,
    UserPaused,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoResult {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub resumed: bool,
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
    pub approval: String,
    pub copilot: bool,
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
    pub profile: String,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestNotification {
    pub request_id: String,
    pub action: String,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResolvedNotification {
    pub request_id: String,
    pub action: String,
    pub approved: bool,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRespondParams {
    pub request_id: String,
    pub approved: bool,
    #[serde(default)]
    pub remember: bool,
}

/// A server-to-client notification carrying an ordered core event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEventNotification {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub sequence: u64,
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
    fn approval_notifications_preserve_turn_and_call_correlation() {
        let request = serde_json::to_value(ApprovalRequestNotification {
            request_id: "approval-1".to_string(),
            action: "shell command `pwd`".to_string(),
            thread_id: ThreadId::new("thread-1"),
            turn_id: Some(TurnId::new("turn-1")),
            call_id: Some("shell-call-1".to_string()),
        })
        .unwrap();
        assert_eq!(request["requestId"], "approval-1");
        assert_eq!(request["turnId"], "turn-1");
        assert_eq!(request["callId"], "shell-call-1");

        let resolved = serde_json::to_value(ApprovalResolvedNotification {
            request_id: "approval-1".to_string(),
            action: "shell command `pwd`".to_string(),
            approved: true,
            thread_id: ThreadId::new("thread-1"),
            turn_id: Some(TurnId::new("turn-1")),
            call_id: Some("shell-call-1".to_string()),
        })
        .unwrap();
        assert_eq!(resolved["requestId"], "approval-1");
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
            profile: None,
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
}
