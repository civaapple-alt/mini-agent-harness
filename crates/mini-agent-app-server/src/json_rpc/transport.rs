use super::*;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

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
    let mut goal_events = connection.subscribe_goal_notifications();
    let mut settings_events = connection.subscribe_settings_notifications();
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
            event = next_goal_notification(&mut goal_events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => continue,
                };
                write_json_line(&mut writer, &notification).await?;
            }
            event = next_settings_notification(&mut settings_events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => continue,
                };
                write_json_line(&mut writer, &notification).await?;
            }
            event = approval.next_event() => {
                let (method, params) = match event {
                    ApprovalEvent::Requested(request) => (
                        mini_agent_app_server_protocol::METHOD_APPROVAL_REQUEST,
                        serde_json::to_value(ApprovalRequestNotification {
                            request_id: request.request_id,
                            action: request.action,
                            thread_id: request
                                .thread_id
                                .unwrap_or_else(|| connection.server.thread_id().clone()),
                            turn_id: request.turn_id,
                            call_id: request.call_id,
                        }).expect("approval notification is serializable"),
                    ),
                    ApprovalEvent::Resolved(resolution) => (
                        mini_agent_app_server_protocol::METHOD_APPROVAL_RESOLVED,
                        serde_json::to_value(ApprovalResolvedNotification {
                            request_id: resolution.request_id,
                            action: resolution.action,
                            approved: resolution.approved,
                            thread_id: resolution
                                .thread_id
                                .unwrap_or_else(|| connection.server.thread_id().clone()),
                            turn_id: resolution.turn_id,
                            call_id: resolution.call_id,
                        }).expect("approval resolution is serializable"),
                    ),
                };
                let notification = JsonRpcRequest::notification(method, Some(params));
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

async fn next_goal_notification(
    events: &mut Option<broadcast::Receiver<super::GoalRuntimeEvent>>,
) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
    let Some(events) = events.as_mut() else {
        return std::future::pending().await;
    };
    let event = events.recv().await?;
    Ok(super::goal_notification_request(event))
}

async fn next_settings_notification(
    events: &mut Option<broadcast::Receiver<super::SettingsRuntimeEvent>>,
) -> Result<JsonRpcRequest, broadcast::error::RecvError> {
    let Some(events) = events.as_mut() else {
        return std::future::pending().await;
    };
    let event = events.recv().await?;
    Ok(super::settings_notification_request(event))
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
