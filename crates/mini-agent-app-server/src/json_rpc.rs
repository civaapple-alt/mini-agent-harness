//! JSON-RPC-facing connection state over the in-process AppServer backend.

use super::AppServer;
use super::AppServerError;
use super::ApprovalBroker;
use super::ApprovalEvent;
use super::ApprovalRequest;
use super::RuntimeManagementService;
use super::WorkflowService;
use crate::action::ActionFailure;
use crate::action::ActionResponse;
use mini_agent_app_server_protocol::ApprovalRequestNotification;
use mini_agent_app_server_protocol::ApprovalResolvedNotification;
use mini_agent_app_server_protocol::ApprovalRespondParams;
use mini_agent_app_server_protocol::CapabilityManifest;
use mini_agent_app_server_protocol::CollaborationMode;
use mini_agent_app_server_protocol::CollaborationModeKind;
use mini_agent_app_server_protocol::DisabledCapability;
use mini_agent_app_server_protocol::InitializeParams;
use mini_agent_app_server_protocol::InitializeResult;
use mini_agent_app_server_protocol::JsonRpcError;
use mini_agent_app_server_protocol::JsonRpcRequest;
use mini_agent_app_server_protocol::JsonRpcResponse;
use mini_agent_app_server_protocol::METHOD_APPROVAL_RESPOND;
use mini_agent_app_server_protocol::METHOD_INITIALIZE;
use mini_agent_app_server_protocol::METHOD_INITIALIZED;
use mini_agent_app_server_protocol::METHOD_MCP_RETRY;
use mini_agent_app_server_protocol::METHOD_MCP_STATUS;
use mini_agent_app_server_protocol::METHOD_SESSION_INFO;
use mini_agent_app_server_protocol::METHOD_THREAD_CLOSE;
use mini_agent_app_server_protocol::METHOD_THREAD_FORK;
use mini_agent_app_server_protocol::METHOD_THREAD_LIST;
use mini_agent_app_server_protocol::METHOD_THREAD_READ;
use mini_agent_app_server_protocol::METHOD_THREAD_RESUME;
use mini_agent_app_server_protocol::METHOD_THREAD_SETTINGS_UPDATE;
use mini_agent_app_server_protocol::METHOD_THREAD_START;
use mini_agent_app_server_protocol::METHOD_TURN_EVENT;
use mini_agent_app_server_protocol::METHOD_TURN_INTERRUPT;
use mini_agent_app_server_protocol::METHOD_TURN_READ;
use mini_agent_app_server_protocol::METHOD_TURN_START;
use mini_agent_app_server_protocol::METHOD_TURN_STEER;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_ADVANCE;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_CRITERIA;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_FAIL;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_PAUSE;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_RECORD_VERDICT;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_GOAL_START;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_STATE;
use mini_agent_app_server_protocol::METHOD_WORLD_REFRESH;
use mini_agent_app_server_protocol::METHOD_WORLD_SET_EXECUTION;
use mini_agent_app_server_protocol::METHOD_WORLD_STATE;
use mini_agent_app_server_protocol::McpRetryResult as ProtocolMcpRetryResult;
use mini_agent_app_server_protocol::McpStatusResult;
use mini_agent_app_server_protocol::PROTOCOL_VERSION;
use mini_agent_app_server_protocol::ServerCapabilities;
use mini_agent_app_server_protocol::SessionInfoResult;
use mini_agent_app_server_protocol::ThreadCloseParams;
use mini_agent_app_server_protocol::ThreadForkParams;
use mini_agent_app_server_protocol::ThreadForkResult;
use mini_agent_app_server_protocol::ThreadListParams;
use mini_agent_app_server_protocol::ThreadListResult;
use mini_agent_app_server_protocol::ThreadReadParams;
use mini_agent_app_server_protocol::ThreadReadResult;
use mini_agent_app_server_protocol::ThreadResumeParams;
use mini_agent_app_server_protocol::ThreadResumeResult;
use mini_agent_app_server_protocol::ThreadSettingsUpdateParams;
use mini_agent_app_server_protocol::ThreadSettingsUpdateResult;
use mini_agent_app_server_protocol::ThreadStartParams;
use mini_agent_app_server_protocol::ThreadStartResult;
use mini_agent_app_server_protocol::TurnEventNotification;
use mini_agent_app_server_protocol::TurnInterruptParams;
use mini_agent_app_server_protocol::TurnReadParams;
use mini_agent_app_server_protocol::TurnStartParams;
use mini_agent_app_server_protocol::TurnSteerParams;
use mini_agent_app_server_protocol::WorkflowGoalAdvanceParams;
use mini_agent_app_server_protocol::WorkflowGoalRecordVerdictParams;
use mini_agent_app_server_protocol::WorkflowGoalStartParams;
use mini_agent_app_server_protocol::WorkflowGoalState;
use mini_agent_app_server_protocol::WorkflowGoalStatus;
use mini_agent_app_server_protocol::WorkflowState;
use mini_agent_app_server_protocol::WorkflowVerdictOutcome;
use mini_agent_app_server_protocol::WorkflowVerifierVerdict;
use mini_agent_app_server_protocol::WorldRefreshResult;
use mini_agent_app_server_protocol::WorldSetExecutionParams;
use mini_agent_app_server_protocol::WorldSetExecutionResult;
use mini_agent_app_server_protocol::WorldStateResult;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_core::SessionState;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::Model;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use serde_json::Value;
use tokio::sync::broadcast;

mod thread;
mod transport;
mod turn;
mod workflow;
mod world;

pub use transport::{
    serve_stdio_with_approval_and_manifest, serve_stdio_with_startup_and_services,
};

/// Per-connection protocol state over one app-server backend.
///
/// The connection owns initialization state and an event subscription. The
/// backend remains responsible for Thread lifecycle and execution semantics.
pub struct AppServerConnection<M> {
    server: AppServer<M>,
    events: broadcast::Receiver<EventEnvelope>,
    initialized: bool,
    approval: ApprovalBroker,
    approval_enabled: bool,
    capability_manifest: CapabilityManifest,
    runtime: Option<RuntimeServices<M>>,
}

/// Host-owned services that share one runtime identity.
///
/// Keeping these services together prevents a connection from accidentally
/// combining workflow state from one runtime with management state from
/// another runtime.
#[derive(Clone)]
pub struct RuntimeServices<M> {
    management: RuntimeManagementService<M>,
    workflows: WorkflowService,
}

impl<M> RuntimeServices<M> {
    pub fn new(
        management: RuntimeManagementService<M>,
        workflows: WorkflowService,
    ) -> Result<Self, String>
    where
        M: Model + Send + 'static,
    {
        let (management, workflows) = management.bind_workflow(workflows)?;
        Ok(Self {
            management,
            workflows,
        })
    }

    fn management(&self) -> &RuntimeManagementService<M> {
        &self.management
    }

    fn workflows(&self) -> &WorkflowService {
        &self.workflows
    }
}

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub fn new(server: AppServer<M>) -> Self {
        Self::with_capability_manifest(server, default_capability_manifest())
    }

    pub fn with_capability_manifest(
        server: AppServer<M>,
        capability_manifest: CapabilityManifest,
    ) -> Self {
        Self::with_state(server, ApprovalBroker::new(), false, capability_manifest)
    }

    pub fn with_approval_broker_and_capability_manifest(
        server: AppServer<M>,
        approval: ApprovalBroker,
        capability_manifest: CapabilityManifest,
    ) -> Self {
        Self::with_state(server, approval, true, capability_manifest)
    }

    fn with_state(
        server: AppServer<M>,
        approval: ApprovalBroker,
        approval_enabled: bool,
        capability_manifest: CapabilityManifest,
    ) -> Self {
        let events = server.subscribe();
        Self {
            server,
            events,
            initialized: false,
            approval,
            approval_enabled,
            capability_manifest,
            runtime: None,
        }
    }

    /// Attaches the host-owned services for one runtime identity.
    pub fn with_runtime_services(mut self, runtime: RuntimeServices<M>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn initialized(&self) -> bool {
        self.initialized
    }

    pub async fn next_approval_request(&self) -> ApprovalRequest {
        self.approval.next_request().await
    }

    pub fn approval_response(&self, request_id: &str, approved: bool) -> Result<(), String> {
        self.approval.respond(request_id, approved)
    }

    /// Handles one JSON-RPC request. Notifications do not produce responses.
    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let notification = request.id.is_none();
        self.handle_request_inner(request)
            .await
            .filter(|_| !notification)
    }

    async fn handle_request_inner(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id.clone();
        if request
            .jsonrpc
            .as_deref()
            .is_some_and(|version| version != mini_agent_app_server_protocol::JSONRPC_VERSION)
        {
            return response_error(
                id,
                JsonRpcError::invalid_request("unsupported jsonrpc version"),
            );
        }

        if request.method == METHOD_INITIALIZE {
            return self.handle_initialize(request).await;
        }
        if request.method == METHOD_INITIALIZED {
            self.initialized = true;
            return None;
        }
        if !self.initialized {
            return response_error(
                id,
                JsonRpcError::server_error("connection is not initialized"),
            );
        }
        if request.method == METHOD_APPROVAL_RESPOND {
            return self.handle_approval_response(request).await;
        }

        match request.method.as_str() {
            METHOD_THREAD_START => self.handle_thread_start(request).await,
            METHOD_THREAD_LIST => self.handle_thread_list(request).await,
            METHOD_THREAD_FORK => self.handle_thread_fork(request).await,
            METHOD_THREAD_RESUME => self.handle_thread_resume(request).await,
            METHOD_THREAD_READ => self.handle_thread_read(request).await,
            METHOD_THREAD_CLOSE => self.handle_thread_close(request).await,
            METHOD_THREAD_SETTINGS_UPDATE => self.handle_thread_settings_update(request).await,
            METHOD_TURN_START => self.handle_turn_start(request).await,
            METHOD_TURN_READ => self.handle_turn_read(request).await,
            METHOD_TURN_STEER => self.handle_turn_steer(request).await,
            METHOD_TURN_INTERRUPT => self.handle_turn_interrupt(request).await,
            METHOD_WORKFLOW_STATE => self.handle_workflow_state(request).await,
            METHOD_WORKFLOW_GOAL_START => self.handle_workflow_goal_start(request).await,
            METHOD_WORKFLOW_GOAL_PAUSE => self.handle_workflow_goal_pause(request).await,
            METHOD_WORKFLOW_GOAL_FAIL => self.handle_workflow_goal_fail(request).await,
            METHOD_WORKFLOW_GOAL_CRITERIA => self.handle_workflow_goal_criteria(request).await,
            METHOD_WORKFLOW_GOAL_ADVANCE => self.handle_workflow_goal_advance(request).await,
            METHOD_WORKFLOW_GOAL_RECORD_VERDICT => {
                self.handle_workflow_goal_record_verdict(request).await
            }
            METHOD_SESSION_INFO => self.handle_session_info(request).await,
            METHOD_WORLD_STATE => self.handle_world_state(request).await,
            METHOD_WORLD_REFRESH => self.handle_world_refresh(request).await,
            METHOD_WORLD_SET_EXECUTION => self.handle_world_set_execution(request).await,
            METHOD_MCP_STATUS => self.handle_mcp_status(request).await,
            METHOD_MCP_RETRY => self.handle_mcp_retry(request).await,
            _ => response_error(id, JsonRpcError::method_not_found(request.method)),
        }
    }

    /// Waits for the next ordered core event and projects it as a JSON-RPC
    /// notification. A transport can serialize the returned request as JSONL.
    pub async fn next_notification(
        &mut self,
    ) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
        transport::next_event_notification(&mut self.events).await
    }

    async fn handle_initialize(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if self.initialized {
            return response_error(
                request.id,
                JsonRpcError::server_error("already initialized"),
            );
        }
        let params = match request.decode_params::<InitializeParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if params.protocol_version != PROTOCOL_VERSION {
            return response_error(
                request.id,
                JsonRpcError::server_error(format!(
                    "unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                    params.protocol_version
                )),
            );
        }
        if let Some(requested_profile) = params.profile.as_deref()
            && requested_profile != self.capability_manifest.profile
        {
            return response_error(
                request.id,
                JsonRpcError::invalid_params(format!(
                    "profile `{requested_profile}` is unavailable; active profile is `{}`",
                    self.capability_manifest.profile
                )),
            );
        }
        if let Some(providers) = params.providers.as_ref() {
            let provider_checks = [
                (
                    "model",
                    providers.model.as_deref(),
                    self.capability_manifest.model_provider.as_str(),
                ),
                (
                    "tool",
                    providers.tools.as_deref(),
                    self.capability_manifest.tool_provider.as_str(),
                ),
                (
                    "extension",
                    providers.extensions.as_deref(),
                    self.capability_manifest.extension_provider.as_str(),
                ),
                (
                    "policy",
                    providers.policy.as_deref(),
                    self.capability_manifest.policy_provider.as_str(),
                ),
            ];
            if let Some((kind, requested, active)) =
                provider_checks.into_iter().find(|(_, requested, active)| {
                    requested.is_some_and(|requested| requested != *active)
                })
            {
                return response_error(
                    request.id,
                    JsonRpcError::invalid_params(format!(
                        "{kind} provider `{}` is unavailable; active provider is `{active}`",
                        requested.expect("mismatched provider is present")
                    )),
                );
            }
        }
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            server_name: "mini-agent-app-server".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: ServerCapabilities {
                approvals: false,
                steering: true,
                thread_resume: true,
                thread_fork: self.server.supports_thread_factory(),
                thread_read: true,
                thread_close: true,
                thread_settings_update: self.runtime.is_some(),
                turn_read: true,
                thread_list: true,
                approval_requests: self.approval_enabled,
                workflows: self.runtime.is_some(),
                runtime_management: self.runtime.is_some(),
            },
            profile: self.capability_manifest.profile.clone(),
            capability_manifest: self.capability_manifest.clone(),
        };
        self.initialized = true;
        response_value(request.id, result)
    }

    async fn handle_approval_response(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ApprovalRespondParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self.approval_response(&params.request_id, params.approved) {
            Ok(()) => response_value(request.id, serde_json::json!({ "accepted": true })),
            Err(error) => response_error(request.id, JsonRpcError::invalid_params(error)),
        }
    }

    fn check_thread(&self, thread_id: &mini_agent_protocol::ThreadId) -> Result<(), JsonRpcError> {
        if self.server.has_thread(thread_id) {
            Ok(())
        } else {
            Err(JsonRpcError::server_error("unknown thread"))
        }
    }

    pub(crate) async fn thread_id(&self) -> mini_agent_protocol::ThreadId {
        match self.runtime.as_ref() {
            Some(runtime) => runtime
                .management()
                .thread_id()
                .await
                .unwrap_or_else(|_| self.server.thread_id().clone()),
            None => self.server.thread_id().clone(),
        }
    }

    pub(crate) fn runtime_management(&self) -> Result<&RuntimeManagementService<M>, String> {
        self.management_service().map_err(|error| error.message)
    }
}

/// Optional services attached to a startup-created App Server connection.
pub struct StartupServices<M> {
    pub runtime: Option<RuntimeServices<M>>,
}

impl<M> Default for StartupServices<M> {
    fn default() -> Self {
        Self { runtime: None }
    }
}

fn default_capability_manifest() -> CapabilityManifest {
    CapabilityManifest {
        profile: "unknown".to_string(),
        model_provider: "unknown".to_string(),
        tool_provider: "unknown".to_string(),
        extension_provider: "unknown".to_string(),
        policy_provider: "unknown".to_string(),
        enabled: Vec::new(),
        disabled: vec![DisabledCapability {
            name: "host-profile".to_string(),
            reason: "no host runtime manifest was supplied".to_string(),
        }],
        extension_depth: "unknown".to_string(),
        selected_extensions: Vec::new(),
        prompt_sources: Vec::new(),
        rule_sources: Vec::new(),
        rule_source_status: Vec::new(),
        prompt_source_fingerprints: Vec::new(),
        rule_source_fingerprints: Vec::new(),
        prompt_rule_precedence: Vec::new(),
        rule_resolution: "unknown".to_string(),
        rule_conflicts: Vec::new(),
        rule_policy: mini_agent_app_server_protocol::RulePolicy {
            workspace_write: false,
            shell_execution: false,
            workflow_scope: "unknown".to_string(),
        },
        context_limits: Default::default(),
        sandbox: "unknown".to_string(),
        security: "unknown".to_string(),
    }
}

fn response_value<T: serde::Serialize>(id: Option<Value>, value: T) -> Option<JsonRpcResponse> {
    Some(JsonRpcResponse::result(
        id,
        serde_json::to_value(value).expect("JSON-RPC result is serializable"),
    ))
}

fn response_error(id: Option<Value>, error: JsonRpcError) -> Option<JsonRpcResponse> {
    Some(JsonRpcResponse::error(id, error))
}

fn response_action<T: serde::Serialize>(
    id: Option<Value>,
    response: ActionResponse<T>,
) -> Option<JsonRpcResponse> {
    response_value(id, response.into_protocol())
}

fn response_action_with<T: serde::Serialize>(
    id: Option<Value>,
    response: ActionResponse<impl Sized>,
    value: T,
) -> Option<JsonRpcResponse> {
    response_value(
        id,
        mini_agent_app_server_protocol::ActionResult {
            value,
            action_id: response.metadata().action_id,
            action_sequence: response.metadata().action_sequence,
            state_revision: response.metadata().state_revision,
        },
    )
}

fn workflow_error(message: String) -> JsonRpcError {
    JsonRpcError::server_error(format!("workflow operation failed: {message}"))
}

fn map_server_error(error: AppServerError) -> JsonRpcError {
    JsonRpcError::server_error(error.to_string())
}

fn map_action_error(error: ActionFailure) -> JsonRpcError {
    let metadata = error.metadata();
    let mut mapped = map_server_error(error.error);
    if let Some(metadata) = metadata {
        mapped.data = serde_json::to_value(metadata).ok();
    }
    mapped
}

#[cfg(test)]
#[path = "json_rpc_tests.rs"]
mod tests;
