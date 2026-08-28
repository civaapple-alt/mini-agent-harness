//! Experimental Agent Client Protocol (ACP) edge adapter.
//!
//! ACP uses JSON-RPC, but its session/prompt vocabulary differs from the
//! mini-agent app-server vocabulary. This crate translates the small stable
//! subset implemented by the service; it deliberately does not add ACP types
//! or dependencies to `mini-agent-core`.

use mini_agent_app_server::AppServerConnection;
use mini_agent_app_server::AppServerRuntime;
use mini_agent_app_server_protocol::CapabilityProviderSelection;
use mini_agent_app_server_protocol::InitializeParams;
use mini_agent_app_server_protocol::JsonRpcError;
use mini_agent_app_server_protocol::JsonRpcRequest;
use mini_agent_app_server_protocol::JsonRpcResponse;
use mini_agent_app_server_protocol::METHOD_INITIALIZE;
use mini_agent_app_server_protocol::METHOD_THREAD_START;
use mini_agent_app_server_protocol::METHOD_TURN_INTERRUPT;
use mini_agent_app_server_protocol::METHOD_TURN_START;
use mini_agent_app_server_protocol::PROTOCOL_VERSION;
use mini_agent_app_server_protocol::ThreadStartParams;
use mini_agent_app_server_protocol::TurnEventNotification;
use mini_agent_app_server_protocol::TurnInterruptParams;
use mini_agent_app_server_protocol::TurnStartParams;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_core::Event;
use mini_agent_core::Model;
use mini_agent_core::StopReason;
use mini_agent_core::ThreadId;
use mini_agent_core::TurnId;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnSubmission;
use mini_agent_host::RuntimeProfile;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use tokio::sync::broadcast;

pub const ACP_PROTOCOL_VERSION: u32 = 1;
pub const METHOD_SESSION_NEW: &str = "session/new";
pub const METHOD_SESSION_LIST: &str = "session/list";
pub const METHOD_SESSION_RESUME: &str = "session/resume";
pub const METHOD_SESSION_PROMPT: &str = "session/prompt";
pub const METHOD_SESSION_CANCEL: &str = "session/cancel";
pub const METHOD_SESSION_UPDATE: &str = "session/update";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeParams {
    pub protocol_version: u32,
    #[serde(default)]
    pub client_capabilities: Value,
    #[serde(default)]
    pub client_info: Option<AcpClientInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<CapabilityProviderSelection>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeResult {
    pub protocol_version: u32,
    pub agent_info: AcpAgentInfo,
    pub agent_capabilities: AcpAgentCapabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub prompt_capabilities: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewParams {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListResult {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeParams {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeResult {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PromptContent {
    Text { text: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub stop_reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCancelParams {
    pub session_id: String,
}

/// An ACP-like session bridge over the app-server service.
pub struct AcpBridge<M> {
    app: AppServerConnection<M>,
    profile: RuntimeProfile,
    session_id: Option<ThreadId>,
    active_turn: Option<TurnId>,
    pending: VecDeque<JsonRpcRequest>,
    initialized: bool,
}

impl<M> AcpBridge<M>
where
    M: Model + Send + 'static,
{
    pub fn new(app: AppServerConnection<M>) -> Self {
        Self::with_profile(app, RuntimeProfile::acp_default())
    }

    /// Creates an ACP bridge with an explicit host capability profile.
    ///
    /// ACP clients receive the selected profile and its bounded capability
    /// manifest during `initialize`; the bridge itself remains transport-only
    /// and does not compose tools or models.
    pub fn with_profile(app: AppServerConnection<M>, profile: RuntimeProfile) -> Self {
        Self {
            app,
            profile,
            session_id: None,
            active_turn: None,
            pending: VecDeque::new(),
            initialized: false,
        }
    }

    /// Handles one ACP baseline request. Unsupported ACP surfaces return a
    /// standard JSON-RPC method-not-found error instead of being implied.
    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = request.id.clone();
        if request
            .jsonrpc
            .as_deref()
            .is_some_and(|version| version != "2.0")
        {
            return Some(JsonRpcResponse::error(
                id,
                JsonRpcError::invalid_request("unsupported jsonrpc version"),
            ));
        }
        match request.method.as_str() {
            METHOD_INITIALIZE => self.initialize(request).await,
            METHOD_SESSION_NEW => self.new_session(request).await,
            METHOD_SESSION_LIST => self.list_sessions(request).await,
            METHOD_SESSION_RESUME => self.resume_session(request).await,
            METHOD_SESSION_PROMPT => self.prompt(request).await,
            METHOD_SESSION_CANCEL => self.cancel(request).await,
            _ => Some(JsonRpcResponse::error(
                id,
                JsonRpcError::method_not_found(request.method),
            )),
        }
    }

    pub async fn next_notification(
        &mut self,
    ) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
        if let Some(notification) = self.pending.pop_front() {
            return Ok(notification);
        }
        let notification = self.app.next_notification().await?;
        Ok(self.map_event(notification))
    }

    async fn initialize(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if self.initialized {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("already initialized"),
            ));
        }
        let params = match request.decode_params::<AcpInitializeParams>() {
            Ok(params) => params,
            Err(error) => return Some(JsonRpcResponse::error(request.id, error)),
        };
        if params.protocol_version != ACP_PROTOCOL_VERSION {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error(format!(
                    "unsupported ACP protocol version {}; expected {ACP_PROTOCOL_VERSION}",
                    params.protocol_version
                )),
            ));
        }
        if let Some(requested_profile) = params.profile.as_deref()
            && requested_profile != self.profile.name
        {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::invalid_params(format!(
                    "profile `{requested_profile}` is unavailable; active profile is `{}`",
                    self.profile.name
                )),
            ));
        }
        if let Some(providers) = params.providers.as_ref() {
            if let Some(provider) = providers.model.as_deref()
                && provider != self.profile.model_provider
            {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::invalid_params(format!(
                        "model provider `{provider}` is unavailable; active provider is `{}`",
                        self.profile.model_provider
                    )),
                ));
            }
            if let Some(provider) = providers.tools.as_deref()
                && provider != self.profile.tool_provider
            {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::invalid_params(format!(
                        "tool provider `{provider}` is unavailable; active provider is `{}`",
                        self.profile.tool_provider
                    )),
                ));
            }
            if let Some(provider) = providers.extensions.as_deref()
                && provider != self.profile.extension_provider
            {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::invalid_params(format!(
                        "extension provider `{provider}` is unavailable; active provider is `{}`",
                        self.profile.extension_provider
                    )),
                ));
            }
            if let Some(provider) = providers.policy.as_deref()
                && provider != self.profile.policy_provider
            {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::invalid_params(format!(
                        "policy provider `{provider}` is unavailable; active provider is `{}`",
                        self.profile.policy_provider
                    )),
                ));
            }
        }
        let app_params = InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client_name: params
                .client_info
                .as_ref()
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "acp-client".to_string()),
            client_version: params
                .client_info
                .as_ref()
                .map(|info| info.version.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            capabilities: Default::default(),
            profile: None,
            providers: params.providers,
        };
        let app_request = JsonRpcRequest::request(
            request.id.clone().unwrap_or(Value::Null),
            METHOD_INITIALIZE,
            serde_json::to_value(app_params).expect("initialize params are serializable"),
        );
        let app_response = match self.app.handle_request(app_request).await {
            Some(response) => response,
            None => {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::server_error("app-server initialization returned no response"),
                ));
            }
        };
        if let Some(error) = app_response.error {
            return Some(JsonRpcResponse::error(request.id, error));
        }
        let capability_manifest = app_response
            .result
            .as_ref()
            .filter(|result| result["profile"] == self.profile.name)
            .map(|result| result["capabilityManifest"].clone())
            .unwrap_or_else(|| {
                serde_json::to_value(self.profile.manifest())
                    .expect("ACP capability manifest is serializable")
            });
        self.initialized = true;
        let result = AcpInitializeResult {
            protocol_version: ACP_PROTOCOL_VERSION,
            agent_info: AcpAgentInfo {
                name: "mini-agent".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            agent_capabilities: AcpAgentCapabilities {
                load_session: false,
                prompt_capabilities: serde_json::json!({
                    "text": true,
                    "profile": self.profile.name.clone(),
                    "capabilities": capability_manifest
                }),
            },
        };
        Some(JsonRpcResponse::result(
            request.id,
            serde_json::to_value(result).expect("ACP initialize result is serializable"),
        ))
    }

    async fn new_session(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if !self.initialized {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("connection is not initialized"),
            ));
        }
        if let Err(error) = decode_optional_params::<SessionNewParams>(&request) {
            return Some(JsonRpcResponse::error(request.id, error));
        }
        let app_request = JsonRpcRequest::request(
            request.id.clone().unwrap_or(Value::Null),
            METHOD_THREAD_START,
            serde_json::to_value(ThreadStartParams::default())
                .expect("thread params are serializable"),
        );
        let response = self.app.handle_request(app_request).await?;
        if let Some(error) = response.error {
            return Some(JsonRpcResponse::error(request.id, error));
        }
        let thread: mini_agent_app_server_protocol::ThreadStartResult =
            match serde_json::from_value(response.result?) {
                Ok(thread) => thread,
                Err(error) => {
                    return Some(JsonRpcResponse::error(
                        request.id,
                        JsonRpcError::server_error(error.to_string()),
                    ));
                }
            };
        self.session_id = Some(thread.thread_id.clone());
        Some(JsonRpcResponse::result(
            request.id,
            serde_json::to_value(SessionNewResult {
                session_id: thread.thread_id.0,
            })
            .expect("ACP session result is serializable"),
        ))
    }

    async fn prompt(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if !self.initialized {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("connection is not initialized"),
            ));
        }
        let params = match request.decode_params::<SessionPromptParams>() {
            Ok(params) => params,
            Err(error) => return Some(JsonRpcResponse::error(request.id, error)),
        };
        let Some(session_id) = &self.session_id else {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("session/new is required before session/prompt"),
            ));
        };
        if params.session_id != session_id.as_str() {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("unknown session"),
            ));
        }
        let text = params
            .prompt
            .into_iter()
            .map(|content| match content {
                PromptContent::Text { text } => Ok(text),
            })
            .collect::<Result<Vec<_>, JsonRpcError>>()
            .map(|parts| parts.join("\n"));
        let text = match text {
            Ok(text) => text,
            Err(error) => return Some(JsonRpcResponse::error(request.id, error)),
        };
        let app_request = JsonRpcRequest::request(
            request.id.clone().unwrap_or(Value::Null),
            METHOD_TURN_START,
            serde_json::to_value(TurnStartParams {
                thread_id: session_id.clone(),
                input: TurnInput::new(TurnInputMode::Start, text),
            })
            .expect("turn params are serializable"),
        );
        let response = match self.app.handle_request(app_request).await {
            Some(response) => response,
            None => return None,
        };
        if let Some(error) = response.error {
            return Some(JsonRpcResponse::error(request.id, error));
        }
        let submission: TurnSubmission = match serde_json::from_value(response.result?) {
            Ok(submission) => submission,
            Err(error) => {
                return Some(JsonRpcResponse::error(
                    request.id,
                    JsonRpcError::server_error(error.to_string()),
                ));
            }
        };
        let TurnSubmission::Started { turn_id } = submission else {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("session/prompt was not started"),
            ));
        };
        self.active_turn = Some(turn_id.clone());
        let stop_reason = self.wait_for_turn(&turn_id).await;
        self.active_turn = None;
        Some(JsonRpcResponse::result(
            request.id,
            serde_json::to_value(SessionPromptResult { stop_reason })
                .expect("ACP prompt result is serializable"),
        ))
    }

    async fn list_sessions(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if !self.initialized {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("connection is not initialized"),
            ));
        }
        let sessions = self
            .session_id
            .as_ref()
            .map(|session_id| {
                vec![SessionInfo {
                    session_id: session_id.as_str().to_string(),
                }]
            })
            .unwrap_or_default();
        Some(JsonRpcResponse::result(
            request.id,
            serde_json::to_value(SessionListResult { sessions })
                .expect("ACP session list is serializable"),
        ))
    }

    async fn resume_session(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if !self.initialized {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("connection is not initialized"),
            ));
        }
        let params = match request.decode_params::<SessionResumeParams>() {
            Ok(params) => params,
            Err(error) => return Some(JsonRpcResponse::error(request.id, error)),
        };
        let session_id = ThreadId::new(params.session_id);
        if !self.app.has_thread(&session_id) {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("unknown session"),
            ));
        }
        self.session_id = Some(session_id.clone());
        Some(JsonRpcResponse::result(
            request.id,
            serde_json::to_value(SessionResumeResult {
                session_id: session_id.0,
            })
            .expect("ACP session resume is serializable"),
        ))
    }

    async fn cancel(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<SessionCancelParams>() {
            Ok(params) => params,
            Err(error) => return Some(JsonRpcResponse::error(request.id, error)),
        };
        let Some(session_id) = &self.session_id else {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("unknown session"),
            ));
        };
        if params.session_id != session_id.as_str() {
            return Some(JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error("unknown session"),
            ));
        }
        let Some(turn_id) = &self.active_turn else {
            return Some(JsonRpcResponse::result(request.id, Value::Null));
        };
        let app_request = JsonRpcRequest::request(
            request.id.clone().unwrap_or(Value::Null),
            METHOD_TURN_INTERRUPT,
            serde_json::to_value(TurnInterruptParams {
                thread_id: session_id.clone(),
                turn_id: turn_id.clone(),
            })
            .expect("interrupt params are serializable"),
        );
        let response = self.app.handle_request(app_request).await?;
        if let Some(error) = response.error {
            return Some(JsonRpcResponse::error(request.id, error));
        }
        Some(JsonRpcResponse::result(request.id, Value::Null))
    }

    async fn wait_for_turn(&mut self, turn_id: &TurnId) -> String {
        let mut stop_reason = "end_turn".to_string();
        loop {
            let notification = match self.app.next_notification().await {
                Ok(notification) => notification,
                Err(_) => return "error".to_string(),
            };
            let event = match notification.params.as_ref() {
                Some(params) => serde_json::from_value::<TurnEventNotification>(params.clone()),
                None => continue,
            };
            let Ok(event) = event else { continue };
            if event.turn_id.as_ref() != Some(turn_id) {
                continue;
            }
            if let Event::RunFinished {
                stop_reason: reason,
                ..
            } = event.event
            {
                stop_reason = acp_stop_reason(reason).to_string();
            }
            let done = matches!(event.event, Event::TurnFinished { .. });
            self.pending.push_back(JsonRpcRequest::notification(
                METHOD_SESSION_UPDATE,
                Some(serde_json::json!({
                    "sessionId": event.thread_id,
                    "update": { "sessionUpdate": "core_event", "event": event.event }
                })),
            ));
            if done {
                return stop_reason;
            }
        }
    }

    fn map_event(&self, notification: JsonRpcRequest) -> JsonRpcRequest {
        let Some(params) = notification.params else {
            return JsonRpcRequest::notification(METHOD_SESSION_UPDATE, None);
        };
        let event: TurnEventNotification = match serde_json::from_value(params) {
            Ok(event) => event,
            Err(_) => return JsonRpcRequest::notification(METHOD_SESSION_UPDATE, None),
        };
        JsonRpcRequest::notification(
            METHOD_SESSION_UPDATE,
            Some(serde_json::json!({
                "sessionId": event.thread_id,
                "update": { "sessionUpdate": "core_event", "event": event.event }
            })),
        )
    }
}

impl AcpBridge<OpenAiModel> {
    /// Adapts a host-built App Server runtime to ACP without creating a
    /// second harness or turn loop. Callers should build the runtime with
    /// `RuntimeProfile::acp_default()` or another allowlisted profile first.
    pub fn from_runtime(runtime: AppServerRuntime) -> Self {
        Self::new(runtime.into_connection())
    }
}

fn decode_optional_params<T>(request: &JsonRpcRequest) -> Result<T, JsonRpcError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    request
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
        .map(|params| params.unwrap_or_default())
}

fn acp_stop_reason(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Completed => "end_turn",
        StopReason::StepLimit => "max_tokens",
        StopReason::Steered => "cancelled",
        StopReason::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_app_server::AppServer;
    use mini_agent_core::Harness;
    use mini_agent_core::HarnessConfig;
    use mini_agent_core::ModelEventSink;
    use mini_agent_core::ModelRequest;
    use mini_agent_core::ModelResponse;
    use mini_agent_core::Thread;
    use mini_agent_core::ThreadStart;
    use mini_agent_core::ToolRegistry;
    use std::convert::Infallible;

    struct DoneModel;

    impl Model for DoneModel {
        type Error = Infallible;

        async fn respond<'a>(
            &'a mut self,
            _request: ModelRequest<'a>,
            _events: &'a mut (dyn ModelEventSink + Send),
        ) -> Result<ModelResponse, Self::Error> {
            Ok(ModelResponse {
                reasoning: String::new(),
                text: "done".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            })
        }
    }

    fn bridge() -> AcpBridge<DoneModel> {
        let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
        let server = AppServer::new(
            ThreadStart::new(ThreadId::new("default")),
            Thread::new(ThreadId::new("initial"), harness),
        );
        AcpBridge::new(AppServerConnection::new(server))
    }

    fn app_connection() -> AppServerConnection<DoneModel> {
        let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
        let server = AppServer::new(
            ThreadStart::new(ThreadId::new("default")),
            Thread::new(ThreadId::new("initial"), harness),
        );
        AppServerConnection::new(server)
    }

    #[tokio::test]
    async fn maps_initialize_session_and_prompt_without_core_changes() {
        let mut bridge = bridge();
        let initialize = bridge
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(AcpInitializeParams {
                    protocol_version: ACP_PROTOCOL_VERSION,
                    client_capabilities: Value::Null,
                    client_info: None,
                    profile: None,
                    providers: None,
                }),
            ))
            .await
            .unwrap();
        assert!(initialize.error.is_none());
        assert_eq!(
            initialize.result.unwrap()["agentCapabilities"]["promptCapabilities"]["profile"],
            "acp"
        );
        let session = bridge
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_SESSION_NEW,
                serde_json::json!(SessionNewParams::default()),
            ))
            .await
            .unwrap();
        let session_id = session.result.unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        let listed = bridge
            .handle_request(JsonRpcRequest::request(
                21,
                METHOD_SESSION_LIST,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(
            listed.result.unwrap()["sessions"][0]["sessionId"],
            session_id
        );
        let resumed = bridge
            .handle_request(JsonRpcRequest::request(
                22,
                METHOD_SESSION_RESUME,
                serde_json::json!(SessionResumeParams {
                    session_id: session_id.clone(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resumed.result.unwrap()["sessionId"], session_id);
        let prompt = bridge
            .handle_request(JsonRpcRequest::request(
                3,
                METHOD_SESSION_PROMPT,
                serde_json::json!(SessionPromptParams {
                    session_id,
                    prompt: vec![PromptContent::Text {
                        text: "hello".to_string()
                    }],
                }),
            ))
            .await
            .unwrap();
        assert_eq!(prompt.result.unwrap()["stopReason"], "end_turn");
        assert!(bridge.next_notification().await.is_ok());
    }

    #[tokio::test]
    async fn rejects_an_unavailable_requested_profile() {
        let mut bridge = bridge();
        let response = bridge
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(AcpInitializeParams {
                    protocol_version: ACP_PROTOCOL_VERSION,
                    client_capabilities: Value::Null,
                    client_info: None,
                    profile: Some("acp-minimal".to_string()),
                    providers: None,
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn rejects_an_unavailable_requested_provider() {
        let mut bridge = bridge();
        let response = bridge
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(AcpInitializeParams {
                    protocol_version: ACP_PROTOCOL_VERSION,
                    client_capabilities: Value::Null,
                    client_info: None,
                    profile: None,
                    providers: Some(CapabilityProviderSelection {
                        tools: Some("remote".to_string()),
                        ..CapabilityProviderSelection::default()
                    }),
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn maps_the_complete_acp_event_trace_from_the_app_server() {
        let mut bridge = bridge();
        let initialize = bridge
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(AcpInitializeParams {
                    protocol_version: ACP_PROTOCOL_VERSION,
                    client_capabilities: Value::Null,
                    client_info: None,
                    profile: None,
                    providers: None,
                }),
            ))
            .await
            .unwrap();
        assert!(initialize.error.is_none());
        let session = bridge
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_SESSION_NEW,
                serde_json::json!(SessionNewParams::default()),
            ))
            .await
            .unwrap();
        let session_id = session.result.unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        let prompt = bridge
            .handle_request(JsonRpcRequest::request(
                3,
                METHOD_SESSION_PROMPT,
                serde_json::json!(SessionPromptParams {
                    session_id,
                    prompt: vec![PromptContent::Text {
                        text: "hello".to_string()
                    }],
                }),
            ))
            .await
            .unwrap();
        assert_eq!(prompt.result.unwrap()["stopReason"], "end_turn");

        let mut acp_events = Vec::new();
        loop {
            let notification = bridge.next_notification().await.unwrap();
            let event = notification.params.unwrap()["update"]["event"].clone();
            let event: Event = serde_json::from_value(event).unwrap();
            let finished = matches!(event, Event::TurnFinished { .. });
            acp_events.push(event);
            if finished {
                break;
            }
        }

        let mut app = app_connection();
        app.handle_request(JsonRpcRequest::request(
            1,
            METHOD_INITIALIZE,
            serde_json::json!(InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                client_name: "trace-test".to_string(),
                client_version: "0".to_string(),
                capabilities: Default::default(),
                profile: None,
                providers: None,
            }),
        ))
        .await;
        app.handle_request(JsonRpcRequest::request(
            2,
            METHOD_TURN_START,
            serde_json::json!(TurnStartParams {
                thread_id: ThreadId::new("default"),
                input: TurnInput::new(TurnInputMode::Start, "hello"),
            }),
        ))
        .await;
        let mut app_events = Vec::new();
        loop {
            let notification = app.next_notification().await.unwrap();
            let params = notification.params.unwrap();
            let event: TurnEventNotification = serde_json::from_value(params).unwrap();
            let finished = matches!(event.event, Event::TurnFinished { .. });
            app_events.push(event.event);
            if finished {
                break;
            }
        }

        assert_eq!(acp_events, app_events);
    }
}
