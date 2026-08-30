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
                        thread_id: connection.thread_id().await,
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
