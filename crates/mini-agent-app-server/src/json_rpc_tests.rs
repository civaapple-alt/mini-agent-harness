use super::*;
use crate::tests::DoneModel;
use mini_agent_app_server_protocol::CapabilityProviderSelection;
use mini_agent_app_server_protocol::ClientCapabilities;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::ResultStore;
use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPolicy;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_capabilities::workspace_tools_with_read_roots_and_results;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRegistry;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelUsage;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::TurnInput;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

fn connection() -> AppServerConnection<DoneModel> {
    AppServerConnection::new(crate::tests::server(DoneModel))
}

fn initialize_request(id: u64, client_name: &str) -> JsonRpcRequest {
    JsonRpcRequest::request(
        id,
        METHOD_INITIALIZE,
        serde_json::json!(InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client_name: client_name.to_string(),
            client_version: "0".to_string(),
            capabilities: ClientCapabilities::default(),
            profile: None,
            providers: None,
        }),
    )
}

fn turn_start_request(id: u64, prompt: &str) -> JsonRpcRequest {
    JsonRpcRequest::request(
        id,
        METHOD_TURN_START,
        serde_json::json!(TurnStartParams {
            thread_id: ThreadId::new("thread-1"),
            input: TurnInput::new(TurnInputMode::Start, prompt),
        }),
    )
}

fn rpc_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "mini-agent-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

struct ShellApprovalModel;

struct StepLimitModel;

struct BudgetModel;

struct TimeoutModel {
    release: Arc<tokio::sync::Notify>,
}

impl Model for ShellApprovalModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        if request.messages.iter().any(|message| {
            matches!(
                message,
                Message::Tool {
                    name,
                    outcome: Some(ToolExecutionStatus::Completed),
                    ..
                } if name == "shell"
            )
        }) {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "shell completed".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "shell-call-1".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": shell_approval_command()}),
            }],
            usage: None,
        })
    }
}

impl Model for StepLimitModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "step-limit-call".to_string(),
                name: "missing_tool".to_string(),
                arguments: serde_json::json!({}),
            }],
            usage: None,
        })
    }
}

impl Model for BudgetModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        Ok(ModelResponse {
            reasoning: String::new(),
            text: "budget reached".to_string(),
            tool_calls: Vec::new(),
            usage: Some(ModelUsage {
                input_tokens: 3,
                cached_input_tokens: 0,
                output_tokens: 2,
            }),
        })
    }
}

impl Model for TimeoutModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.release.notified().await;
        Ok(ModelResponse {
            reasoning: String::new(),
            text: "released".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

fn shell_approval_command() -> &'static str {
    #[cfg(windows)]
    {
        "Write-Output shell-approved"
    }
    #[cfg(not(windows))]
    {
        "printf shell-approved"
    }
}

fn managed_connection(name: &str) -> (AppServerConnection<DoneModel>, std::path::PathBuf) {
    managed_connection_with(DoneModel, name, crate::workflows::GoalLimits::default())
}

fn managed_connection_with<M: Model + Send + 'static>(
    model: M,
    name: &str,
    goal_limits: crate::workflows::GoalLimits,
) -> (AppServerConnection<M>, std::path::PathBuf) {
    let root = rpc_root(name);
    let workflows = WorkflowService::new(root.clone(), goal_limits);
    let server = crate::tests::server(model);
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

async fn wait_for_goal_status<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    status: &str,
) -> Value {
    loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(4), connection.next_notification())
                .await
                .expect("Goal execution should settle within the test deadline")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == status {
                return params;
            }
        }
    }
}

async fn wait_for_blocked_goal(connection: &mut AppServerConnection<DoneModel>) -> Value {
    loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(3), connection.next_notification())
                .await
                .expect("Goal preparation failure should settle within the test deadline")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == "blocked" {
                return params;
            }
        }
    }
}

#[tokio::test]
async fn exposes_session_world_and_mcp_management() {
    let (mut connection, root) = managed_connection("management-rpc");
    let response = connection
        .handle_request(initialize_request(1, "management-test"))
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
    let (connection, root) = managed_connection("management-rpc");
    let management = connection.runtime_management().unwrap().clone();
    let commands = management.server.command_sender();

    let (first_reply, first_response) = oneshot::channel();
    commands
        .send(crate::worker::Command::Runtime(
            crate::runtime_actor::RuntimeRequest {
                expected_revision: crate::action::RuntimeRevision::default(),
                command: crate::runtime_actor::RuntimeCommand::SetExecution {
                    approval: ApprovalMode::Interactive,
                    copilot: true,
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
                command: crate::runtime_actor::RuntimeCommand::SetExecution {
                    approval: ApprovalMode::Automatic,
                    copilot: true,
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
        .handle_request(initialize_request(2, "test"))
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
        .handle_request(turn_start_request(3, "hello"))
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
async fn exposes_read_only_workflow_state_from_thread_goal() {
    let (mut connection, root) = managed_connection("workflow-rpc");
    connection
        .handle_request(initialize_request(1, "workflow-test"))
        .await
        .unwrap();

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_SETTINGS_UPDATE,
            serde_json::json!({
                "threadId": "thread-1",
                "collaborationMode": {"mode": "plan"},
                "builtinTools": ["shell", "read_file"]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.result.unwrap()["stateRevision"], 1);

    let response = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "rpc goal"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.result.unwrap()["value"]["goal"]["status"],
        "active"
    );
    wait_for_blocked_goal(&mut connection).await;

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
    assert_eq!(state["collaborationMode"]["mode"], "plan");
    assert_eq!(state["goal"]["status"], "blocked");
    assert_eq!(state["goal"]["objective"], "rpc goal");
    assert!(state["goal"].get("path").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn broadcasts_thread_settings_updates_with_action_revision() {
    let (mut connection, root) = managed_connection("thread-settings-notification");
    connection
        .handle_request(initialize_request(1, "thread-settings-test"))
        .await
        .unwrap();

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_SETTINGS_UPDATE,
            serde_json::json!({
                "threadId": "thread-1",
                "collaborationMode": {"mode": "plan"},
                "builtinTools": ["shell", "read_file"]
            }),
        ))
        .await
        .unwrap();
    let response_revision = response.result.unwrap()["stateRevision"].clone();

    let notification = loop {
        let notification = connection.next_notification().await.unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_SETTINGS_UPDATED {
            break notification;
        }
    };
    let params = notification.params.unwrap();
    assert_eq!(params["threadId"], "thread-1");
    assert_eq!(params["collaborationMode"]["mode"], "plan");
    assert_eq!(
        params["builtinTools"],
        serde_json::json!(["shell", "read_file"])
    );
    assert_eq!(params["stateRevision"], response_revision);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exposes_codex_shaped_thread_goal_lifecycle() {
    let (mut connection, root) = managed_connection("thread-goal-rpc");
    connection
        .handle_request(initialize_request(1, "thread-goal-test"))
        .await
        .unwrap();

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_GOAL_GET,
            serde_json::json!({"threadId": "thread-1"}),
        ))
        .await
        .unwrap();
    assert!(response.result.unwrap()["value"]["goal"].is_null());

    let response = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "ship the next iteration",
                "tokenBudget": 1200
            }),
        ))
        .await
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result["value"]["goal"]["objective"],
        "ship the next iteration"
    );
    assert_eq!(result["value"]["goal"]["status"], "active");
    assert_eq!(result["value"]["goal"]["tokenBudget"], 1200);
    assert!(result["value"]["goal"].get("path").is_none());
    let mut active_turn_seen = false;
    let notification = loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(3), connection.next_notification())
                .await
                .expect("Goal preparation failure should settle within the test deadline")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == "active" && params["turnId"] == "turn-1" {
                active_turn_seen = true;
            }
            if params["goal"]["status"] == "blocked" {
                break params;
            }
        }
    };
    assert!(active_turn_seen);
    assert_eq!(notification["goal"]["objective"], "ship the next iteration");
    assert_eq!(notification["turnId"], "turn-1");

    let response = connection
        .handle_request(JsonRpcRequest::request(
            4,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "replace after verifier preparation failure"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.result.unwrap()["value"]["goal"]["objective"],
        "replace after verifier preparation failure"
    );
    wait_for_blocked_goal(&mut connection).await;

    let response = connection
        .handle_request(JsonRpcRequest::request(
            5,
            METHOD_THREAD_GOAL_CLEAR,
            serde_json::json!({"threadId": "thread-1"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.result.unwrap()["value"]["cleared"], true);
    loop {
        let notification = connection.next_notification().await.unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_CLEARED {
            break;
        }
    }

    let response = connection
        .handle_request(JsonRpcRequest::request(
            6,
            METHOD_THREAD_GOAL_GET,
            serde_json::json!({"threadId": "thread-1"}),
        ))
        .await
        .unwrap();
    assert!(response.result.unwrap()["value"]["goal"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn enforces_goal_step_budget_at_the_core_boundary() {
    let (mut connection, root) = managed_connection_with(
        StepLimitModel,
        "goal-step-budget",
        crate::workflows::GoalLimits {
            milestone_step_budget: 1,
            ..crate::workflows::GoalLimits::default()
        },
    );
    connection
        .handle_request(initialize_request(1, "goal-step-budget-test"))
        .await
        .unwrap();
    connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "stop after one model step"
            }),
        ))
        .await
        .unwrap();

    let notification = wait_for_goal_status(&mut connection, "usageLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn records_goal_usage_and_enforces_token_budget() {
    let (mut connection, root) = managed_connection_with(
        BudgetModel,
        "goal-token-budget",
        crate::workflows::GoalLimits::default(),
    );
    connection
        .handle_request(initialize_request(1, "goal-token-budget-test"))
        .await
        .unwrap();
    connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "stop at the token budget",
                "tokenBudget": 5
            }),
        ))
        .await
        .unwrap();

    let notification = wait_for_goal_status(&mut connection, "budgetLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 5);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn enforces_goal_timeout_with_cooperative_cancellation() {
    let release = Arc::new(tokio::sync::Notify::new());
    let (mut connection, root) = managed_connection_with(
        TimeoutModel {
            release: release.clone(),
        },
        "goal-timeout",
        crate::workflows::GoalLimits {
            milestone_timeout_secs: 1,
            ..crate::workflows::GoalLimits::default()
        },
    );
    connection
        .handle_request(initialize_request(1, "goal-timeout-test"))
        .await
        .unwrap();
    connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_GOAL_SET,
            serde_json::json!({
                "threadId": "thread-1",
                "objective": "stop when the milestone times out"
            }),
        ))
        .await
        .unwrap();

    loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(2), connection.next_notification())
                .await
                .unwrap()
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED
            && notification.params.as_ref().is_some_and(|params| {
                params["turnId"] == "turn-1" && params["goal"]["status"] == "active"
            })
        {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    release.notify_one();

    let notification = wait_for_goal_status(&mut connection, "usageLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn binds_active_goal_workspace_to_approval_controller() {
    let root = rpc_root("goal-approval-path");
    mini_agent_host::HostWorkflowStore::new(root.clone(), crate::workflows::GoalLimits::default())
        .set_goal("bind the goal workspace", None)
        .unwrap();
    mini_agent_host::HostWorkflowStore::new(root.clone(), crate::workflows::GoalLimits::default())
        .pause_goal()
        .unwrap();
    let approval = ApprovalController::with_preset(ApprovalMode::Automatic, Default::default());
    let observed_approval = approval.clone();
    let server = crate::tests::server(DoneModel);
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
        approval,
    );
    let workflows = WorkflowService::new(root.clone(), crate::workflows::GoalLimits::default());
    let _connection = AppServerConnection::new(server)
        .with_runtime_services(RuntimeServices::new(management, workflows).unwrap());
    assert_eq!(
        observed_approval.goal_dir(),
        Some(mini_agent_capabilities::normalize_path(&root.join("goal")))
    );
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
        .handle_request(initialize_request(1, "test"))
        .await;
    let _ = connection
        .handle_request(turn_start_request(2, "hello"))
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
    let request = initialize_request(1, "jsonl-test");
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
async fn serves_builtin_shell_approval_with_request_turn_and_call_identity() {
    let root = rpc_root("shell-approval-rpc");
    let broker = ApprovalBroker::new();
    let approval_broker = broker.clone();
    let approval = ApprovalController::with_policy_and_context_callback(
        ApprovalMode::Interactive,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        move |request| {
            approval_broker
                .request_with_context(request)
                .map_err(mini_agent_protocol::ToolError)
        },
    );
    let tools = workspace_tools_with_read_roots_and_results(
        root.clone(),
        approval.clone(),
        Vec::new(),
        SandboxKind::Native,
        ImageStore::memory_only(),
        ResultStore::default(),
    )
    .unwrap();
    let registry = ToolRegistry::with_executor(
        tools,
        Arc::new(mini_agent_host::ToolOrchestrator::new(approval)),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(
            ThreadId::new("initial"),
            Harness::new(ShellApprovalModel, registry, HarnessConfig::default()),
        ),
    );

    let mut connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server,
        broker.clone(),
        default_capability_manifest(),
    );
    let initialize = connection
        .handle_request(initialize_request(1, "shell-rpc"))
        .await
        .unwrap();
    assert_eq!(
        initialize.result.unwrap()["capabilities"]["approvalRequests"],
        true
    );

    let turn_response = connection
        .handle_request(turn_start_request(2, "run shell"))
        .await
        .unwrap();
    assert_eq!(turn_response.result.unwrap()["value"]["turn_id"], "turn-1");

    let pending = tokio::time::timeout(Duration::from_secs(3), broker.next_request())
        .await
        .expect("Shell approval should reach the App Server broker");
    assert_eq!(pending.request_id, "approval-1");
    assert_eq!(
        pending.action,
        format!("shell command `{}`", shell_approval_command())
    );
    assert_eq!(pending.call_id.as_deref(), Some("shell-call-1"));
    assert_eq!(pending.thread_id, Some(ThreadId::new("thread-1")));
    assert_eq!(
        pending.turn_id,
        Some(mini_agent_protocol::TurnId::new("turn-1"))
    );

    let approval_response = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_APPROVAL_RESPOND,
            serde_json::to_value(ApprovalRespondParams {
                request_id: pending.request_id.clone(),
                approved: true,
                remember: false,
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(approval_response.result.unwrap()["accepted"], true);

    let resolution = tokio::time::timeout(Duration::from_secs(3), broker.next_event())
        .await
        .expect("Shell approval resolution should be recorded");
    let resolution = match resolution {
        ApprovalEvent::Resolved(resolution) => resolution,
        ApprovalEvent::Requested(_) => panic!("expected approval resolution"),
    };
    assert_eq!(resolution.request_id, "approval-1");
    assert_eq!(resolution.call_id.as_deref(), Some("shell-call-1"));
    assert_eq!(resolution.thread_id, Some(ThreadId::new("thread-1")));
    assert_eq!(
        resolution.turn_id,
        Some(mini_agent_protocol::TurnId::new("turn-1"))
    );
    assert!(resolution.approved);

    let mut tool_finished_seen = false;
    loop {
        let notification = connection.next_notification().await.unwrap();
        assert_eq!(notification.method, METHOD_TURN_EVENT);
        let params = notification.params.unwrap();
        let notification: TurnEventNotification = serde_json::from_value(params).unwrap();
        assert_eq!(
            notification.turn_id,
            Some(mini_agent_protocol::TurnId::new("turn-1"))
        );
        match notification.event {
            mini_agent_protocol::Event::ToolFinished {
                call_id,
                name,
                outcome: Some(ToolExecutionStatus::Completed),
                ..
            } => {
                assert_eq!(call_id, "shell-call-1");
                assert_eq!(name, "shell");
                tool_finished_seen = true;
            }
            mini_agent_protocol::Event::TurnFinished { .. } => break,
            _ => {}
        }
    }
    assert!(tool_finished_seen);
    std::fs::remove_dir_all(root).unwrap();
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
        .handle_request(initialize_request(1, "json-rpc-trace"))
        .await
        .unwrap();
    json_rpc
        .handle_request(turn_start_request(2, "hello"))
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
        .handle_request(initialize_request(1, "checkpoint-test"))
        .await;
    let started = connection
        .handle_request(turn_start_request(2, "hello"))
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
    let mut connection = AppServerConnection::with_approval_broker_and_capability_manifest(
        server,
        broker.clone(),
        default_capability_manifest(),
    );
    let _ = connection
        .handle_request(initialize_request(1, "approval-test"))
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
        .handle_request(initialize_request(1, "lifecycle-test"))
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
        .handle_request(initialize_request(1, "notification-test"))
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
