use super::*;
use mini_agent_protocol::{ThreadId, TurnId};
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

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
    let connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server.clone(),
        approval.clone(),
        capability_manifest,
    );
    serve_connection(connection, approval, reader, writer).await
}

/// Serves stdio after startup while attaching optional runtime services to the
/// JSON-RPC connection.
pub async fn serve_stdio_with_startup_and_services<M, R, W, F>(
    approval: ApprovalBroker,
    mut reader: R,
    mut writer: W,
    startup: F,
) -> Result<(), std::io::Error>
where
    M: Model + Send + 'static,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce(
        InitializeParams,
    ) -> Result<(AppServer<M>, CapabilityManifest, StartupServices<M>), String>,
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
    let (server, capability_manifest, services) = match startup(params) {
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
    if let Some(runtime) = services.runtime {
        connection = connection.with_runtime_services(runtime);
    }
    if let Some(response) = connection.handle_request(request).await {
        write_json_line(&mut writer, &response).await?;
    }
    serve_connection(connection, approval, reader, writer).await
}

async fn serve_connection<M, R, W>(
    connection: AppServerConnection<M>,
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
    let mut runtime_events = connection.subscribe_notifications();
    let mut events = (runtime_events.is_none()).then(|| server.subscribe());
    let initialized = connection.initialized_flag();
    let connection = std::sync::Arc::new(Mutex::new(connection));
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel();
    let mut request_tasks = JoinSet::new();
    let mut line = String::new();
    loop {
        tokio::select! {
            outgoing = outgoing_rx.recv() => {
                let Some(outgoing) = outgoing else {
                    break;
                };
                match outgoing {
                    OutgoingMessage::Response(response) => {
                        write_json_line(&mut writer, &response).await?;
                    }
                    OutgoingMessage::Notification(notification) => {
                        write_json_line(&mut writer, &notification).await?;
                    }
                }
            }
            event = next_event_notification_optional(&mut events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                outgoing_tx
                    .send(OutgoingMessage::Notification(notification))
                    .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
            }
            event = next_runtime_notification(&mut runtime_events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => continue,
                };
                outgoing_tx
                    .send(OutgoingMessage::Notification(notification))
                    .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
            }
            event = approval.next_event() => {
                let (method, params) = match event {
                    ApprovalEvent::Requested(request) => {
                        publish_approval_status(
                            &server,
                            request.thread_id.clone(),
                            request.turn_id.clone(),
                            &request.request_id,
                            mini_agent_app_server_protocol::RuntimePhase::WaitingApproval,
                        );
                        (
                            mini_agent_app_server_protocol::METHOD_APPROVAL_REQUEST,
                            serde_json::to_value(ApprovalRequestNotification {
                            request_id: request.request_id,
                            project_id: request.project_id,
                            workspace_id: request.workspace_id,
                            workspace_revision: request.workspace_revision,
                            session_id: request.session_id,
                            thread_id: request.thread_id,
                            turn_id: request.turn_id,
                            call_id: request.call_id,
                            tool_name: request.tool_name,
                            action_class: request.action_class,
                            action_summary: request.action_summary,
                            action_key: request.action_key,
                            path_scope: approval_path_scope(request.access, request.target_paths),
                            access: request.access,
                            policy: request.policy,
                            allowed_grant_scopes: request.allowed_grant_scopes,
                            })
                            .expect("approval notification is serializable"),
                        )
                    }
                    ApprovalEvent::Resolved(resolution) => {
                        publish_approval_status(
                            &server,
                            resolution.thread_id.clone(),
                            resolution.turn_id.clone(),
                            &resolution.request_id,
                            mini_agent_app_server_protocol::RuntimePhase::Tool,
                        );
                        (
                            mini_agent_app_server_protocol::METHOD_APPROVAL_RESOLVED,
                            serde_json::to_value(ApprovalResolvedNotification {
                            request_id: resolution.request_id,
                            outcome: resolution.outcome,
                            grant_scope: resolution.grant_scope,
                            reason: resolution.reason,
                            project_id: resolution.project_id,
                            workspace_id: resolution.workspace_id,
                            workspace_revision: resolution.workspace_revision,
                            session_id: resolution.session_id,
                            thread_id: resolution.thread_id,
                            turn_id: resolution.turn_id,
                            call_id: resolution.call_id,
                            tool_name: resolution.tool_name,
                            action_class: resolution.action_class,
                            action_summary: resolution.action_summary,
                            })
                            .expect("approval resolution is serializable"),
                        )
                    }
                };
                let notification = JsonRpcRequest::notification(method, Some(params));
                outgoing_tx
                    .send(OutgoingMessage::Notification(notification))
                    .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
            }
            read = reader.read_line(&mut line) => {
                let read = read?;
                if read == 0 {
                    // A peer may close stdin immediately after initialize.
                    // Flush responses already queued by the inline handshake
                    // before ending the writer loop.
                    while let Ok(outgoing) = outgoing_rx.try_recv() {
                        match outgoing {
                            OutgoingMessage::Response(response) => {
                                write_json_line(&mut writer, &response).await?;
                            }
                            OutgoingMessage::Notification(notification) => {
                                write_json_line(&mut writer, &notification).await?;
                            }
                        }
                    }
                    break;
                }
                let input = std::mem::take(&mut line);
                let request = match serde_json::from_str::<JsonRpcRequest>(input.trim()) {
                    Ok(request) => request,
                    Err(error) => {
                        outgoing_tx
                            .send(OutgoingMessage::Response(response_error(
                                None,
                                JsonRpcError::parse_error(error.to_string()),
                            ).expect("parse errors always have a response")))
                            .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
                        continue;
                    }
                };

                // Initialization is deliberately ordered before any spawned
                // request. This preserves the JSON-RPC handshake even when
                // the peer closes stdin immediately after receiving its
                // initialize response.
                if request.method == METHOD_INITIALIZE
                    || !initialized.load(std::sync::atomic::Ordering::Acquire)
                {
                    let response = connection.lock().await.handle_request(request).await;
                    if let Some(response) = response {
                        outgoing_tx
                            .send(OutgoingMessage::Response(response))
                            .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
                    }
                    continue;
                }

                // Approval resolution is a control-plane fast path. A normal
                // request task may be waiting for a turn command to settle,
                // so it must not hold the connection mutex in front of the
                // response that unblocks that turn.
                if request.method == METHOD_APPROVAL_RESPOND
                    && initialized.load(std::sync::atomic::Ordering::Acquire)
                {
                    let notification = request.id.is_none();
                    if let Some(response) =
                        AppServerConnection::<M>::approval_response_fast_path(&approval, request)
                            .filter(|_| !notification)
                    {
                        outgoing_tx
                            .send(OutgoingMessage::Response(response))
                            .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))?;
                    }
                    continue;
                }

                let connection = connection.clone();
                let outgoing_tx = outgoing_tx.clone();
                request_tasks.spawn(async move {
                    let response = connection.lock().await.handle_request(request).await;
                    if let Some(response) = response {
                        let _ = outgoing_tx.send(OutgoingMessage::Response(response));
                    }
                });
            }
        }
    }
    request_tasks.abort_all();
    Ok(())
}

fn publish_approval_status<M>(
    server: &AppServer<M>,
    thread_id: Option<ThreadId>,
    turn_id: Option<TurnId>,
    request_id: &str,
    phase: mini_agent_app_server_protocol::RuntimePhase,
) where
    M: Model + Send + 'static,
{
    let status = server.runtime_status();
    let thread_id = thread_id.unwrap_or_else(|| server.thread_id().clone());
    crate::status::publish(
        &server.runtime_status_handle(),
        &server.notifications(),
        thread_id,
        phase,
        turn_id,
        Some(crate::status::operation("approval", request_id)),
        status.checkpoint_seq,
        &server.runtime_revision_handle(),
        None,
    );
}

enum OutgoingMessage {
    Response(JsonRpcResponse),
    Notification(JsonRpcRequest),
}

fn approval_path_scope(
    access: mini_agent_app_server_protocol::AccessScope,
    paths: Vec<String>,
) -> mini_agent_app_server_protocol::ApprovalPathScope {
    mini_agent_app_server_protocol::ApprovalPathScope {
        kind: if access == mini_agent_app_server_protocol::AccessScope::FullMachine {
            mini_agent_app_server_protocol::ApprovalPathKind::Machine
        } else {
            mini_agent_app_server_protocol::ApprovalPathKind::Project
        },
        paths,
    }
}

pub(super) async fn next_event_notification(
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

async fn next_event_notification_optional(
    events: &mut Option<broadcast::Receiver<EventEnvelope>>,
) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
    let Some(events) = events.as_mut() else {
        return std::future::pending().await;
    };
    next_event_notification(events).await
}

async fn next_runtime_notification(
    events: &mut Option<broadcast::Receiver<super::RuntimeNotification>>,
) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
    let Some(events) = events.as_mut() else {
        return std::future::pending().await;
    };
    let event = events.recv().await?;
    Ok(super::runtime_notification_request(event))
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
