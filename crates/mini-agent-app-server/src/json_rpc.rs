//! JSON-RPC-facing connection state over the in-process AppServer backend.

use super::AppServer;
use super::AppServerError;
use super::ApprovalBroker;
use super::ApprovalRequest;
use mini_agent_app_server_protocol::ApprovalRequestNotification;
use mini_agent_app_server_protocol::ApprovalRespondParams;
use mini_agent_app_server_protocol::CapabilityManifest;
use mini_agent_app_server_protocol::DisabledCapability;
use mini_agent_app_server_protocol::InitializeParams;
use mini_agent_app_server_protocol::InitializeResult;
use mini_agent_app_server_protocol::JsonRpcError;
use mini_agent_app_server_protocol::JsonRpcRequest;
use mini_agent_app_server_protocol::JsonRpcResponse;
use mini_agent_app_server_protocol::METHOD_APPROVAL_RESPOND;
use mini_agent_app_server_protocol::METHOD_INITIALIZE;
use mini_agent_app_server_protocol::METHOD_INITIALIZED;
use mini_agent_app_server_protocol::METHOD_THREAD_CLOSE;
use mini_agent_app_server_protocol::METHOD_THREAD_FORK;
use mini_agent_app_server_protocol::METHOD_THREAD_LIST;
use mini_agent_app_server_protocol::METHOD_THREAD_READ;
use mini_agent_app_server_protocol::METHOD_THREAD_RESUME;
use mini_agent_app_server_protocol::METHOD_THREAD_START;
use mini_agent_app_server_protocol::METHOD_TURN_EVENT;
use mini_agent_app_server_protocol::METHOD_TURN_INTERRUPT;
use mini_agent_app_server_protocol::METHOD_TURN_READ;
use mini_agent_app_server_protocol::METHOD_TURN_START;
use mini_agent_app_server_protocol::METHOD_TURN_STEER;
use mini_agent_app_server_protocol::PROTOCOL_VERSION;
use mini_agent_app_server_protocol::ServerCapabilities;
use mini_agent_app_server_protocol::ThreadCloseParams;
use mini_agent_app_server_protocol::ThreadForkParams;
use mini_agent_app_server_protocol::ThreadForkResult;
use mini_agent_app_server_protocol::ThreadListParams;
use mini_agent_app_server_protocol::ThreadListResult;
use mini_agent_app_server_protocol::ThreadReadParams;
use mini_agent_app_server_protocol::ThreadReadResult;
use mini_agent_app_server_protocol::ThreadResumeParams;
use mini_agent_app_server_protocol::ThreadResumeResult;
use mini_agent_app_server_protocol::ThreadStartParams;
use mini_agent_app_server_protocol::ThreadStartResult;
use mini_agent_app_server_protocol::TurnEventNotification;
use mini_agent_app_server_protocol::TurnInterruptParams;
use mini_agent_app_server_protocol::TurnReadParams;
use mini_agent_app_server_protocol::TurnStartParams;
use mini_agent_app_server_protocol::TurnSteerParams;
use mini_agent_core::EventEnvelope;
use mini_agent_core::Model;
use mini_agent_core::SessionState;
use mini_agent_core::TurnCancel;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStart;
use serde_json::Value;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

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
}

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub fn new(server: AppServer<M>) -> Self {
        Self::with_capability_manifest(server, default_capability_manifest())
    }

    pub fn with_approval_broker(server: AppServer<M>, approval: ApprovalBroker) -> Self {
        Self::with_approval_broker_and_capability_manifest(
            server,
            approval,
            default_capability_manifest(),
        )
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
        }
    }

    pub fn initialized(&self) -> bool {
        self.initialized
    }

    pub fn has_thread(&self, thread_id: &mini_agent_core::ThreadId) -> bool {
        self.server.has_thread(thread_id)
    }

    pub fn approval_broker(&self) -> ApprovalBroker {
        self.approval.clone()
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
            METHOD_TURN_START => self.handle_turn_start(request).await,
            METHOD_TURN_READ => self.handle_turn_read(request).await,
            METHOD_TURN_STEER => self.handle_turn_steer(request).await,
            METHOD_TURN_INTERRUPT => self.handle_turn_interrupt(request).await,
            _ => response_error(id, JsonRpcError::method_not_found(request.method)),
        }
    }

    /// Waits for the next ordered core event and projects it as a JSON-RPC
    /// notification. A transport can serialize the returned request as JSONL.
    pub async fn next_notification(
        &mut self,
    ) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
        next_event_notification(&mut self.events).await
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
                turn_read: true,
                thread_list: true,
                approval_requests: self.approval_enabled,
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

    async fn handle_thread_start(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadStartParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let thread_id = params
            .thread_id
            .unwrap_or_else(|| self.server.thread_id().clone());
        if !self.server.has_thread(&thread_id) {
            return match self.server.thread_start(thread_id.clone()).await {
                Ok(thread_id) => response_value(request.id, ThreadStartResult { thread_id }),
                Err(error) => response_error(request.id, map_server_error(error)),
            };
        }
        response_value(request.id, ThreadStartResult { thread_id })
    }

    async fn handle_thread_list(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.params {
            Some(params) => match serde_json::from_value::<ThreadListParams>(params) {
                Ok(params) => params,
                Err(error) => {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(error.to_string()),
                    );
                }
            },
            None => ThreadListParams::default(),
        };
        let start = params
            .cursor
            .as_deref()
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = params
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(usize::MAX);
        let ids = self.server.thread_ids();
        let data = ids
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor =
            (start + data.len() < ids.len()).then(|| (start + data.len()).to_string());
        response_value(request.id, ThreadListResult { data, next_cursor })
    }

    async fn handle_thread_fork(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadForkParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self
            .server
            .thread_fork(params.source_thread_id, params.new_thread_id)
            .await
        {
            Ok(thread_id) => response_value(request.id, ThreadForkResult { thread_id }),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_thread_resume(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadResumeParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let checkpoint = params.checkpoint;
        let core_checkpoint = mini_agent_core::ThreadCheckpoint {
            thread_id: params.thread_id.clone(),
            session: SessionState::from_messages(checkpoint.messages)
                .with_context_revision(checkpoint.context_revision),
            status: checkpoint.status,
            next_turn_number: checkpoint.next_turn_number,
            last_turn_id: checkpoint.last_turn_id,
            next_event_sequence: checkpoint.next_event_sequence,
        };
        match self
            .server
            .thread_resume(params.thread_id, core_checkpoint)
            .await
        {
            Ok(thread_id) => response_value(request.id, ThreadResumeResult { thread_id }),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_turn_start(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnStartParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if !matches!(
            params.input.mode,
            TurnInputMode::Start | TurnInputMode::StartIfIdle
        ) {
            return response_error(
                request.id,
                JsonRpcError::invalid_params("turn/start requires start or start_if_idle"),
            );
        }
        match self
            .server
            .turn_start_for(params.thread_id, TurnStart::new(params.input))
            .await
        {
            Ok(submission) => response_value(request.id, submission),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_thread_read(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadReadParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        match self.server.thread_read_for(params.thread_id).await {
            Ok(checkpoint) => response_value(
                request.id,
                ThreadReadResult {
                    thread_id: checkpoint.thread_id,
                    status: checkpoint.status,
                    messages: checkpoint.session.messages().to_vec(),
                    context_revision: checkpoint.session.context_revision(),
                    next_turn_number: checkpoint.next_turn_number,
                    last_turn_id: checkpoint.last_turn_id,
                    next_event_sequence: checkpoint.next_event_sequence,
                },
            ),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_thread_close(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadCloseParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        match self.server.thread_close_for(params.thread_id).await {
            Ok(()) => response_value(request.id, serde_json::json!({ "closed": true })),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_turn_read(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnReadParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self.server.turn_read(params.turn_id.clone()).await {
            Ok(result) => response_value(
                request.id,
                mini_agent_app_server_protocol::TurnReadResult {
                    turn_id: result.id,
                    status: result.status,
                    stop_reason: result.outcome.as_ref().map(|outcome| outcome.stop_reason),
                    final_text: result
                        .outcome
                        .as_ref()
                        .map(|outcome| outcome.final_text.clone()),
                    steps: result.outcome.as_ref().map_or(0, |outcome| outcome.steps),
                    messages: result
                        .outcome
                        .as_ref()
                        .map_or_else(Vec::new, |outcome| outcome.messages.clone()),
                    error: result.error,
                },
            ),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_turn_steer(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnSteerParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self
            .server
            .turn_steer_for(params.thread_id, params.turn_id, params.text)
            .await
        {
            Ok(submission) => response_value(request.id, submission),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    async fn handle_turn_interrupt(&self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnInterruptParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self
            .server
            .turn_cancel_for(params.thread_id, TurnCancel::new(params.turn_id))
            .await
        {
            Ok(()) => response_value(request.id, serde_json::json!({ "accepted": true })),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    fn check_thread(&self, thread_id: &mini_agent_core::ThreadId) -> Result<(), JsonRpcError> {
        if self.server.has_thread(thread_id) {
            Ok(())
        } else {
            Err(JsonRpcError::server_error("unknown thread"))
        }
    }
}

/// Serves newline-delimited JSON-RPC over an async reader and writer.
///
/// Requests and event notifications share one ordered output stream. The
/// supplied AppServer remains the execution backend; this function only owns
/// framing, decoding, and connection-level protocol state.
pub async fn serve_stdio<M, R, W>(
    server: AppServer<M>,
    reader: R,
    writer: W,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve_stdio_with_approval_and_manifest(
        server,
        ApprovalBroker::new(),
        default_capability_manifest(),
        reader,
        writer,
    )
    .await
}

/// Serves stdio while forwarding host approval callbacks to the client.
pub async fn serve_stdio_with_approval<M, R, W>(
    server: AppServer<M>,
    approval: ApprovalBroker,
    reader: R,
    writer: W,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve_stdio_with_approval_and_manifest(
        server,
        approval,
        default_capability_manifest(),
        reader,
        writer,
    )
    .await
}

/// Serves stdio with a host-resolved capability manifest.
pub async fn serve_stdio_with_approval_and_manifest<M, R, W>(
    server: AppServer<M>,
    approval: ApprovalBroker,
    capability_manifest: CapabilityManifest,
    reader: R,
    writer: W,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server.clone(),
        approval.clone(),
        capability_manifest,
    );
    serve_connection(&mut connection, approval, reader, writer).await
}

/// Serves stdio after selecting the host profile from the first initialize
/// request. The callback runs before a Thread or App Server is constructed,
/// so profile selection is a startup decision and remains frozen for the
/// service lifetime.
pub async fn serve_stdio_with_startup<M, R, W, F>(
    approval: ApprovalBroker,
    mut reader: R,
    mut writer: W,
    startup: F,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce(InitializeParams) -> Result<(AppServer<M>, CapabilityManifest), String>,
{
    let mut line = String::new();
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Ok(());
    }
    let request = match serde_json::from_str::<JsonRpcRequest>(line.trim()) {
        Ok(request) => request,
        Err(error) => {
            write_json_line(
                &mut writer,
                &JsonRpcResponse::error(None, JsonRpcError::parse_error(error.to_string())),
            )
            .await?;
            return Ok(());
        }
    };
    if request.method != METHOD_INITIALIZE {
        write_json_line(
            &mut writer,
            &JsonRpcResponse::error(
                request.id.clone(),
                JsonRpcError::invalid_request("first request must be initialize"),
            ),
        )
        .await?;
        return Ok(());
    }
    let params = match request.decode_params::<InitializeParams>() {
        Ok(params) => params,
        Err(error) => {
            write_json_line(&mut writer, &JsonRpcResponse::error(request.id, error)).await?;
            return Ok(());
        }
    };
    if params.protocol_version != PROTOCOL_VERSION {
        write_json_line(
            &mut writer,
            &JsonRpcResponse::error(
                request.id,
                JsonRpcError::server_error(format!(
                    "unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                    params.protocol_version
                )),
            ),
        )
        .await?;
        return Ok(());
    }
    let (server, capability_manifest) = match startup(params) {
        Ok(started) => started,
        Err(error) => {
            write_json_line(
                &mut writer,
                &JsonRpcResponse::error(request.id, JsonRpcError::server_error(error)),
            )
            .await?;
            return Ok(());
        }
    };
    let mut connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server,
        approval.clone(),
        capability_manifest,
    );
    if let Some(response) = connection.handle_request(request).await {
        write_json_line(&mut writer, &response).await?;
    }
    serve_connection(&mut connection, approval, reader, writer).await
}

async fn serve_connection<M, R, W>(
    connection: &mut AppServerConnection<M>,
    approval: ApprovalBroker,
    mut reader: R,
    mut writer: W,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let server = connection.server.clone();
    let mut events = server.subscribe();
    let mut line = String::new();
    loop {
        tokio::select! {
            event = next_event_notification(&mut events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                write_json_line(&mut writer, &notification).await?;
            }
            request = approval.next_request() => {
                let notification = JsonRpcRequest::notification(
                    mini_agent_app_server_protocol::METHOD_APPROVAL_REQUEST,
                    Some(serde_json::to_value(ApprovalRequestNotification {
                        request_id: request.request_id,
                        action: request.action,
                        thread_id: connection.server.thread_id().clone(),
                        turn_id: None,
                    }).expect("approval notification is serializable")),
                );
                write_json_line(&mut writer, &notification).await?;
            }
            read = reader.read_line(&mut line) => {
                let read = read?;
                if read == 0 {
                    break;
                }
                let input = std::mem::take(&mut line);
                let response = match serde_json::from_str::<JsonRpcRequest>(input.trim()) {
                    Ok(request) => connection.handle_request(request).await,
                    Err(error) => response_error(None, JsonRpcError::parse_error(error.to_string())),
                };
                if let Some(response) = response {
                    write_json_line(&mut writer, &response).await?;
                }
            }
        }
    }
    Ok(())
}

async fn next_event_notification(
    events: &mut broadcast::Receiver<EventEnvelope>,
) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
    let event = events.recv().await?;
    let params = serde_json::to_value(TurnEventNotification::from(event))
        .expect("event notification is serializable");
    Ok(JsonRpcRequest::notification(
        METHOD_TURN_EVENT,
        Some(params),
    ))
}

fn default_capability_manifest() -> CapabilityManifest {
    CapabilityManifest {
        profile: "unknown".to_string(),
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
            process_execution: false,
            workflow_scope: "unknown".to_string(),
        },
        context_limits: Default::default(),
        sandbox: "unknown".to_string(),
        security: "unknown".to_string(),
    }
}

async fn write_json_line<W: AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), std::io::Error> {
    let encoded =
        serde_json::to_vec(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    writer.write_all(&encoded).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
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

fn map_server_error(error: AppServerError) -> JsonRpcError {
    JsonRpcError::server_error(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_app_server_protocol::ClientCapabilities;
    use mini_agent_core::Harness;
    use mini_agent_core::HarnessConfig;
    use mini_agent_core::ModelEventSink;
    use mini_agent_core::ModelRequest;
    use mini_agent_core::ModelResponse;
    use mini_agent_core::Thread;
    use mini_agent_core::ThreadId;
    use mini_agent_core::ThreadStart;
    use mini_agent_core::ToolRegistry;
    use mini_agent_core::TurnInput;
    use std::convert::Infallible;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;

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

    fn connection() -> AppServerConnection<DoneModel> {
        let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
        let server = AppServer::new(
            ThreadStart::new(ThreadId::new("thread-1")),
            Thread::new(ThreadId::new("initial"), harness),
        );
        AppServerConnection::new(server)
    }

    #[tokio::test]
    async fn requires_initialize_and_handles_turn_start() {
        let mut connection = connection();
        let request = JsonRpcRequest::request(1, METHOD_TURN_START, serde_json::json!({}));
        let response = connection.handle_request(request).await.unwrap();
        assert_eq!(response.error.unwrap().code, -32000);

        let response = connection
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await
            .unwrap();
        assert!(response.error.is_none());
        assert_eq!(response.result.as_ref().unwrap()["profile"], "unknown");
        assert_eq!(
            response.result.as_ref().unwrap()["capabilityManifest"]["profile"],
            "unknown"
        );
        assert_eq!(
            response.result.as_ref().unwrap()["capabilityManifest"]["rulePolicy"]["workspaceWrite"],
            false
        );
        assert_eq!(
            response.result.as_ref().unwrap()["capabilityManifest"]["ruleSourceStatus"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert!(connection.initialized());

        let response = connection
            .handle_request(JsonRpcRequest::request(
                3,
                METHOD_TURN_START,
                serde_json::json!(TurnStartParams {
                    thread_id: ThreadId::new("thread-1"),
                    input: TurnInput::new(TurnInputMode::Start, "hello"),
                }),
            ))
            .await
            .unwrap();
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap()["status"], "started");
    }

    #[tokio::test]
    async fn rejects_an_unavailable_requested_profile() {
        let mut connection = connection();
        let response = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: Some("acp".to_string()),
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.error.unwrap().code, -32602);
        assert!(!connection.initialized());
    }

    #[tokio::test]
    async fn selects_profile_before_starting_stdio_service() {
        let request = JsonRpcRequest::request(
            1,
            METHOD_INITIALIZE,
            serde_json::json!(InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                client_name: "startup-profile-test".to_string(),
                client_version: "0".to_string(),
                capabilities: ClientCapabilities::default(),
                profile: Some("acp".to_string()),
            }),
        );
        let input = format!("{}\n", serde_json::to_string(&request).unwrap());
        let mut output = Vec::new();
        let broker = ApprovalBroker::new();
        serve_stdio_with_startup(
            broker,
            tokio::io::BufReader::new(std::io::Cursor::new(input.into_bytes())),
            &mut output,
            |params| {
                assert_eq!(params.profile.as_deref(), Some("acp"));
                let mut manifest = default_capability_manifest();
                manifest.profile = "acp".to_string();
                Ok((connection().server, manifest))
            },
        )
        .await
        .unwrap();
        let response: JsonRpcResponse = serde_json::from_slice(output.trim_ascii()).unwrap();
        assert_eq!(response.result.unwrap()["profile"], "acp");
    }

    #[tokio::test]
    async fn projects_core_events_as_notifications() {
        let mut connection = connection();
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await;
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_TURN_START,
                serde_json::json!(TurnStartParams {
                    thread_id: ThreadId::new("thread-1"),
                    input: TurnInput::new(TurnInputMode::Start, "hello"),
                }),
            ))
            .await;
        let notification = connection.next_notification().await.unwrap();
        assert_eq!(notification.method, METHOD_TURN_EVENT);
        assert!(notification.id.is_none());
    }

    #[tokio::test]
    async fn serves_initialize_over_jsonl_stdio() {
        let (mut input, server_input) = tokio::io::duplex(4096);
        let (server_output, client_output) = tokio::io::duplex(4096);
        let task = tokio::spawn(serve_stdio(
            connection().server.clone(),
            tokio::io::BufReader::new(server_input),
            server_output,
        ));
        let request = JsonRpcRequest::request(
            1,
            METHOD_INITIALIZE,
            serde_json::json!(InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                client_name: "jsonl-test".to_string(),
                client_version: "0".to_string(),
                capabilities: ClientCapabilities::default(),
                profile: None,
            }),
        );
        input
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        input.shutdown().await.unwrap();

        let mut output = tokio::io::BufReader::new(client_output);
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        let response: JsonRpcResponse = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(response.id, Some(serde_json::json!(1)));
        assert_eq!(
            response.result.unwrap()["protocolVersion"],
            PROTOCOL_VERSION
        );
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn local_client_uses_the_same_service_contract() {
        let connection = connection();
        let server = connection.server.clone();
        let mut client = crate::LocalAppServerClient::new(AppServerConnection::new(server));
        let initialized = client.initialize("local-test", "0").await.unwrap();
        assert_eq!(initialized.protocol_version, PROTOCOL_VERSION);
        assert_eq!(
            client.list_threads().await.unwrap().data,
            vec![ThreadId::new("thread-1")]
        );
        let thread = client.start_thread().await.unwrap();
        let submission = client
            .start_turn(
                thread.thread_id.clone(),
                TurnInput::new(TurnInputMode::Start, "hello"),
            )
            .await
            .unwrap();
        assert!(matches!(
            submission,
            mini_agent_core::TurnSubmission::Started { .. }
        ));
        assert_eq!(client.next_event().await.unwrap().sequence, 1);
    }

    #[tokio::test]
    async fn local_and_json_rpc_clients_preserve_the_same_event_trace() {
        let mut local =
            crate::LocalAppServerClient::new(AppServerConnection::new(connection().server.clone()));
        local.initialize("local-trace", "0").await.unwrap();
        local
            .start_turn(
                ThreadId::new("thread-1"),
                TurnInput::new(TurnInputMode::Start, "hello"),
            )
            .await
            .unwrap();
        let mut local_events = Vec::new();
        loop {
            let event = local.next_event().await.unwrap();
            let finished = matches!(event.event, mini_agent_core::Event::TurnFinished { .. });
            local_events.push(event);
            if finished {
                break;
            }
        }

        let mut json_rpc = connection();
        json_rpc
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::to_value(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "json-rpc-trace".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        json_rpc
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_TURN_START,
                serde_json::to_value(TurnStartParams {
                    thread_id: ThreadId::new("thread-1"),
                    input: TurnInput::new(TurnInputMode::Start, "hello"),
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        let mut json_events = Vec::new();
        loop {
            let notification = json_rpc.next_notification().await.unwrap();
            let params = notification.params.unwrap();
            let event: TurnEventNotification = serde_json::from_value(params).unwrap();
            let envelope =
                EventEnvelope::new(event.thread_id, event.turn_id, event.sequence, event.event);
            let finished = matches!(envelope.event, mini_agent_core::Event::TurnFinished { .. });
            json_events.push(envelope);
            if finished {
                break;
            }
        }

        assert_eq!(local_events, json_events);
    }

    #[tokio::test]
    async fn exposes_settled_turn_and_thread_checkpoint_over_json_rpc() {
        let mut connection = connection();
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "checkpoint-test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await;
        let started = connection
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_TURN_START,
                serde_json::json!(TurnStartParams {
                    thread_id: ThreadId::new("thread-1"),
                    input: TurnInput::new(TurnInputMode::Start, "hello"),
                }),
            ))
            .await
            .unwrap();
        let turn_id: mini_agent_core::TurnId =
            serde_json::from_value(started.result.unwrap()["turn_id"].clone()).unwrap();
        for _ in 0..6 {
            let _ = connection.next_notification().await.unwrap();
        }
        let turn = connection
            .handle_request(JsonRpcRequest::request(
                3,
                METHOD_TURN_READ,
                serde_json::json!(TurnReadParams {
                    turn_id: turn_id.clone(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(turn.result.unwrap()["finalText"], "done");

        let thread = connection
            .handle_request(JsonRpcRequest::request(
                4,
                METHOD_THREAD_READ,
                serde_json::json!(ThreadReadParams {
                    thread_id: ThreadId::new("thread-1"),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(thread.result.unwrap()["status"], "idle");
    }

    #[tokio::test]
    async fn forwards_approval_response_through_json_rpc_connection() {
        let server = connection().server.clone();
        let broker = ApprovalBroker::new();
        let requester = broker.clone();
        let mut connection = AppServerConnection::with_approval_broker(server, broker.clone());
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "approval-test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await;
        let task = tokio::task::spawn_blocking(move || requester.request("edit file"));
        let pending = connection.next_approval_request().await;
        let response = connection
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_APPROVAL_RESPOND,
                serde_json::json!(ApprovalRespondParams {
                    request_id: pending.request_id,
                    approved: true,
                    remember: false,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.result.unwrap()["accepted"], true);
        assert!(task.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn exposes_factory_backed_thread_lifecycle_methods() {
        let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
        let server = AppServer::with_thread_factory(
            ThreadStart::new(ThreadId::new("thread-1")),
            vec![Thread::new(ThreadId::new("initial"), harness)],
            |id| {
                Ok(Thread::new(
                    id,
                    Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default()),
                ))
            },
        );
        let mut connection = AppServerConnection::new(server);
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "lifecycle-test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await;
        let created = connection
            .handle_request(JsonRpcRequest::request(
                2,
                METHOD_THREAD_START,
                serde_json::json!(ThreadStartParams {
                    thread_id: Some(ThreadId::new("thread-2")),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(created.result.unwrap()["threadId"], "thread-2");
        let forked = connection
            .handle_request(JsonRpcRequest::request(
                3,
                METHOD_THREAD_FORK,
                serde_json::json!(ThreadForkParams {
                    source_thread_id: ThreadId::new("thread-1"),
                    new_thread_id: ThreadId::new("thread-3"),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(forked.result.unwrap()["threadId"], "thread-3");
        let listed = connection
            .handle_request(JsonRpcRequest::request(
                4,
                METHOD_THREAD_LIST,
                serde_json::json!(ThreadListParams::default()),
            ))
            .await
            .unwrap();
        assert_eq!(listed.result.unwrap()["data"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn suppresses_responses_for_json_rpc_notifications() {
        let mut connection = connection();
        let _ = connection
            .handle_request(JsonRpcRequest::request(
                1,
                METHOD_INITIALIZE,
                serde_json::json!(InitializeParams {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "notification-test".to_string(),
                    client_version: "0".to_string(),
                    capabilities: ClientCapabilities::default(),
                    profile: None,
                }),
            ))
            .await;
        assert!(
            connection
                .handle_request(JsonRpcRequest::notification(
                    METHOD_THREAD_LIST,
                    Some(serde_json::to_value(ThreadListParams::default()).unwrap()),
                ))
                .await
                .is_none()
        );
    }
}
