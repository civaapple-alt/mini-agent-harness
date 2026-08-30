use super::*;
use mini_agent_app_server_protocol::CapabilityProviderSelection;
use mini_agent_app_server_protocol::ClientCapabilities;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_capabilities::SandboxKind;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRegistry;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::TurnInput;
use std::convert::Infallible;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

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

fn workflow_connection() -> (AppServerConnection<DoneModel>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "mini-agent-workflow-rpc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let workflows = WorkflowService::new(root.clone(), crate::workflows::GoalLimits::default());
    let management = RuntimeManagementService::new(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect(
            &root,
            ApprovalMode::Automatic,
            false,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        ApprovalController::with_preset(ApprovalMode::Automatic, Default::default()),
    );
    (
        AppServerConnection::new(server)
            .with_runtime_services(RuntimeServices::new(management, workflows).unwrap()),
        root,
    )
}

fn management_connection() -> (AppServerConnection<DoneModel>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "mini-agent-management-rpc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let management = RuntimeManagementService::new(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect(
            &root,
            ApprovalMode::Automatic,
            false,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        ApprovalController::with_preset(ApprovalMode::Automatic, Default::default()),
    );
    (
        AppServerConnection::new(server).with_runtime_services(
            RuntimeServices::new(
                management,
                WorkflowService::new(root.clone(), crate::workflows::GoalLimits::default()),
            )
            .unwrap(),
        ),
        root,
    )
}

#[tokio::test]
async fn exposes_session_world_and_mcp_management() {
    let (mut connection, root) = management_connection();
    let response = connection
        .handle_request(JsonRpcRequest::request(
            1,
            METHOD_INITIALIZE,
            serde_json::json!(InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                client_name: "management-test".to_string(),
                client_version: "0".to_string(),
                capabilities: ClientCapabilities::default(),
                profile: None,
                providers: None,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.result.unwrap()["capabilities"]["runtimeManagement"],
        true
    );

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_SESSION_INFO,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert!(result["value"].is_null());
    assert_eq!(
        result["actionId"], 1,
        "session/info is the first admitted runtime action"
    );
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_WORLD_STATE,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["value"]["workspace"], root.display().to_string());
    assert_eq!(result["actionId"], 2);
    assert_eq!(result["actionSequence"], 2);
    assert_eq!(result["stateRevision"], 0);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            4,
            METHOD_MCP_STATUS,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["value"]["toolCount"], 0);
    assert_eq!(result["actionId"], 3);
    assert_eq!(result["actionSequence"], 3);
    assert_eq!(result["stateRevision"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn runtime_mutations_reject_stale_revision_tokens() {
    let (connection, root) = management_connection();
    let management = connection.runtime_management().unwrap().clone();
    let world = management.world().await.unwrap();
    let commands = management.server.command_sender();
    let first_world = world.with_execution(ApprovalMode::Interactive, true, world.sandbox());
    let second_world = world.with_execution(ApprovalMode::Automatic, true, world.sandbox());

    let (first_reply, first_response) = oneshot::channel();
    commands
        .send(crate::worker::Command::Runtime(
            crate::runtime_actor::RuntimeRequest {
                expected_revision: crate::action::RuntimeRevision::default(),
                command: crate::runtime_actor::RuntimeCommand::UpdateWorld {
                    updated: first_world,
                    reply: first_reply,
                },
            },
        ))
        .await
        .unwrap();
    let (second_reply, second_response) = oneshot::channel();
    commands
        .send(crate::worker::Command::Runtime(
            crate::runtime_actor::RuntimeRequest {
                expected_revision: crate::action::RuntimeRevision::default(),
                command: crate::runtime_actor::RuntimeCommand::UpdateWorld {
                    updated: second_world,
                    reply: second_reply,
                },
            },
        ))
        .await
        .unwrap();

    let first = first_response.await.unwrap();
    let second = second_response.await.unwrap();
    assert!(first.is_ok() ^ second.is_ok());
    let conflict = if first.is_err() { first } else { second };
    assert!(matches!(
        conflict.map_err(|failure| failure.error),
        Err(AppServerError::RevisionConflict {
            expected: 0,
            actual: 1
        })
    ));
    std::fs::remove_dir_all(root).unwrap();
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
                providers: None,
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
    let result = response.result.unwrap();
    assert_eq!(result["value"]["status"], "started");
    assert_eq!(result["actionId"], 1);
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);
}

#[tokio::test]
async fn exposes_workflow_management_without_host_paths() {
    let (mut connection, root) = workflow_connection();
    let response = connection
        .handle_request(JsonRpcRequest::request(
            1,
            METHOD_INITIALIZE,
            serde_json::json!(InitializeParams {
                protocol_version: PROTOCOL_VERSION,
                client_name: "workflow-test".to_string(),
                client_version: "0".to_string(),
                capabilities: ClientCapabilities::default(),
                profile: None,
                providers: None,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.result.unwrap()["capabilities"]["workflows"], true);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_WORKFLOW_PLAN_SET,
            serde_json::json!({"active": true, "prompt": "rpc plan"}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["value"]["planActive"], true);
    assert_eq!(result["actionId"], 1);
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 1);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_WORKFLOW_GOAL_START,
            serde_json::json!({"objective": "rpc goal"}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["value"]["status"], "running");
    assert_eq!(result["actionId"], 3);
    assert_eq!(result["actionSequence"], 3);
    assert_eq!(result["stateRevision"], 2);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            4,
            METHOD_WORKFLOW_STATE,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    let state = result["value"].clone();
    assert_eq!(state["planActive"], true);
    assert_eq!(state["goal"]["status"], "running");
    assert!(state["goal"].get("planFile").is_none());
    assert_eq!(result["actionId"], 4);
    assert_eq!(result["actionSequence"], 4);
    assert_eq!(result["stateRevision"], 2);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            5,
            METHOD_WORKFLOW_GOAL_PAUSE,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["value"]["status"], "user_paused");
    assert_eq!(result["actionId"], 5);
    assert_eq!(result["actionSequence"], 5);
    assert_eq!(result["stateRevision"], 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rejects_an_unavailable_requested_provider() {
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
                profile: None,
                providers: Some(CapabilityProviderSelection {
                    tools: Some("builtin".to_string()),
                    ..CapabilityProviderSelection::default()
                }),
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.error.unwrap().code, -32602);
    assert!(!connection.initialized());
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
                providers: None,
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
    let task = tokio::spawn(serve_stdio_with_approval_and_manifest(
        connection().server.clone(),
        ApprovalBroker::new(),
        default_capability_manifest(),
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
            providers: None,
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
        mini_agent_protocol::TurnSubmission::Started { .. }
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
        let finished = matches!(event.event, mini_agent_protocol::Event::TurnFinished { .. });
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
                providers: None,
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
        let finished = matches!(
            envelope.event,
            mini_agent_protocol::Event::TurnFinished { .. }
        );
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
                providers: None,
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
    let turn_id: mini_agent_protocol::TurnId =
        serde_json::from_value(started.result.unwrap()["value"]["turn_id"].clone()).unwrap();
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
    assert_eq!(turn.result.unwrap()["value"]["finalText"], "done");

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
    assert_eq!(thread.result.unwrap()["value"]["status"], "idle");
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
                providers: None,
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
                providers: None,
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
    let result = created.result.unwrap();
    assert_eq!(result["value"]["threadId"], "thread-2");
    assert_eq!(result["actionId"], 1);
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);
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
    let result = forked.result.unwrap();
    assert_eq!(result["value"]["threadId"], "thread-3");
    assert_eq!(result["actionId"], 2);
    assert_eq!(result["actionSequence"], 2);
    assert_eq!(result["stateRevision"], 0);
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
                providers: None,
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
