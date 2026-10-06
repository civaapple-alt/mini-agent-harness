use super::*;
use mini_agent_protocol::{ThreadId, TurnId};
use std::sync::OnceLock;
use std::time::Instant;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinSet;

const MAX_JSON_RPC_LINE_BYTES: usize = 2 * 1024 * 1024;

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
) -> Result<usize, std::io::Error> {
    let read = reader
        .take(MAX_JSON_RPC_LINE_BYTES as u64 + 1)
        .read_line(line)
        .await?;
    if read > MAX_JSON_RPC_LINE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "JSON-RPC line exceeds the byte limit",
        ));
    }
    Ok(read)
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
    let user_questions = UserQuestionBroker::new();
    let connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server.clone(),
        approval.clone(),
        capability_manifest,
    )
    .with_user_questions(user_questions.clone(), false);
    serve_connection(connection, approval, user_questions, false, reader, writer).await
}

/// Serves stdio after startup while attaching optional runtime services to the
/// JSON-RPC connection.
pub async fn serve_stdio_with_startup_and_services<M, R, W, F>(
    approval: ApprovalBroker,
    reader: R,
    writer: W,
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
    serve_stdio_with_startup_and_user_questions(
        approval,
        UserQuestionBroker::new(),
        reader,
        writer,
        startup,
    )
    .await
}

/// Serves stdio with a shared interactive user-question broker.
pub async fn serve_stdio_with_startup_and_user_questions<M, R, W, F>(
    approval: ApprovalBroker,
    user_questions: UserQuestionBroker,
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
    let read = read_bounded_line(&mut reader, &mut line).await?;
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
    let user_questions_enabled = params.capabilities.user_questions;
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
    )
    .with_user_questions(user_questions.clone(), user_questions_enabled);
    if let Some(runtime) = services.runtime {
        connection = connection.with_runtime_services(runtime);
    }
    if let Some(response) = connection.handle_request(request).await {
        write_json_line(&mut writer, &response).await?;
    }
    serve_connection(
        connection,
        approval,
        user_questions,
        user_questions_enabled,
        reader,
        writer,
    )
    .await
}

async fn serve_connection<M, R, W>(
    connection: AppServerConnection<M>,
    approval: ApprovalBroker,
    user_questions: UserQuestionBroker,
    user_questions_enabled: bool,
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
    let mut question_events = user_questions.subscribe();
    let connection = std::sync::Arc::new(Mutex::new(connection));
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<QueuedMessage>();
    let mut request_tasks = JoinSet::new();
    let mut line = String::new();
    loop {
        tokio::select! {
            outgoing = outgoing_rx.recv() => {
                let Some(outgoing) = outgoing else {
                    break;
                };
                write_queued_message(&mut writer, outgoing).await?;
            }
            event = next_event_notification_optional(&mut events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                enqueue_notification(&outgoing_tx, notification)?;
            }
            event = next_runtime_notification(&mut runtime_events) => {
                let notification = match event {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => continue,
                };
                enqueue_notification(&outgoing_tx, notification)?;
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
                enqueue_notification(&outgoing_tx, notification)?;
            }
            event = user_questions.next_event(&mut question_events), if user_questions_enabled => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => continue,
                };
                publish_user_question_status(&server, &event.interaction, event.phase);
                let method = if event.phase == mini_agent_app_server_protocol::UserQuestionPhase::Requested {
                    mini_agent_app_server_protocol::METHOD_USER_QUESTION_REQUEST
                } else {
                    mini_agent_app_server_protocol::METHOD_USER_QUESTION_UPDATED
                };
                let notification = JsonRpcRequest::notification(
                    method,
                    Some(serde_json::to_value(mini_agent_app_server_protocol::UserQuestionNotification {
                        phase: event.phase,
                        interaction: event.interaction,
                    }).expect("user question notification is serializable")),
                );
                enqueue_notification(&outgoing_tx, notification)?;
            }
            read = async {
                let started = Instant::now();
                let result = read_bounded_line(&mut reader, &mut line).await;
                (result, started.elapsed().as_micros())
            } => {
                let (read, read_us) = read;
                let read = read?;
                if read == 0 {
                    // A peer may close stdin immediately after initialize.
                    // Flush responses already queued by the inline handshake
                    // before ending the writer loop.
                    while let Ok(outgoing) = outgoing_rx.try_recv() {
                        write_queued_message(&mut writer, outgoing).await?;
                    }
                    break;
                }
                let input = std::mem::take(&mut line);
                let parse_started = Instant::now();
                let parsed_request = serde_json::from_str::<JsonRpcRequest>(input.trim());
                let parse_us = parse_started.elapsed().as_micros();
                let mut timings = RequestStageTimes {
                    read_us,
                    parse_us,
                    ..RequestStageTimes::default()
                };
                let request = match parsed_request {
                    Ok(request) => request,
                    Err(error) => {
                        enqueue_response(
                            &outgoing_tx,
                            response_error(
                                None,
                                JsonRpcError::parse_error(error.to_string()),
                            )
                            .expect("parse errors always have a response"),
                            "parse_error".to_string(),
                            timings,
                        )?;
                        continue;
                    }
                };
                let request_method = request.method.clone();

                // Initialization is deliberately ordered before any spawned
                // request. This preserves the JSON-RPC handshake even when
                // the peer closes stdin immediately after receiving its
                // initialize response.
                if request.method == METHOD_INITIALIZE
                    || !initialized.load(std::sync::atomic::Ordering::Acquire)
                {
                    let dispatch_started = Instant::now();
                    let response = connection.lock().await.handle_request(request).await;
                    timings.dispatch_us = dispatch_started.elapsed().as_micros();
                    if let Some(response) = response {
                        enqueue_response(&outgoing_tx, response, request_method, timings)?;
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
                    let dispatch_started = Instant::now();
                    if let Some(response) =
                        AppServerConnection::<M>::approval_response_fast_path(&approval, request)
                            .filter(|_| !notification)
                    {
                        timings.dispatch_us = dispatch_started.elapsed().as_micros();
                        enqueue_response(&outgoing_tx, response, request_method, timings)?;
                    }
                    continue;
                }

                if request.method == mini_agent_app_server_protocol::METHOD_USER_QUESTION_RESPOND
                    && initialized.load(std::sync::atomic::Ordering::Acquire)
                    && user_questions_enabled
                {
                    let notification = request.id.is_none();
                    let dispatch_started = Instant::now();
                    if let Some(response) = AppServerConnection::<M>::user_question_response_fast_path(&user_questions, request)
                        .filter(|_| !notification)
                    {
                        timings.dispatch_us = dispatch_started.elapsed().as_micros();
                        enqueue_response(&outgoing_tx, response, request_method, timings)?;
                    }
                    continue;
                }

                let connection = connection.clone();
                let outgoing_tx = outgoing_tx.clone();
                request_tasks.spawn(async move {
                    let dispatch_started = Instant::now();
                    let response = connection.lock().await.handle_request(request).await;
                    timings.dispatch_us = dispatch_started.elapsed().as_micros();
                    if let Some(response) = response {
                        let _ = enqueue_response(
                            &outgoing_tx,
                            response,
                            request_method,
                            timings,
                        );
                    }
                });
            }
        }
    }
    request_tasks.abort_all();
    Ok(())
}

fn publish_user_question_status<M>(
    server: &AppServer<M>,
    interaction: &mini_agent_protocol::UserQuestionInteraction,
    phase: mini_agent_app_server_protocol::UserQuestionPhase,
) where
    M: Model + Send + 'static,
{
    let status = server.runtime_status();
    let next_phase = match phase {
        mini_agent_app_server_protocol::UserQuestionPhase::Requested
        | mini_agent_app_server_protocol::UserQuestionPhase::Updated => {
            mini_agent_app_server_protocol::RuntimePhase::WaitingForUserInput
        }
        mini_agent_app_server_protocol::UserQuestionPhase::Resolved
        | mini_agent_app_server_protocol::UserQuestionPhase::Cancelled => {
            mini_agent_app_server_protocol::RuntimePhase::Tool
        }
    };
    crate::status::publish(
        &server.runtime_status_handle(),
        &server.notifications(),
        interaction.thread_id.clone(),
        next_phase,
        Some(interaction.turn_id.clone()),
        Some(crate::status::operation(
            "user-question",
            &interaction.interaction_id,
        )),
        status.checkpoint_seq,
        &server.runtime_revision_handle(),
        None,
    );
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

#[derive(Clone, Copy, Default)]
struct RequestStageTimes {
    read_us: u128,
    parse_us: u128,
    dispatch_us: u128,
}

struct QueuedMessage {
    message: OutgoingMessage,
    method: String,
    queued_at: Instant,
    request_times: RequestStageTimes,
}

fn enqueue_outgoing(
    sender: &mpsc::UnboundedSender<QueuedMessage>,
    message: OutgoingMessage,
    method: String,
    request_times: RequestStageTimes,
) -> Result<(), std::io::Error> {
    sender
        .send(QueuedMessage {
            message,
            method,
            queued_at: Instant::now(),
            request_times,
        })
        .map_err(|_| std::io::Error::other("JSON-RPC writer stopped"))
}

fn enqueue_response(
    sender: &mpsc::UnboundedSender<QueuedMessage>,
    response: JsonRpcResponse,
    method: String,
    request_times: RequestStageTimes,
) -> Result<(), std::io::Error> {
    enqueue_outgoing(
        sender,
        OutgoingMessage::Response(response),
        method,
        request_times,
    )
}

fn enqueue_notification(
    sender: &mpsc::UnboundedSender<QueuedMessage>,
    notification: JsonRpcRequest,
) -> Result<(), std::io::Error> {
    let method = notification.method.clone();
    enqueue_outgoing(
        sender,
        OutgoingMessage::Notification(notification),
        method,
        RequestStageTimes::default(),
    )
}

async fn write_queued_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    queued: QueuedMessage,
) -> Result<(), std::io::Error> {
    let queue_us = queued.queued_at.elapsed().as_micros();
    let (serialized_us, write_us, bytes) = match queued.message {
        OutgoingMessage::Response(response) => write_json_line_timed(writer, &response).await?,
        OutgoingMessage::Notification(notification) => {
            write_json_line_timed(writer, &notification).await?
        }
    };
    if json_rpc_diagnostics_enabled() {
        eprintln!(
            "mini_agent_json_rpc method={} read_us={} parse_us={} dispatch_us={} queue_us={} serialize_us={} write_us={} bytes={}",
            diagnostic_method(&queued.method),
            queued.request_times.read_us,
            queued.request_times.parse_us,
            queued.request_times.dispatch_us,
            queue_us,
            serialized_us,
            write_us,
            bytes,
        );
    }
    Ok(())
}

fn json_rpc_diagnostics_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MINI_AGENT_JSON_RPC_DIAGNOSTICS")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
    })
}

fn diagnostic_method(method: &str) -> String {
    method
        .chars()
        .take(128)
        .map(|character| {
            if character.is_ascii_alphanumeric() || "._/-".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect()
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
    write_json_line_timed(writer, value).await.map(|_| ())
}

async fn write_json_line_timed<W: AsyncWrite + Unpin, T: serde::Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(u128, u128, usize), std::io::Error> {
    let serialize_started = Instant::now();
    let encoded =
        serde_json::to_vec(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    let serialize_us = serialize_started.elapsed().as_micros();
    let byte_count = encoded.len() + 1;
    let write_started = Instant::now();
    writer.write_all(&encoded).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok((
        serialize_us,
        write_started.elapsed().as_micros(),
        byte_count,
    ))
}

#[cfg(test)]
mod tests {
    use super::{MAX_JSON_RPC_LINE_BYTES, read_bounded_line};

    #[tokio::test]
    async fn rejects_json_rpc_lines_over_the_byte_limit() {
        let bytes = vec![b'x'; MAX_JSON_RPC_LINE_BYTES + 1024];
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(bytes));
        let mut line = String::new();
        let error = read_bounded_line(&mut reader, &mut line).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(line.len(), MAX_JSON_RPC_LINE_BYTES + 1);
    }
}
