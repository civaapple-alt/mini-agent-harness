use super::*;
use crate::tests::{DoneModel, harness};
use mini_agent_app_server_protocol::{
    ActionGrantScope, ApprovalDecision, CapabilityProviderSelection, ClientCapabilities,
    METHOD_USER_QUESTION_RESPOND, SESSION_FORK_CONFLICT_CODE, TurnSource,
};
use mini_agent_capabilities::{
    ApprovalController, ApprovalPolicy, BackgroundShellManager, ImageStore, ResultStore,
    SandboxKind, ScheduledTaskManager, SecurityPolicy, SecurityPreset,
    SessionRequest as SessionStoreRequest, SessionStore,
    workspace_tools_with_read_roots_and_results,
    workspace_tools_with_read_roots_results_and_background_shells,
};
use mini_agent_core::{
    ContextLimitBehavior, ExecutionJournalSink, Harness, HarnessConfig, Thread, ToolRouter,
};
use mini_agent_protocol::{
    Message, Model, ModelEvent, ModelEventSink, ModelRequest, ModelResponse, ModelUsage, ThreadId,
    ThreadStart, ToolApprovalRequest, ToolCall, ToolError, ToolExecutionStatus, ToolHandler,
    ToolRuntime, ToolSpec, TurnInput,
};
use serde_json::Value;
use std::convert::Infallible;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
            operation_id: None,
            operation_attempt: None,
            operation_attempt_kind: None,
            operation_group_id: None,
            execution_mode: None,
            group_sequence: None,
            turn_source: None,
        }),
    )
}

fn session_turn_start_request(
    id: u64,
    thread_id: &str,
    prompt: &str,
    turn_source: Option<TurnSource>,
) -> JsonRpcRequest {
    JsonRpcRequest::request(
        id,
        METHOD_TURN_START,
        serde_json::json!(TurnStartParams {
            thread_id: ThreadId::new(thread_id),
            input: TurnInput::new(TurnInputMode::Start, prompt),
            operation_id: None,
            operation_attempt: None,
            operation_attempt_kind: None,
            operation_group_id: None,
            execution_mode: None,
            group_sequence: None,
            turn_source,
        }),
    )
}

#[tokio::test]
async fn approval_request_ids_remain_unique_across_brokers() {
    let first = ApprovalBroker::new();
    let second = ApprovalBroker::new();
    let first_receiver = first.clone();
    let second_receiver = second.clone();
    let second_request_broker = second_receiver.clone();
    let request = ToolApprovalRequest {
        action: "shell command `pwd`".to_string(),
        tool_name: Some("shell".to_string()),
        call_id: Some("same-call-id-fixture".to_string()),
        ..ToolApprovalRequest::default()
    };

    let first_task = tokio::task::spawn_blocking(move || first.request_resolution(&request));
    let second_task = tokio::task::spawn_blocking(move || {
        second_request_broker.request_resolution(&ToolApprovalRequest {
            action: "shell command `pwd`".to_string(),
            tool_name: Some("shell".to_string()),
            call_id: Some("same-call-id-fixture".to_string()),
            ..ToolApprovalRequest::default()
        })
    });

    let first_pending = tokio::time::timeout(Duration::from_secs(2), first_receiver.next_request())
        .await
        .expect("first approval should be queued");
    let second_pending =
        tokio::time::timeout(Duration::from_secs(2), second_receiver.next_request())
            .await
            .expect("second approval should be queued");
    assert_ne!(first_pending.request_id, second_pending.request_id);

    for (broker, request_id) in [
        (first_receiver, first_pending.request_id),
        (second_receiver, second_pending.request_id),
    ] {
        broker
            .respond(ApprovalRespondParams {
                request_id,
                decision: ApprovalDecision::Approve,
                grant_scope: Some(ActionGrantScope::Once),
                reason: None,
            })
            .expect("approval should resolve its own broker entry");
    }
    first_task.await.unwrap().unwrap();
    second_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn exposes_empty_background_shell_task_list_and_capability() {
    let (mut connection, root) = managed_connection("background-shell-list");
    let initialize = connection
        .handle_request(initialize_request(1, "background-shell-test"))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(initialize["capabilities"]["backgroundTasks"], true);
    let result = rpc_call(
        &mut connection,
        2,
        METHOD_BACKGROUND_TASK_LIST,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(result["value"]["data"], serde_json::json!([]));
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exposes_empty_scheduled_task_list_and_capability() {
    let (mut connection, root) = managed_connection("scheduled-task-list");
    let initialize = connection
        .handle_request(initialize_request(1, "scheduled-task-test"))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(initialize["capabilities"]["scheduledTasks"], true);
    let result = rpc_call(
        &mut connection,
        2,
        METHOD_SCHEDULED_TASK_LIST,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(result["value"]["data"], serde_json::json!([]));
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn negotiates_user_questions_and_routes_scoped_answer_responses() {
    use mini_agent_protocol::{UserQuestion, UserQuestionInteraction};

    let broker = UserQuestionBroker::new();
    broker.restore_pending(UserQuestionInteraction {
        interaction_id: "uq-protocol".to_string(),
        thread_id: ThreadId::new("thread-1"),
        turn_id: mini_agent_protocol::TurnId::new("turn-1"),
        call_id: "call-1".to_string(),
        questions: vec![UserQuestion {
            id: "q1".to_string(),
            prompt: "Choose".to_string(),
            options: vec![mini_agent_protocol::UserQuestionOption {
                id: "q1-o1".to_string(),
                label: "Recommended".to_string(),
                description: None,
                recommended: true,
                recommendation_reason: Some("Fits the goal".to_string()),
            }],
            allow_free_text: true,
            allow_skip: true,
        }],
        answers: vec![None],
        current_index: 0,
    });
    let mut connection =
        AppServerConnection::new(crate::tests::server(DoneModel)).with_user_questions(broker, true);
    let mut initialize = initialize_request(1, "user-question-client");
    initialize.params.as_mut().expect("initialize has params")["capabilities"]["userQuestions"] =
        serde_json::json!(true);
    let initialized = connection
        .handle_request(initialize)
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(initialized["capabilities"]["userQuestions"], true);

    let checkpoint = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_READ,
            serde_json::json!({ "threadId": "thread-1" }),
        ))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        checkpoint["value"]["pendingUserQuestion"]["interactionId"],
        "uq-protocol"
    );

    let answered = connection
        .handle_request(JsonRpcRequest::request(
            3,
            METHOD_USER_QUESTION_RESPOND,
            serde_json::json!({
                "interactionId": "uq-protocol",
                "threadId": "thread-1",
                "turnId": "turn-1",
                "callId": "call-1",
                "questionId": "q1",
                "answer": { "type": "option", "optionId": "q1-o1" }
            }),
        ))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(answered["accepted"], true);
    assert_eq!(answered["interaction"]["currentIndex"], 1);

    let stale = connection
        .handle_request(JsonRpcRequest::request(
            4,
            METHOD_USER_QUESTION_RESPOND,
            serde_json::json!({
                "interactionId": "uq-protocol",
                "threadId": "another-thread",
                "turnId": "turn-1",
                "callId": "call-1",
                "questionId": "q1",
                "answer": { "type": "option", "optionId": "q1-o1" }
            }),
        ))
        .await
        .unwrap();
    assert!(stale.error.is_some());
    connection.shutdown().await.unwrap();
}

#[tokio::test]
async fn skills_list_refreshes_project_discovery_after_runtime_start() {
    let root = rpc_root("skills-list-refresh");
    let registry = mini_agent_capabilities::CapabilityRegistry::builtin();
    let enabled_groups = Vec::new();
    let discovery = registry
        .discover_extensions_with_builtin_groups("builtin", &root, &enabled_groups)
        .unwrap();
    let skill_read_roots =
        mini_agent_capabilities::SkillReadRoots::from_paths(discovery.skill_read_roots());
    let refresh = mini_agent_host::SkillDiscoveryRefresh::new(
        registry,
        "builtin",
        root.clone(),
        enabled_groups,
        mini_agent_host::ExtensionSelection::All,
        true,
    );
    let approval = ApprovalController::with_preset(ApprovalPolicy::Automatic, Default::default());
    let server = crate::tests::server(DoneModel);
    let management = RuntimeManagementService::new_with_harness_config_and_skills(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        approval,
        HarnessConfig::default(),
        Some(discovery),
    )
    .with_skill_discovery_refresh(Some(refresh), skill_read_roots);
    let services = RuntimeServices::new(
        management,
        ThreadSettingsService::new(),
        ThreadGoalRequestProcessor::new(root.clone(), crate::goal_service::GoalLimits::default()),
    )
    .unwrap();
    let mut connection = AppServerConnection::new(server).with_runtime_services(services);

    let initialized = connection
        .handle_request(initialize_request(1, "skills-list-test"))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(initialized["capabilities"]["skillsList"], true);

    let skill_dir = root.join(".agents/skills/installed-during-runtime");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: installed-during-runtime\ndescription: A test Skill.\n---\nUse this test Skill.\n",
    )
    .unwrap();

    let listed = rpc_call(
        &mut connection,
        2,
        mini_agent_app_server_protocol::METHOD_SKILLS_LIST,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert!(
        listed["value"]["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| {
                skill["name"] == "installed-during-runtime"
                    && skill["source"] == "project"
                    && skill["enabled"] == true
            })
    );

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn background_shell_survives_turn_and_is_controlled_by_next_rpc() {
    let root = rpc_root("background-shell-lifecycle");
    let background_shells = BackgroundShellManager::new();
    let approval = ApprovalController::with_policy_and_callback(
        ApprovalPolicy::Automatic,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        |_| {
            Ok(mini_agent_protocol::ToolApprovalResolution {
                outcome: mini_agent_protocol::ApprovalOutcome::Approved,
                grant_scope: mini_agent_protocol::ActionGrantScope::Once,
                reason: None,
            })
        },
    );
    let scheduled_tasks = ScheduledTaskManager::new();
    scheduled_tasks.bind_owner("thread-1");
    let mut tools = workspace_tools_with_read_roots_results_and_background_shells(
        root.clone(),
        approval.clone(),
        Vec::new(),
        Vec::new(),
        SandboxKind::Native,
        ImageStore::memory_only(),
        ResultStore::default(),
        background_shells.clone(),
    )
    .unwrap();
    tools.retain(|tool| tool.spec().name != "scheduled_task");
    tools.extend(mini_agent_capabilities::scheduled_task_tools(
        scheduled_tasks.clone(),
    ));
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(
            ThreadId::new("initial"),
            Harness::new(
                ScenarioModel::BackgroundShell,
                ToolRouter::with_executor(
                    tools,
                    Arc::new(mini_agent_host::ToolOrchestrator::new(approval.clone())),
                ),
                HarnessConfig::default(),
            ),
        ),
    );
    let management = RuntimeManagementService::new_with_harness_config_and_skills_and_task_managers(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        approval,
        HarnessConfig::default(),
        None,
        background_shells,
        scheduled_tasks,
    );
    let mut connection = AppServerConnection::new(server).with_runtime_services(
        RuntimeServices::new(
            management,
            ThreadSettingsService::new(),
            ThreadGoalRequestProcessor::new(
                root.clone(),
                crate::goal_service::GoalLimits::default(),
            ),
        )
        .unwrap(),
    );
    initialize_connection(&mut connection, "background-shell-lifecycle").await;
    let started = start_turn(
        &mut connection,
        2,
        "Start a local process that must outlive this Turn.",
    )
    .await;
    assert_eq!(started["value"]["turn_id"], "turn-thread-1-1");
    wait_for_turn_finished(&mut connection).await;

    let listed = rpc_call(
        &mut connection,
        3,
        METHOD_BACKGROUND_TASK_LIST,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(listed["value"]["data"].as_array().unwrap().len(), 1);
    assert_eq!(listed["value"]["data"][0]["taskId"], "scenario-task");
    assert!(matches!(
        listed["value"]["data"][0]["state"].as_str(),
        Some("starting" | "running")
    ));

    let delayed = rpc_call(
        &mut connection,
        4,
        METHOD_SCHEDULED_TASK_LIST,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(delayed["value"]["data"], serde_json::json!([]));

    let restarted = rpc_call(
        &mut connection,
        5,
        METHOD_BACKGROUND_TASK_RESTART,
        serde_json::json!({"threadId": "thread-1", "taskId": "scenario-task"}),
    )
    .await;
    assert!(matches!(
        restarted["value"]["state"].as_str(),
        Some("starting" | "running")
    ));

    let stopped = rpc_call(
        &mut connection,
        6,
        METHOD_BACKGROUND_TASK_STOP,
        serde_json::json!({"threadId": "thread-1", "taskId": "scenario-task"}),
    )
    .await;
    assert_eq!(stopped["value"]["state"], "stopped");
    let logs = rpc_call(
        &mut connection,
        7,
        METHOD_BACKGROUND_TASK_LOGS,
        serde_json::json!({"threadId": "thread-1", "taskId": "scenario-task"}),
    )
    .await;
    assert!(logs["value"].get("text").is_some());

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

async fn initialize_connection<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    client_name: &str,
) {
    connection
        .handle_request(initialize_request(1, client_name))
        .await
        .unwrap();
}

async fn rpc_result<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    request: JsonRpcRequest,
) -> Value {
    connection
        .handle_request(request)
        .await
        .unwrap()
        .result
        .unwrap()
}

async fn rpc_call<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    rpc_result(connection, JsonRpcRequest::request(id, method, params)).await
}

async fn start_turn<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    id: u64,
    prompt: &str,
) -> Value {
    rpc_result(connection, turn_start_request(id, prompt)).await
}

fn goal_set_request(
    id: u64,
    objective: Option<&str>,
    status: Option<&str>,
    token_budget: Option<i64>,
) -> JsonRpcRequest {
    let mut params = serde_json::json!({"threadId": "thread-1"});
    if let Some(objective) = objective {
        params["objective"] = Value::String(objective.to_string());
    }
    if let Some(status) = status {
        params["status"] = Value::String(status.to_string());
    }
    if let Some(token_budget) = token_budget {
        params["tokenBudget"] = Value::from(token_budget);
    }
    JsonRpcRequest::request(id, METHOD_THREAD_GOAL_SET, params)
}

async fn set_goal<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    id: u64,
    objective: Option<&str>,
    status: Option<&str>,
    token_budget: Option<i64>,
) -> Value {
    rpc_result(
        connection,
        goal_set_request(id, objective, status, token_budget),
    )
    .await
}

async fn next_turn_event<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
) -> TurnEventNotification {
    loop {
        let notification = connection.next_notification().await.unwrap();
        if notification.method == METHOD_TURN_EVENT {
            return serde_json::from_value(notification.params.unwrap()).unwrap();
        }
    }
}

async fn wait_for_turn_finished<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
) {
    loop {
        if matches!(
            next_turn_event(connection).await.event,
            mini_agent_protocol::Event::TurnFinished { .. }
        ) {
            break;
        }
    }
}

async fn wait_for_turn_finished_id<M: Model + Send + 'static>(
    connection: &mut AppServerConnection<M>,
    turn_id: &str,
) {
    loop {
        let notification = next_turn_event(connection).await;
        if notification
            .turn_id
            .as_ref()
            .is_some_and(|id| id.as_str() == turn_id)
            && matches!(
                notification.event,
                mini_agent_protocol::Event::TurnFinished { .. }
            )
        {
            break;
        }
    }
}

#[derive(Clone, Copy)]
enum ToolOutputRecoveryMode {
    InitialTurn,
    ResumedSession,
}

struct ToolOutputRecoveryModel {
    mode: ToolOutputRecoveryMode,
}

impl Model for ToolOutputRecoveryModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        if request.tools.is_empty() {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "Continue from the retained Session output pointer and cursor.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }

        let shell_output = last_tool_output(&request, "shell");
        let read_output = last_tool_output(&request, "read_tool_output");
        let tool_call = match self.mode {
            ToolOutputRecoveryMode::InitialTurn if shell_output.is_none() => {
                let command = if cfg!(windows) {
                    "Write-Output ('x' * 50000)"
                } else {
                    "printf 'HEAD'; head -c 50000 /dev/zero | tr '\\000' 'x'; printf 'TAIL'"
                };
                Some(ToolCall {
                    id: "large-shell-output".to_string(),
                    name: "shell".to_string(),
                    arguments: serde_json::json!({"command": command}),
                })
            }
            ToolOutputRecoveryMode::InitialTurn if read_output.is_none() => {
                let handle = shell_output
                    .and_then(|content| content.split_once("Full output handle: "))
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    .map(|value| value.trim_end_matches('.'))
                    .unwrap_or_else(|| {
                        panic!("large Shell result should provide a Session output handle: {shell_output:?}")
                    });
                Some(read_output_call(handle, 0))
            }
            ToolOutputRecoveryMode::ResumedSession => {
                let Some(previous_page) = read_output else {
                    panic!("resumed Session should retain the first output page")
                };
                let cursor = output_page_field(previous_page, "cursor")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if cursor == 0 {
                    let handle = output_page_field(previous_page, "handle")
                        .expect("persisted output page should retain its handle");
                    let next_cursor = output_page_field(previous_page, "next_cursor")
                        .and_then(|value| value.parse::<usize>().ok())
                        .expect("first output page should expose a next cursor");
                    Some(read_output_call(handle, next_cursor))
                } else {
                    None
                }
            }
            ToolOutputRecoveryMode::InitialTurn => None,
        };

        if let Some(tool_call) = tool_call {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![tool_call],
                usage: Some(ModelUsage {
                    input_tokens: 120,
                    cached_input_tokens: Some(60),
                    output_tokens: 8,
                }),
            });
        }

        let text = match self.mode {
            ToolOutputRecoveryMode::InitialTurn => "First bounded page read before restart.",
            ToolOutputRecoveryMode::ResumedSession => {
                let page = read_output.expect("the resumed page should be available");
                assert!(page.contains("cursor: 10240"), "unexpected page: {page}");
                "Second bounded page read after restart."
            }
        }
        .to_string();
        events.emit(ModelEvent::TextDelta(text.clone()));
        Ok(ModelResponse {
            reasoning: String::new(),
            text,
            tool_calls: Vec::new(),
            usage: Some(ModelUsage {
                input_tokens: 120,
                cached_input_tokens: Some(60),
                output_tokens: 8,
            }),
        })
    }
}

fn last_tool_output<'a>(request: &ModelRequest<'a>, name: &str) -> Option<&'a str> {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Tool {
                name: tool_name,
                content,
                ..
            } if tool_name == name => Some(content.as_str()),
            _ => None,
        })
}

fn output_page_field<'a>(content: &'a str, field: &str) -> Option<&'a str> {
    content
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{field}: ")))
}

fn read_output_call(handle: &str, cursor: usize) -> ToolCall {
    ToolCall {
        id: format!("read-output-{cursor}"),
        name: "read_tool_output".to_string(),
        arguments: serde_json::json!({"handle": handle, "cursor": cursor, "max_bytes": 10 * 1024}),
    }
}

fn tool_output_session_connection(
    model: ToolOutputRecoveryModel,
    root: PathBuf,
    opened: mini_agent_capabilities::OpenedSession,
) -> AppServerConnection<ToolOutputRecoveryModel> {
    let thread_id = ThreadId::new(opened.store.thread_id().to_string());
    let approval = ApprovalController::with_policy_and_callback(
        ApprovalPolicy::Automatic,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        |_| {
            Ok(mini_agent_protocol::ToolApprovalResolution {
                outcome: mini_agent_protocol::ApprovalOutcome::Approved,
                grant_scope: mini_agent_protocol::ActionGrantScope::Once,
                reason: None,
            })
        },
    );
    approval.bind_session_file(opened.store.path());
    let results = opened.store.result_store();
    let mut tools = workspace_tools_with_read_roots_and_results(
        root.clone(),
        approval.clone(),
        Vec::new(),
        Vec::new(),
        SandboxKind::Native,
        ImageStore::memory_only(),
        results,
    )
    .unwrap();
    tools.retain(|tool| matches!(tool.spec().name.as_str(), "shell" | "read_tool_output"));

    let config = HarnessConfig {
        max_context_bytes: 28 * 1024,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(
        model,
        ToolRouter::with_executor(
            tools,
            Arc::new(mini_agent_host::ToolOrchestrator::new(approval.clone())),
        ),
        config.clone(),
    );
    if opened.resumed {
        harness.restore_session(opened.state.clone()).unwrap();
    }
    let server = AppServer::new(
        ThreadStart::new(thread_id.clone()),
        Thread::new(thread_id.clone(), harness),
    );
    let management = RuntimeManagementService::new_with_harness_config(
        server.clone(),
        Some(opened),
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        approval,
        config,
    );
    AppServerConnection::new(server).with_runtime_services(
        RuntimeServices::new(
            management,
            ThreadSettingsService::new(),
            ThreadGoalRequestProcessor::new(root, crate::goal_service::GoalLimits::default()),
        )
        .unwrap(),
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

#[tokio::test]
async fn scripted_session_reads_large_shell_output_after_compaction_and_restart() {
    let root = rpc_root("tool-output-recovery");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let thread_id = opened.store.thread_id().to_string();
    let session_path = opened.store.path().to_path_buf();
    let mut connection = tool_output_session_connection(
        ToolOutputRecoveryModel {
            mode: ToolOutputRecoveryMode::InitialTurn,
        },
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "tool-output-recovery-initial").await;

    let start = rpc_result(
        &mut connection,
        session_turn_start_request(2, &thread_id, "inspect long Shell output", None),
    )
    .await;
    assert_eq!(start["value"]["status"], "started");
    let first_turn_id = start["value"]["turn_id"].as_str().unwrap().to_string();
    let (mut saw_compaction, mut saw_truncated_shell, mut saw_timing, mut saw_usage) =
        (false, false, false, false);
    loop {
        let event = next_turn_event(&mut connection).await.event;
        match event {
            mini_agent_protocol::Event::ContextCompactionFinished { .. } => {
                saw_compaction = true;
            }
            mini_agent_protocol::Event::ToolFinished {
                name,
                truncated: true,
                ..
            } if name == "shell" => saw_truncated_shell = true,
            mini_agent_protocol::Event::ModelResponded {
                model_timing: Some(timing),
                usage: Some(usage),
                ..
            } if timing.ttft_ms.is_some() => {
                assert_eq!(usage.input_tokens, 120);
                assert_eq!(usage.cached_input_tokens, Some(60));
                assert_eq!(
                    usage.cached_input_tokens.unwrap() as f64 / usage.input_tokens as f64,
                    0.5
                );
                saw_timing = true;
                saw_usage = true;
            }
            mini_agent_protocol::Event::TurnFinished { .. } => break,
            _ => {}
        }
    }
    assert!(
        saw_compaction,
        "long output should trigger context compaction"
    );
    assert!(
        saw_truncated_shell,
        "the bounded Shell notice must stay marked truncated"
    );
    assert!(saw_timing, "streamed final text should include TTFT");
    assert!(
        saw_usage,
        "scripted Provider usage should remain observable"
    );

    let log = std::fs::read_to_string(&session_path).unwrap();
    let records = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let artifact = records
        .iter()
        .find(|record| record["kind"] == "result_stored")
        .expect("large Shell output should be persisted as a Session artifact");
    assert_eq!(artifact["storage"], "sidecar");
    assert_eq!(artifact["metadata"]["kind"], "tool_output");
    assert!(artifact.get("content").is_none());
    let turn_record = records
        .iter()
        .find(|record| record["kind"] == "turn_started" && record["turn_id"] == first_turn_id)
        .expect("completed Turn should be persisted");
    assert!(turn_record["presentation"]["modelTiming"]["ttftMs"].is_number());

    let first_turn = rpc_call(
        &mut connection,
        3,
        METHOD_TURN_READ,
        serde_json::json!({"turnId": first_turn_id}),
    )
    .await;
    assert_eq!(
        first_turn["value"]["finalText"],
        "First bounded page read before restart."
    );
    connection.shutdown().await.unwrap();

    let resumed = SessionStore::open(&root, SessionStoreRequest::Resume(session_id)).unwrap();
    let mut restarted = tool_output_session_connection(
        ToolOutputRecoveryModel {
            mode: ToolOutputRecoveryMode::ResumedSession,
        },
        root.clone(),
        resumed,
    );
    initialize_connection(&mut restarted, "tool-output-recovery-resumed").await;
    let resumed_start = rpc_result(
        &mut restarted,
        session_turn_start_request(2, &thread_id, "continue reading the output", None),
    )
    .await;
    assert_eq!(resumed_start["value"]["status"], "started");
    let resumed_turn_id = resumed_start["value"]["turn_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_turn_finished_id(&mut restarted, &resumed_turn_id).await;
    let resumed_turn = rpc_call(
        &mut restarted,
        3,
        METHOD_TURN_READ,
        serde_json::json!({"turnId": resumed_turn_id}),
    )
    .await;
    assert_eq!(
        resumed_turn["value"]["finalText"], "Second bounded page read after restart.",
        "resumed Turn response: {resumed_turn}"
    );

    restarted.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn force_kill_child_process_fixture() {
    let (Some(root), Some(marker)) = (
        std::env::var_os("MINI_AGENT_CRASH_FIXTURE_ROOT"),
        std::env::var_os("MINI_AGENT_CRASH_FIXTURE_MARKER"),
    ) else {
        return;
    };
    let root = PathBuf::from(root);
    let marker = PathBuf::from(marker);
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let thread_id = opened.store.thread_id().to_string();
    let session_path = opened.store.path().to_path_buf();
    let mut connection = tool_output_session_connection(
        ToolOutputRecoveryModel {
            mode: ToolOutputRecoveryMode::InitialTurn,
        },
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "forced-crash-child").await;
    let start = rpc_result(
        &mut connection,
        session_turn_start_request(
            2,
            &thread_id,
            "persist large output before process exit",
            None,
        ),
    )
    .await;
    assert_eq!(start["value"]["status"], "started");
    let turn_id = start["value"]["turn_id"].as_str().unwrap().to_string();
    wait_for_turn_finished_id(&mut connection, &turn_id).await;
    std::fs::write(
        marker,
        serde_json::to_vec(&serde_json::json!({
            "session_id": session_id,
            "thread_id": thread_id,
            "turn_id": turn_id,
            "session_path": session_path,
        }))
        .unwrap(),
    )
    .unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn app_server_session_recovers_after_forced_process_termination() {
    let root = rpc_root("forced-process-recovery");
    let marker = root.join("child-ready.json");
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(executable)
        .arg("--exact")
        .arg("json_rpc::tests::force_kill_child_process_fixture")
        .arg("--nocapture")
        .env("MINI_AGENT_CRASH_FIXTURE_ROOT", &root)
        .env("MINI_AGENT_CRASH_FIXTURE_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let marker_ready = tokio::time::timeout(Duration::from_secs(45), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    if !marker_ready {
        let _ = child.kill();
        let _ = child.wait();
        panic!("scripted App Server child did not persist its Session checkpoint");
    }
    child.kill().unwrap();
    let exit = child.wait().unwrap();
    assert!(
        !exit.success(),
        "child process must have been force terminated"
    );

    let child_state: Value = serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
    let session_id = child_state["session_id"].as_str().unwrap().to_string();
    let thread_id = child_state["thread_id"].as_str().unwrap().to_string();
    let turn_id = child_state["turn_id"].as_str().unwrap().to_string();
    let session_path = PathBuf::from(child_state["session_path"].as_str().unwrap());
    let records = std::fs::read_to_string(&session_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let turn_record = records
        .iter()
        .find(|record| record["kind"] == "turn_started" && record["turn_id"] == turn_id)
        .expect("the completed turn should have a durable Journal projection");
    assert!(turn_record["presentation"]["modelTiming"]["ttftMs"].is_number());
    assert!(
        turn_record["presentation"]["contextUsage"]["usageTotals"]["requestCount"]
            .as_u64()
            .is_some_and(|count| count >= 2)
    );
    assert!(
        turn_record["presentation"]["contextUsage"]["usageTotals"]["cachedInputTokens"]
            .as_u64()
            .is_some_and(|count| count >= 120)
    );
    assert!(records.iter().any(|record| {
        record["kind"] == "turn_settled"
            && record["turn_id"] == turn_id
            && record["status"] == "completed"
    }));

    let opened = SessionStore::open(&root, SessionStoreRequest::Resume(session_id)).unwrap();
    assert!(opened.store.items().iter().any(|item| {
        item.turn_id.as_deref() == Some(turn_id.as_str())
            && matches!(
                &item.message,
                Message::Tool { name, content, .. }
                    if name == "read_tool_output" && content.contains("next_cursor:")
            )
    }));
    assert!(opened.state.messages().iter().any(|message| matches!(
        message,
        Message::Tool { name, content, is_error: false, .. }
            if name == "shell" && content.contains("Full output handle:")
    )));
    assert!(opened.state.messages().iter().any(|message| matches!(
        message,
        Message::Tool { name, content, is_error: false, .. }
            if name == "read_tool_output" && content.contains("next_cursor:")
    )));

    let mut restarted = tool_output_session_connection(
        ToolOutputRecoveryModel {
            mode: ToolOutputRecoveryMode::ResumedSession,
        },
        root.clone(),
        opened,
    );
    initialize_connection(&mut restarted, "forced-crash-resume").await;
    let checkpoint = rpc_call(
        &mut restarted,
        2,
        METHOD_THREAD_READ,
        serde_json::json!({"threadId": thread_id}),
    )
    .await;
    assert!(
        checkpoint["value"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Full output handle:"))
            })
    );
    let items = rpc_call(
        &mut restarted,
        3,
        METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({"threadId": thread_id, "turnId": turn_id, "limit": 128}),
    )
    .await;
    assert!(
        items["value"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["item"]["type"] == "toolCall" && entry["item"]["name"] == "read_tool_output"
            })
    );

    let resumed_start = rpc_result(
        &mut restarted,
        session_turn_start_request(
            4,
            &thread_id,
            "read the next output page after restart",
            None,
        ),
    )
    .await;
    let resumed_turn_id = resumed_start["value"]["turn_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_turn_finished_id(&mut restarted, &resumed_turn_id).await;
    let resumed_turn = rpc_call(
        &mut restarted,
        5,
        METHOD_TURN_READ,
        serde_json::json!({"turnId": resumed_turn_id}),
    )
    .await;
    assert_eq!(
        resumed_turn["value"]["finalText"],
        "Second bounded page read after restart."
    );
    restarted.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

struct WebFetchFixtureTool {
    executed: Arc<AtomicBool>,
}

impl ToolHandler for WebFetchFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_string(),
            description: "Fetch the scenario fixture page.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"],
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for WebFetchFixtureTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        self.executed.store(true, Ordering::SeqCst);
        Ok("fixture page body".to_string())
    }
}

struct WebFetchVisibilityModel {
    tool_executed: Arc<AtomicBool>,
}

impl Model for WebFetchVisibilityModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        assert!(request.tools.iter().any(|tool| tool.name == "web_fetch"));
        if self.tool_executed.load(Ordering::SeqCst) {
            assert!(request.messages.iter().any(|message| {
                matches!(
                    message,
                    Message::Tool {
                        name,
                        outcome: Some(ToolExecutionStatus::Completed),
                        ..
                    } if name == "web_fetch"
                )
            }));
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "fixture page was read".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "web-fetch-visibility-call".to_string(),
                name: "web_fetch".to_string(),
                arguments: serde_json::json!({"url": "https://example.com/article"}),
            }],
            usage: None,
        })
    }
}

enum ScenarioModel {
    ShellApproval,
    BackgroundShell,
    StepLimit,
    ResumeAfterStepLimit(Arc<AtomicUsize>),
    Budget,
    Counting(Arc<AtomicUsize>),
    Timeout(Arc<tokio::sync::Notify>),
}

impl Model for ScenarioModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        match self {
            Self::BackgroundShell => {
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
                        text: "background shell started".to_string(),
                        tool_calls: Vec::new(),
                        usage: None,
                    });
                }
                let shell = request
                    .tools
                    .iter()
                    .find(|tool| tool.name == "shell")
                    .expect("workspace composition should expose Shell");
                let scheduled = request
                    .tools
                    .iter()
                    .find(|tool| tool.name == "scheduled_task")
                    .expect("workspace composition should expose delayed markers");
                assert!(shell.description.contains("mode=background/action=start"));
                assert!(shell.description.contains("Do not use scheduled_task"));
                assert_eq!(
                    shell.parameters["oneOf"][1]["required"],
                    serde_json::json!(["mode", "action", "task_id", "command"])
                );
                assert_eq!(
                    shell.parameters["oneOf"][2]["required"],
                    serde_json::json!(["mode", "action", "task_id"])
                );
                assert!(scheduled.description.contains("wake or resume a Thread"));
                assert_eq!(
                    scheduled.parameters["oneOf"][0]["required"],
                    serde_json::json!(["action", "task_id", "delay_seconds"])
                );
                Ok(ModelResponse {
                    reasoning: String::new(),
                    text: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "background-shell-call".to_string(),
                        name: "shell".to_string(),
                        arguments: serde_json::json!({
                            "mode": "background",
                            "action": "start",
                            "task_id": "scenario-task",
                            "command": background_shell_command(),
                        }),
                    }],
                    usage: None,
                })
            }
            Self::ShellApproval => {
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
            Self::StepLimit => Ok(ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "step-limit-call".to_string(),
                    name: "missing_tool".to_string(),
                    arguments: serde_json::json!({}),
                }],
                usage: None,
            }),
            Self::ResumeAfterStepLimit(calls) => {
                let step = calls.fetch_add(1, Ordering::SeqCst);
                if step < 8 {
                    Ok(ModelResponse {
                        reasoning: String::new(),
                        text: String::new(),
                        tool_calls: vec![ToolCall {
                            id: format!("step-limit-call-{step}"),
                            name: "missing_tool".to_string(),
                            arguments: serde_json::json!({}),
                        }],
                        usage: None,
                    })
                } else {
                    Ok(ModelResponse {
                        reasoning: String::new(),
                        text: "continued after the manual step limit".to_string(),
                        tool_calls: Vec::new(),
                        usage: None,
                    })
                }
            }
            Self::Budget => Ok(ModelResponse {
                reasoning: String::new(),
                text: "budget reached".to_string(),
                tool_calls: Vec::new(),
                usage: Some(ModelUsage {
                    input_tokens: 3,
                    cached_input_tokens: Some(0),
                    output_tokens: 2,
                }),
            }),
            Self::Counting(calls) => {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(ModelResponse {
                    reasoning: String::new(),
                    text: "replayed".to_string(),
                    tool_calls: Vec::new(),
                    usage: None,
                })
            }
            Self::Timeout(release) => {
                release.notified().await;
                Ok(ModelResponse {
                    reasoning: String::new(),
                    text: "released".to_string(),
                    tool_calls: Vec::new(),
                    usage: None,
                })
            }
        }
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

fn background_shell_command() -> &'static str {
    #[cfg(windows)]
    {
        "Write-Output background-started; Start-Sleep -Seconds 30"
    }
    #[cfg(not(windows))]
    {
        "printf background-started; sleep 30"
    }
}

fn managed_connection(name: &str) -> (AppServerConnection<DoneModel>, std::path::PathBuf) {
    managed_connection_with(DoneModel, name, crate::goal_service::GoalLimits::default())
}

fn managed_connection_with<M: Model + Send + 'static>(
    model: M,
    name: &str,
    goal_limits: crate::goal_service::GoalLimits,
) -> (AppServerConnection<M>, std::path::PathBuf) {
    let root = rpc_root(name);
    managed_connection_at(model, root, goal_limits)
}

fn managed_connection_at<M: Model + Send + 'static>(
    model: M,
    root: std::path::PathBuf,
    goal_limits: crate::goal_service::GoalLimits,
) -> (AppServerConnection<M>, std::path::PathBuf) {
    let server = crate::tests::server(model);
    let connection = managed_connection_at_with_server(
        server,
        root.clone(),
        goal_limits,
        mini_agent_host::BuiltinToolSelection::default(),
    );
    (connection, root)
}

fn managed_connection_at_with_server<M: Model + Send + 'static>(
    server: AppServer<M>,
    root: std::path::PathBuf,
    goal_limits: crate::goal_service::GoalLimits,
    initial_builtin_tools: mini_agent_host::BuiltinToolSelection,
) -> AppServerConnection<M> {
    let thread_settings = ThreadSettingsService::new();
    let goals = ThreadGoalRequestProcessor::new(root.clone(), goal_limits);
    let management = RuntimeManagementService::new(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        ApprovalController::with_preset(ApprovalPolicy::Automatic, Default::default()),
    )
    .with_initial_builtin_tools(initial_builtin_tools);
    AppServerConnection::new(server)
        .with_runtime_services(RuntimeServices::new(management, thread_settings, goals).unwrap())
}

fn managed_connection_with_session<M: Model + Send + 'static>(
    model: M,
    root: std::path::PathBuf,
    opened: mini_agent_capabilities::OpenedSession,
) -> AppServerConnection<M> {
    let thread_id = ThreadId::new(opened.store.thread_id().to_string());
    let server = AppServer::new(
        ThreadStart::new(thread_id.clone()),
        Thread::new(
            thread_id,
            Harness::new(model, ToolRouter::default(), HarnessConfig::default()),
        ),
    );
    let thread_settings = ThreadSettingsService::new();
    let goals =
        ThreadGoalRequestProcessor::new(root.clone(), crate::goal_service::GoalLimits::default());
    let management = RuntimeManagementService::new(
        server.clone(),
        Some(opened),
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        ApprovalController::with_preset(ApprovalPolicy::Automatic, Default::default()),
    );
    AppServerConnection::new(server)
        .with_runtime_services(RuntimeServices::new(management, thread_settings, goals).unwrap())
}

fn forked_child_session(
    root: &std::path::Path,
    thread_id: &str,
    mut operation: mini_agent_capabilities::SessionOperation,
) -> (String, String, mini_agent_capabilities::OpenedSession) {
    let parent = SessionStore::open(root, SessionStoreRequest::New).unwrap();
    let parent_thread_id = parent.store.thread_id().to_string();
    operation.parent_thread_id = Some(parent_thread_id.clone());
    let child = SessionStore::fork_from_checkpoint_with_operation(
        root,
        parent.store.session_id(),
        parent.store.checkpoint_seq(),
        thread_id,
        &[],
        mini_agent_capabilities::SessionForkMetadata {
            context_policy: "exact".to_string(),
            context_before_bytes: 0,
            context_after_bytes: 0,
            compacted: false,
            method: "exact".to_string(),
        },
        Some(operation),
    )
    .unwrap();
    let session_id = child.session_id;
    drop(parent);
    let child = SessionStore::open(root, SessionStoreRequest::Resume(session_id.clone())).unwrap();
    (parent_thread_id, session_id, child)
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

    let result = rpc_call(
        &mut connection,
        2,
        METHOD_SESSION_INFO,
        serde_json::json!({}),
    )
    .await;
    assert!(result["value"].is_null());
    assert_eq!(
        result["actionId"], 1,
        "session/info is the first admitted runtime action"
    );
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);

    let result = rpc_call(
        &mut connection,
        3,
        METHOD_WORLD_STATE,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["value"]["workspace"], root.display().to_string());
    assert_eq!(result["actionId"], 2);
    assert_eq!(result["actionSequence"], 2);
    assert_eq!(result["stateRevision"], 0);

    let result = rpc_call(&mut connection, 4, METHOD_MCP_STATUS, serde_json::json!({})).await;
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
                    access: SecurityPreset::Default,
                    policy: ApprovalPolicy::Interactive,
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
                    access: SecurityPreset::FullMachine,
                    policy: ApprovalPolicy::Automatic,
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

    let result = start_turn(&mut connection, 3, "hello").await;
    assert_eq!(result["value"]["status"], "started");
    assert_eq!(result["actionId"], 1);
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);
}

#[tokio::test]
async fn exposes_runtime_status_and_replays_bounded_turn_events() {
    let (mut connection, root) = managed_connection("runtime-observability");
    initialize_connection(&mut connection, "runtime-observability-test").await;

    let status = rpc_call(
        &mut connection,
        2,
        METHOD_RUNTIME_STATUS,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(status["phase"], "idle");
    assert_eq!(status["threadId"], "thread-1");
    assert!(status["timestampMs"].as_u64().unwrap() > 0);

    let _ = start_turn(&mut connection, 3, "observe this run").await;
    wait_for_turn_finished(&mut connection).await;
    let replay = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_EVENTS,
        serde_json::json!({
            "threadId": "thread-1",
            "afterSequence": 0,
            "limit": 128
        }),
    )
    .await;
    assert_eq!(replay["hasGap"], false);
    assert!(!replay["data"].as_array().unwrap().is_empty());
    assert!(replay["nextCursor"].as_u64().unwrap() >= 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rejects_removed_workflow_state_method() {
    let (mut connection, root) = managed_connection("workflow-rpc");
    initialize_connection(&mut connection, "workflow-test").await;

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            "workflow/state",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.error.unwrap().code, -32601);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn broadcasts_thread_settings_updates_with_action_revision() {
    let (mut connection, root) = managed_connection("thread-settings-notification");
    let mut observer = AppServerConnection::new(connection.server.clone())
        .with_runtime_services(connection.runtime.as_ref().unwrap().clone());
    initialize_connection(&mut connection, "thread-settings-test").await;

    let response = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_SETTINGS_UPDATE,
        serde_json::json!({
            "threadId": "thread-1",
            "collaborationMode": {"mode": "plan"},
            "builtinTools": ["shell", "read_file"],
            "continuationMode": "continuous"
        }),
    )
    .await;
    let response_revision = response["stateRevision"].clone();

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
    assert_eq!(params["continuationMode"], "continuous");
    assert_eq!(params["stateRevision"], response_revision);
    let observer_notification = loop {
        let notification = observer.next_notification().await.unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_SETTINGS_UPDATED {
            break notification;
        }
    };
    assert_eq!(
        observer_notification.params.unwrap()["stateRevision"],
        response_revision
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn search_enabled_fetch_selection_is_reported_and_model_visible() {
    let root = rpc_root("search-enabled-fetch-selection");
    let executed = Arc::new(AtomicBool::new(false));
    let selection = mini_agent_host::BuiltinToolSelection::all();
    let mut harness = crate::tests::harness(WebFetchVisibilityModel {
        tool_executed: executed.clone(),
    });
    harness.extend_tools(vec![Box::new(WebFetchFixtureTool {
        executed: executed.clone(),
    })]);
    harness.set_hidden_tools(selection.hidden_names());
    let thread_id = ThreadId::new("thread-1");
    let server = AppServer::new(
        ThreadStart::new(thread_id.clone()),
        Thread::new(thread_id, harness),
    );
    let mut connection = managed_connection_at_with_server(
        server,
        root.clone(),
        crate::goal_service::GoalLimits::default(),
        selection,
    );
    initialize_connection(&mut connection, "search-enabled-fetch-test").await;

    let settings = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_SETTINGS_UPDATE,
        serde_json::json!({
            "threadId": "thread-1",
            "collaborationMode": {"mode": "default"}
        }),
    )
    .await;
    assert!(
        settings["value"]["builtinTools"]
            .as_array()
            .unwrap()
            .contains(&Value::String("web_fetch".to_string()))
    );

    let _ = start_turn(&mut connection, 3, "Read the page").await;
    wait_for_turn_finished(&mut connection).await;
    assert!(executed.load(Ordering::SeqCst));

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn persists_and_restores_thread_continuation_through_app_server_restart() {
    let root = rpc_root("thread-settings-session");
    let mut opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let thread_id = opened.store.thread_id().to_string();
    opened.store.set_continuation_mode("continuous").unwrap();

    let mut connection = managed_connection_with_session(DoneModel, root.clone(), opened);
    initialize_connection(&mut connection, "thread-settings-session-test").await;

    connection.shutdown().await.unwrap();
    let resumed = loop {
        match SessionStore::open(&root, SessionStoreRequest::Resume(session_id.clone())) {
            Ok(opened) => break opened,
            Err(error) if error.contains("locked") => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("unexpected session error: {error}"),
        }
    };
    assert_eq!(resumed.store.continuation_mode(), Some("continuous"));

    let mut restarted = managed_connection_with_session(DoneModel, root.clone(), resumed);
    initialize_connection(&mut restarted, "thread-settings-session-restart-test").await;
    let response = rpc_call(
        &mut restarted,
        2,
        METHOD_THREAD_SETTINGS_UPDATE,
        serde_json::json!({
            "threadId": thread_id,
            "collaborationMode": {"mode": "default"}
        }),
    )
    .await;
    assert_eq!(response["value"]["continuationMode"], "continuous");
    restarted.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn session_fork_retry_reuses_persisted_result_before_core_preparation() {
    let root = rpc_root("session-fork-retry");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let source_thread_id = opened.store.thread_id().to_string();
    let model_calls = Arc::new(AtomicUsize::new(0));
    let mut connection = managed_connection_with_session(
        ScenarioModel::Counting(model_calls.clone()),
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "session-fork-retry-test").await;

    let _ = rpc_result(
        &mut connection,
        JsonRpcRequest::request(
            2,
            METHOD_TURN_START,
            serde_json::json!(TurnStartParams {
                thread_id: ThreadId::new(source_thread_id.clone()),
                input: TurnInput::new(TurnInputMode::Start, "seed fork checkpoint"),
                operation_id: None,
                operation_attempt: None,
                operation_attempt_kind: None,
                operation_group_id: None,
                execution_mode: None,
                group_sequence: None,
                turn_source: None,
            }),
        ),
    )
    .await;
    wait_for_turn_finished(&mut connection).await;
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);

    let params = serde_json::json!({
        "sourceThreadId": source_thread_id,
        "newThreadId": "forked-rpc-thread",
        "contextPolicy": "exact",
        "operationId": "child:forked-rpc-thread",
        "operationAttempt": 1
    });
    let first = rpc_call(&mut connection, 3, METHOD_SESSION_FORK, params.clone()).await;
    let retry = rpc_call(&mut connection, 4, METHOD_SESSION_FORK, params).await;
    assert_eq!(first["value"]["method"], "exact");
    assert_eq!(retry["value"]["sessionId"], first["value"]["sessionId"]);
    assert_eq!(
        retry["value"]["contextBeforeBytes"],
        first["value"]["contextBeforeBytes"]
    );
    let child_session_id = first["value"]["sessionId"].as_str().unwrap();
    let (_, child_path) =
        mini_agent_capabilities::resolve_session_file(&root, child_session_id).unwrap();
    let child_session = std::fs::read_to_string(child_path).unwrap();
    assert!(child_session.contains("\"operation_id\":\"child:forked-rpc-thread\""));
    assert!(child_session.contains("\"status\":\"queued\""));
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);

    let conflict = connection
        .handle_request(JsonRpcRequest::request(
            5,
            METHOD_SESSION_FORK,
            serde_json::json!({
                "sourceThreadId": source_thread_id,
                "newThreadId": "forked-rpc-thread",
                "contextPolicy": "compact"
            }),
        ))
        .await
        .unwrap();
    let error = conflict
        .error
        .expect("policy conflict should be an RPC error");
    assert_eq!(error.code, SESSION_FORK_CONFLICT_CODE);
    let data = error.data.unwrap();
    assert_eq!(data["kind"], "contextPolicy");
    assert_eq!(data["childThreadId"], "forked-rpc-thread");
    assert_eq!(data["requestedContextPolicy"], "compact");
    assert_eq!(data["existingContextPolicy"], "exact");
    assert_eq!(data["actionId"], 4);
    assert_eq!(data["actionSequence"], 4);
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn fork_child_thread_items_do_not_project_inherited_checkpoint_messages() {
    let root = rpc_root("fork-child-items");
    let mut parent = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let parent_session_id = parent.store.session_id().to_string();
    let parent_messages = vec![
        Message::User {
            text: "parent-only prompt".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "parent-only answer".to_string(),
            tool_calls: Vec::new(),
        },
    ];
    parent
        .store
        .record_turn_with_id(
            "parent-turn",
            mini_agent_capabilities::TurnCommit {
                started_at_ms: 1,
                prompt: "parent-only prompt",
                status: mini_agent_capabilities::TurnStatus::Completed,
                steps: 1,
                error: None,
                messages: &parent_messages,
                tool_arguments: &[],
                presentation: None,
                checkpoint: &parent_messages,
            },
        )
        .unwrap();
    let fork = SessionStore::fork_from_checkpoint(
        &root,
        &parent_session_id,
        parent.store.checkpoint_seq(),
        "fork-child-thread",
        &parent_messages,
        mini_agent_capabilities::SessionForkMetadata {
            context_policy: "exact".to_string(),
            context_before_bytes: 0,
            context_after_bytes: 0,
            compacted: false,
            method: "exact".to_string(),
        },
    )
    .unwrap();
    let child_session_id = fork.session_id;
    drop(parent);

    let child = SessionStore::open(&root, SessionStoreRequest::Resume(child_session_id)).unwrap();
    assert!(child.store.is_forked());
    assert!(child.store.items().is_empty());
    assert!(
        child.state.messages().iter().any(
            |message| matches!(message, Message::User { text } if text == "parent-only prompt")
        )
    );
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), child);
    initialize_connection(&mut connection, "fork-child-items-test").await;
    let items = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({"threadId": "fork-child-thread"}),
    )
    .await;
    assert!(items["value"]["data"].as_array().unwrap().is_empty());

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn child_task_report_rpc_checks_attempt_and_persists_idempotently() {
    let root = rpc_root("child-task-report");
    let operation = mini_agent_capabilities::SessionOperation::new(
        "child:child-rpc-thread",
        "child_task",
        "queued",
    );
    let (parent_thread_id, child_session_id, mut child) =
        forked_child_session(&root, "child-rpc-thread", operation);
    child
        .store
        .record_operation(mini_agent_capabilities::SessionOperation::new(
            "child:child-rpc-thread",
            "child_task",
            "running",
        ))
        .unwrap();
    let child_thread_id = child.store.thread_id().to_string();
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), child);
    initialize_connection(&mut connection, "child-task-report-test").await;

    let stale = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_CHILD_TASK,
            serde_json::json!({
                "threadId": child_thread_id,
                "parentThreadId": parent_thread_id,
                "operationId": "child:child-rpc-thread",
                "attempt": 2,
                "action": "report",
                "reportId": "report-call",
                "report": "Checking the report path."
            }),
        ))
        .await
        .unwrap();
    assert!(stale.error.is_some());
    let params = serde_json::json!({
        "threadId": child_thread_id,
        "parentThreadId": parent_thread_id,
        "operationId": "child:child-rpc-thread",
        "attempt": 1,
        "action": "report",
        "reportId": "report-call",
        "report": "Checking the report path."
    });
    let first = rpc_call(&mut connection, 3, METHOD_CHILD_TASK, params.clone()).await;
    let retry = rpc_call(&mut connection, 4, METHOD_CHILD_TASK, params).await;
    assert_eq!(first["value"]["cursor"], retry["value"]["cursor"]);
    assert_eq!(first["value"]["attempt"], 1);

    connection.shutdown().await.unwrap();
    let (_, child_path) =
        mini_agent_capabilities::resolve_session_file(&root, &child_session_id).unwrap();
    let child_log = std::fs::read_to_string(child_path).unwrap();
    assert_eq!(child_log.matches("\"kind\":\"child_report\"").count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn active_child_follow_up_rpc_is_idempotent_and_survives_restart() {
    let root = rpc_root("active-child-follow-up");
    let mut operation = mini_agent_capabilities::SessionOperation::new(
        "child:active-follow-up",
        "child_task",
        "running",
    );
    operation.turn_id = Some("turn-active".to_string());
    operation.prompt = Some("Initial task".to_string());
    operation.attempt_kind = Some(mini_agent_protocol::ChildTaskAttemptKind::Initial);
    let (parent_thread_id, child_session_id, child) =
        forked_child_session(&root, "active-follow-up-child", operation);
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), child);
    initialize_connection(&mut connection, "active-child-follow-up-test").await;
    let params = serde_json::json!({
        "threadId":"active-follow-up-child",
        "parentThreadId":parent_thread_id,
        "operationId":"child:active-follow-up",
        "attempt":1,
        "action":"queue_follow_up",
        "requestId":"follow-up-active-1",
        "prompt":"Review the first result"
    });
    let first = rpc_call(&mut connection, 2, METHOD_CHILD_TASK, params.clone()).await;
    let duplicate = rpc_call(&mut connection, 3, METHOD_CHILD_TASK, params).await;
    assert_eq!(first["value"]["status"], "queued");
    assert_eq!(first["value"]["attempt"], 1);
    assert_eq!(duplicate["value"]["duplicate"], true);
    assert_eq!(duplicate["value"]["status"], "queued");

    let overflow = connection
        .handle_request(JsonRpcRequest::request(
            4,
            METHOD_CHILD_TASK,
            serde_json::json!({
                "threadId":"active-follow-up-child",
                "parentThreadId":parent_thread_id,
                "operationId":"child:active-follow-up",
                "attempt":1,
                "action":"queue_follow_up",
                "requestId":"follow-up-active-2",
                "prompt":"A second follow-up"
            }),
        ))
        .await
        .unwrap();
    assert!(overflow.error.is_some());
    connection.shutdown().await.unwrap();

    let mut resumed =
        SessionStore::open(&root, SessionStoreRequest::Resume(child_session_id.clone())).unwrap();
    let operation = resumed
        .store
        .operation("child:active-follow-up")
        .unwrap()
        .unwrap();
    assert_eq!(operation.status, "running");
    assert_eq!(operation.attempt, 1);
    assert_eq!(operation.prompt.as_deref(), Some("Initial task"));
    let session_log = std::fs::read_to_string(resumed.store.path()).unwrap();
    assert!(session_log.contains("\"request_status\":\"accepted\""));
    assert!(session_log.contains("Review the first result"));
    let mut completed = mini_agent_capabilities::SessionOperation::new(
        "child:active-follow-up",
        "child_task",
        "completed",
    );
    completed.turn_id = Some("turn-active".to_string());
    resumed.store.record_operation(completed).unwrap();
    let promoted = resumed
        .store
        .operation("child:active-follow-up")
        .unwrap()
        .unwrap();
    assert_eq!(promoted.status, "queued");
    assert_eq!(promoted.attempt, 2);
    assert_eq!(promoted.prompt.as_deref(), Some("Review the first result"));
    drop(resumed);
    let recovered =
        SessionStore::open(&root, SessionStoreRequest::Resume(child_session_id)).unwrap();
    let promoted = recovered
        .store
        .operation("child:active-follow-up")
        .unwrap()
        .unwrap();
    assert_eq!(promoted.status, "queued");
    assert_eq!(promoted.attempt, 2);
    drop(recovered);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn completed_child_follow_up_starts_a_new_turn_on_the_same_session() {
    let root = rpc_root("child-follow-up-round");
    let mut operation = mini_agent_capabilities::SessionOperation::new(
        "child:follow-up-rpc",
        "child_task",
        "completed",
    );
    operation.prompt = Some("Initial bounded task".to_string());
    operation.attempt_kind = Some(mini_agent_protocol::ChildTaskAttemptKind::Initial);
    operation.turn_id = Some("turn-initial".to_string());
    let (parent_thread_id, child_session_id, child) =
        forked_child_session(&root, "child-follow-up-rpc-thread", operation);
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), child);
    initialize_connection(&mut connection, "child-follow-up-round-test").await;

    let stale_steer = rpc_call(
        &mut connection,
        2,
        METHOD_TURN_STEER,
        serde_json::json!({
            "threadId": "child-follow-up-rpc-thread",
            "turnId": "turn-initial",
            "requestId": "parent-control-event-2",
            "text": "Review the initial result and fix the requested issue."
        }),
    )
    .await;
    assert_eq!(stale_steer["value"]["status"], "not_submitted");

    let follow_up = rpc_call(
        &mut connection,
        3,
        METHOD_CHILD_TASK,
        serde_json::json!({
            "threadId": "child-follow-up-rpc-thread",
            "parentThreadId": parent_thread_id,
            "operationId": "child:follow-up-rpc",
            "attempt": 1,
            "action": "queue_follow_up",
            "requestId": "parent-control-event-2",
            "prompt": "Review the initial result and fix the requested issue."
        }),
    )
    .await;
    assert_eq!(follow_up["value"]["action"], "queue_follow_up");
    assert_eq!(follow_up["value"]["requestAction"], "queue_follow_up");
    assert_eq!(follow_up["value"]["status"], "queued");
    assert_eq!(follow_up["value"]["attempt"], 2);
    assert_eq!(follow_up["value"]["attemptKind"], "follow_up");
    assert!(follow_up["value"]["turnId"].is_null());

    let changed_prompt = rpc_call(
        &mut connection,
        3,
        METHOD_TURN_START,
        serde_json::json!({
            "threadId": "child-follow-up-rpc-thread",
            "input": {"mode": "start", "text": "Overwrite the queued review prompt."},
            "operationId": "child:follow-up-rpc",
            "operationAttempt": 2,
            "operationAttemptKind": "follow_up"
        }),
    )
    .await;
    assert_eq!(changed_prompt["value"]["status"], "not_submitted");

    let second = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_START,
        serde_json::json!({
            "threadId": "child-follow-up-rpc-thread",
            "input": {"mode": "start", "text": "Review the initial result and fix the requested issue."},
            "operationId": "child:follow-up-rpc",
            "operationAttempt": 2,
            "operationAttemptKind": "follow_up"
        }),
    )
    .await;
    assert_eq!(second["value"]["status"], "started");
    wait_for_turn_finished(&mut connection).await;
    let unknown_operation = rpc_call(
        &mut connection,
        5,
        METHOD_TURN_START,
        serde_json::json!({
            "threadId": "child-follow-up-rpc-thread",
            "input": {"mode": "start", "text": "Unowned task"},
            "operationId": "child:unknown",
            "operationAttempt": 1,
            "operationAttemptKind": "initial"
        }),
    )
    .await;
    assert_eq!(unknown_operation["value"]["status"], "not_submitted");
    connection.shutdown().await.unwrap();

    let resumed = SessionStore::open(&root, SessionStoreRequest::Resume(child_session_id)).unwrap();
    let operation = resumed
        .store
        .operation("child:follow-up-rpc")
        .unwrap()
        .unwrap();
    assert_eq!(operation.attempt, 2);
    assert_eq!(operation.status, "completed");
    assert_eq!(
        operation.attempt_kind,
        Some(mini_agent_protocol::ChildTaskAttemptKind::FollowUp)
    );
    assert_eq!(
        operation.turn_id.as_deref(),
        Some("turn-child-follow-up-rpc-thread-1")
    );
    assert_eq!(resumed.store.thread_id(), "child-follow-up-rpc-thread");
    drop(resumed);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exact_session_fork_can_prepare_from_an_active_parent_turn() {
    let root = rpc_root("active-session-fork");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let source_thread_id = opened.store.thread_id().to_string();
    let initial_checkpoint_seq = opened.store.checkpoint_seq();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut connection = managed_connection_with_session(
        ScenarioModel::Timeout(release.clone()),
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "active-session-fork-test").await;

    let _ = rpc_result(
        &mut connection,
        JsonRpcRequest::request(
            2,
            METHOD_TURN_START,
            serde_json::json!(TurnStartParams {
                thread_id: ThreadId::new(source_thread_id.clone()),
                input: TurnInput::new(TurnInputMode::Start, "active parent"),
                operation_id: None,
                operation_attempt: None,
                operation_attempt_kind: None,
                operation_group_id: None,
                execution_mode: None,
                group_sequence: None,
                turn_source: None,
            }),
        ),
    )
    .await;
    loop {
        let event = next_turn_event(&mut connection).await;
        if matches!(event.event, mini_agent_protocol::Event::TurnStarted { .. }) {
            break;
        }
    }

    let fork = rpc_call(
        &mut connection,
        3,
        METHOD_SESSION_FORK,
        serde_json::json!({
            "sourceThreadId": source_thread_id,
            "newThreadId": "active-child-thread",
            "contextPolicy": "exact"
        }),
    )
    .await;
    assert_eq!(fork["value"]["threadId"], "active-child-thread");
    assert_eq!(fork["value"]["method"], "exact");
    assert_eq!(fork["value"]["parentCheckpointSeq"], initial_checkpoint_seq);

    release.notify_one();
    wait_for_turn_finished(&mut connection).await;
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rejects_thread_continuation_updates_while_goal_runtime_is_active() {
    let (mut connection, root) = managed_connection("goal-owns-continuation");
    initialize_connection(&mut connection, "goal-continuation-test").await;
    mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    )
    .set_goal("preserve Goal loop ownership", None)
    .unwrap();

    let response = connection
        .handle_request(JsonRpcRequest::request(
            2,
            METHOD_THREAD_SETTINGS_UPDATE,
            serde_json::json!({
                "threadId": "thread-1",
                "collaborationMode": {"mode": "default"},
                "continuationMode": "continuous"
            }),
        ))
        .await
        .unwrap();
    let error = response.error.expect("active Goal must own continuation");
    assert_eq!(error.code, -32000);
    assert!(
        error
            .message
            .contains("Goal Runtime owns continuation mode")
    );

    mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    )
    .clear_goal()
    .unwrap();
    let resumed_settings = rpc_call(
        &mut connection,
        3,
        METHOD_THREAD_SETTINGS_UPDATE,
        serde_json::json!({
            "threadId": "thread-1",
            "collaborationMode": {"mode": "default"},
            "continuationMode": "continuous"
        }),
    )
    .await;
    assert_eq!(resumed_settings["value"]["continuationMode"], "continuous");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preserves_cross_stream_notification_order() {
    let (mut connection, root) = managed_connection("cross-stream-order");
    initialize_connection(&mut connection, "cross-stream-test").await;
    let _ = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_SETTINGS_UPDATE,
        serde_json::json!({
            "threadId": "thread-1",
            "collaborationMode": {"mode": "plan"}
        }),
    )
    .await;
    let _ = set_goal(
        &mut connection,
        3,
        Some("preserve notification order"),
        None,
        None,
    )
    .await;

    assert_eq!(
        connection.next_notification().await.unwrap().method,
        mini_agent_app_server_protocol::METHOD_THREAD_SETTINGS_UPDATED
    );
    assert_eq!(
        connection.next_notification().await.unwrap().method,
        mini_agent_app_server_protocol::METHOD_PLAN_UPDATED
    );
    let mut methods = Vec::new();
    loop {
        let method = connection.next_notification().await.unwrap().method;
        methods.push(method.clone());
        if method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            break;
        }
    }
    assert!(
        methods
            .contains(&mini_agent_app_server_protocol::METHOD_GOAL_CONTINUATION_QUEUED.to_string())
    );
    wait_for_goal_status(&mut connection, "blocked").await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn emits_thread_item_lifecycle_notifications_on_the_ordered_stream() {
    let (mut connection, root) = managed_connection("thread-item-lifecycle");
    let initialized = connection
        .handle_request(initialize_request(1, "thread-item-lifecycle-test"))
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(initialized["capabilities"]["threadItemsList"], true);
    assert_eq!(
        initialized["capabilities"]["itemLifecycleNotifications"],
        true
    );
    let _ = start_turn(&mut connection, 2, "hello").await;

    let mut methods = Vec::new();
    loop {
        let notification = connection.next_notification().await.unwrap();
        methods.push(notification.method.clone());
        if notification.method == METHOD_TURN_EVENT {
            let params = notification.params.unwrap();
            let event: TurnEventNotification = serde_json::from_value(params).unwrap();
            if matches!(event.event, mini_agent_protocol::Event::TurnFinished { .. }) {
                break;
            }
        }
    }
    assert!(methods.contains(&mini_agent_app_server_protocol::METHOD_ITEM_STARTED.to_string()));
    assert!(methods.contains(&mini_agent_app_server_protocol::METHOD_ITEM_COMPLETED.to_string()));
    let started = methods
        .iter()
        .position(|method| method == mini_agent_app_server_protocol::METHOD_ITEM_STARTED)
        .unwrap();
    let completed = methods
        .iter()
        .position(|method| method == mini_agent_app_server_protocol::METHOD_ITEM_COMPLETED)
        .unwrap();
    assert!(started < completed);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lists_bounded_thread_items_with_cursor_projection() {
    let (mut connection, root) = managed_connection("thread-items-list");
    initialize_connection(&mut connection, "thread-items-list-test").await;
    let _ = start_turn(&mut connection, 2, "hello").await;
    wait_for_turn_finished(&mut connection).await;

    let first = rpc_call(
        &mut connection,
        3,
        mini_agent_app_server_protocol::METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({"threadId": "thread-1", "limit": 1}),
    )
    .await;
    assert_eq!(first["value"]["data"].as_array().unwrap().len(), 1);
    let cursor = first["value"]["nextCursor"].as_str().unwrap().to_string();
    let second = rpc_call(
        &mut connection,
        4,
        mini_agent_app_server_protocol::METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({
            "threadId": "thread-1",
            "cursor": cursor,
            "limit": 1
        }),
    )
    .await;
    assert_eq!(second["value"]["data"].as_array().unwrap().len(), 1);
    assert_eq!(
        first["value"]["data"][0]["turnId"],
        second["value"]["data"][0]["turnId"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn child_wakeup_source_is_live_replayable_and_persisted_on_thread_items() {
    let root = rpc_root("child-wakeup-source");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let thread_id = opened.store.thread_id().to_string();
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), opened);
    initialize_connection(&mut connection, "child-wakeup-source-test").await;

    let start = rpc_result(
        &mut connection,
        JsonRpcRequest::request(
            2,
            METHOD_TURN_START,
            serde_json::json!(TurnStartParams {
                thread_id: ThreadId::new(thread_id.clone()),
                input: TurnInput::new(TurnInputMode::Start, "child update batch"),
                operation_id: None,
                operation_attempt: None,
                operation_attempt_kind: None,
                operation_group_id: None,
                execution_mode: None,
                group_sequence: None,
                turn_source: Some(TurnSource::ChildWakeup),
            }),
        ),
    )
    .await;
    assert_eq!(start["value"]["status"], "started");

    let started = loop {
        let event = next_turn_event(&mut connection).await;
        if matches!(event.event, mini_agent_protocol::Event::TurnStarted { .. }) {
            break event;
        }
    };
    assert_eq!(started.turn_source, Some(TurnSource::ChildWakeup));
    wait_for_turn_finished(&mut connection).await;

    let items = rpc_call(
        &mut connection,
        3,
        METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({"threadId": thread_id, "limit": 16}),
    )
    .await;
    let data = items["value"]["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert!(
        data.iter()
            .all(|entry| entry["turnSource"] == "child_wakeup")
    );
    assert!(
        data.iter()
            .any(|entry| entry["item"]["text"] == "child update batch")
    );

    let replay = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_EVENTS,
        serde_json::json!({"threadId": thread_id, "afterSequence": 0, "limit": 64}),
    )
    .await;
    let replay_events = replay["data"].as_array().unwrap();
    assert!(replay_events.iter().any(|event| {
        event["event"]["type"] == "turn_started" && event["turnSource"] == "child_wakeup"
    }));

    connection.shutdown().await.unwrap();
    let resumed = SessionStore::open(&root, SessionStoreRequest::Resume(session_id)).unwrap();
    let mut restarted = managed_connection_with_session(DoneModel, root.clone(), resumed);
    initialize_connection(&mut restarted, "child-wakeup-source-restart-test").await;
    let items = rpc_call(
        &mut restarted,
        2,
        METHOD_THREAD_ITEMS_LIST,
        serde_json::json!({"threadId": thread_id, "limit": 16}),
    )
    .await;
    assert!(
        items["value"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["turnSource"] == "child_wakeup")
    );
    restarted.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn frozen_session_rejects_turns_until_explicit_resume_turn() {
    let root = rpc_root("session-control-freeze");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let thread_id = opened.store.thread_id().to_string();
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), opened);
    initialize_connection(&mut connection, "session-control-test").await;

    let control = |id, action: &str, request_id: Option<&str>| {
        let mut params = serde_json::json!({
            "threadId": thread_id,
            "action": action,
        });
        if let Some(request_id) = request_id {
            params["requestId"] = serde_json::json!(request_id);
        }
        JsonRpcRequest::request(id, METHOD_SESSION_CONTROL, params)
    };
    let start =
        |id, turn_source| session_turn_start_request(id, &thread_id, "continue work", turn_source);

    let freezing = rpc_result(&mut connection, control(2, "freeze", Some("freeze-1"))).await;
    assert_eq!(freezing["value"]["status"], "freezing");
    let frozen = rpc_result(
        &mut connection,
        control(3, "freeze_settled", Some("freeze-1")),
    )
    .await;
    assert_eq!(frozen["value"]["status"], "frozen");

    let rejected = rpc_result(&mut connection, start(4, None)).await;
    assert_eq!(rejected["value"]["status"], "not_submitted");
    assert!(
        rejected["value"]["reason"]
            .as_str()
            .unwrap()
            .contains("explicit continue")
    );

    let resuming = rpc_result(&mut connection, control(5, "resume", Some("resume-1"))).await;
    assert_eq!(resuming["value"]["status"], "resuming");
    let rejected_during_resume = rpc_result(&mut connection, start(6, None)).await;
    assert_eq!(rejected_during_resume["value"]["status"], "not_submitted");

    let resumed_turn = rpc_result(&mut connection, start(7, Some(TurnSource::SessionResume))).await;
    assert_eq!(resumed_turn["value"]["status"], "started");
    wait_for_turn_finished(&mut connection).await;
    let running = rpc_result(&mut connection, control(8, "read", None)).await;
    assert_eq!(running["value"]["status"], "running");

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn turn_resume_continues_the_same_turn_from_a_persisted_execution_checkpoint() {
    let root = rpc_root("execution-checkpoint-resume");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let session_id = opened.store.session_id().to_string();
    let thread_id = opened.store.thread_id().to_string();
    let input = TurnInput::new(TurnInputMode::Start, "continue persisted work");
    let turn_id = mini_agent_protocol::TurnId::new("turn-resume-checkpoint");
    let messages = vec![Message::User {
        text: input.text.clone(),
    }];
    let mut journal = opened.store.execution_journal(&messages);
    let checkpoint_seq = journal
        .append(mini_agent_core::ExecutionJournalEntry::Checkpoint {
            checkpoint: mini_agent_core::ExecutionCheckpoint {
                turn_id: turn_id.clone(),
                input,
                messages,
                next_model_step: 1,
                final_text: String::new(),
                phase: mini_agent_core::ExecutionPhase::ModelRequest,
            },
        })
        .unwrap();
    journal
        .append(mini_agent_core::ExecutionJournalEntry::WaitingForContinue {
            turn_id: turn_id.clone(),
            reason: "provider_temporarily_unavailable".to_string(),
        })
        .unwrap();
    drop(journal);
    drop(opened);

    let resumed = SessionStore::open(&root, SessionStoreRequest::Resume(session_id)).unwrap();
    let mut connection = managed_connection_with_session(DoneModel, root.clone(), resumed);
    initialize_connection(&mut connection, "execution-checkpoint-resume-test").await;

    let checkpoint = rpc_call(
        &mut connection,
        2,
        "thread/read",
        serde_json::json!({"threadId": thread_id}),
    )
    .await;
    assert_eq!(
        checkpoint["value"]["executionRecovery"]["turnId"],
        turn_id.as_str()
    );
    assert_eq!(
        checkpoint["value"]["executionRecovery"]["status"],
        "waiting_for_continue"
    );
    assert_eq!(
        checkpoint["value"]["executionRecovery"]["phase"],
        "model_request"
    );
    assert_eq!(
        checkpoint["value"]["executionRecovery"]["checkpointSeq"],
        checkpoint_seq
    );

    let blocked_start = rpc_call(
        &mut connection,
        3,
        METHOD_TURN_START,
        serde_json::json!({
            "threadId": thread_id,
            "input": {"mode": "start", "text": "do not replace the saved Turn"}
        }),
    )
    .await;
    assert_eq!(blocked_start["value"]["status"], "not_submitted");
    assert!(
        blocked_start["value"]["reason"]
            .as_str()
            .unwrap()
            .contains("execution checkpoint")
    );

    let submission = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_RESUME,
        serde_json::json!({
            "threadId": thread_id,
            "turnId": turn_id,
            "checkpointSeq": checkpoint_seq,
            "requestId": "resume-checkpoint-1"
        }),
    )
    .await;
    assert_eq!(submission["value"]["status"], "started");
    assert_eq!(submission["value"]["turn_id"], turn_id.as_str());

    wait_for_turn_finished(&mut connection).await;
    let result = rpc_call(
        &mut connection,
        5,
        METHOD_TURN_READ,
        serde_json::json!({"turnId": turn_id}),
    )
    .await;
    assert_eq!(result["value"]["status"], "completed");
    assert_eq!(result["value"]["finalText"], "done");
    assert_eq!(result["value"]["recovery"]["status"], "settled");

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn turn_resume_after_step_limit_gets_another_bounded_step_slice() {
    let root = rpc_root("step-limit-resume-slice");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let thread_id = opened.store.thread_id().to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut connection = managed_connection_with_session(
        ScenarioModel::ResumeAfterStepLimit(calls.clone()),
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "step-limit-resume-slice-test").await;

    let start = rpc_result(
        &mut connection,
        session_turn_start_request(2, &thread_id, "continue within bounded slices", None),
    )
    .await;
    assert_eq!(start["value"]["status"], "started");
    let turn_id = start["value"]["turn_id"].as_str().unwrap().to_string();
    wait_for_turn_finished(&mut connection).await;
    assert_eq!(calls.load(Ordering::SeqCst), 8);

    let checkpoint = rpc_call(
        &mut connection,
        3,
        METHOD_THREAD_READ,
        serde_json::json!({"threadId": thread_id}),
    )
    .await;
    let recovery = &checkpoint["value"]["executionRecovery"];
    assert_eq!(recovery["status"], "waiting_for_continue");
    let checkpoint_seq = recovery["checkpointSeq"].as_u64().unwrap();
    let resumed = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_RESUME,
        serde_json::json!({
            "threadId": thread_id,
            "turnId": turn_id,
            "checkpointSeq": checkpoint_seq,
            "requestId": "step-limit-resume-slice-1",
        }),
    )
    .await;
    assert_eq!(resumed["value"]["status"], "started");
    wait_for_turn_finished(&mut connection).await;

    let result = rpc_call(
        &mut connection,
        5,
        METHOD_TURN_READ,
        serde_json::json!({"turnId": turn_id}),
    )
    .await;
    assert_eq!(result["value"]["status"], "completed");
    assert_eq!(
        result["value"]["finalText"],
        "continued after the manual step limit"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 9);

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn resume_settlement_is_idempotent_while_the_resumed_turn_is_active() {
    let root = rpc_root("session-resume-settled-active");
    let mut opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let thread_id = opened.store.thread_id().to_string();
    opened
        .store
        .transition_session_control(
            mini_agent_capabilities::SessionControlAction::Freeze,
            "freeze-before-resume",
        )
        .unwrap();
    opened
        .store
        .transition_session_control(
            mini_agent_capabilities::SessionControlAction::FreezeSettled,
            "freeze-before-resume",
        )
        .unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut connection = managed_connection_with_session(
        ScenarioModel::Timeout(release.clone()),
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "session-resume-settled-active-test").await;

    let resuming = rpc_call(
        &mut connection,
        2,
        METHOD_SESSION_CONTROL,
        serde_json::json!({
            "threadId": thread_id,
            "action": "resume",
            "requestId": "active-resume-1",
        }),
    )
    .await;
    assert_eq!(resuming["value"]["status"], "resuming");

    let started = rpc_result(
        &mut connection,
        session_turn_start_request(
            3,
            &thread_id,
            "hold the resumed turn active",
            Some(TurnSource::SessionResume),
        ),
    )
    .await;
    assert_eq!(started["value"]["status"], "started");
    loop {
        if matches!(
            next_turn_event(&mut connection).await.event,
            mini_agent_protocol::Event::TurnStarted { .. }
        ) {
            break;
        }
    }

    let settled = rpc_call(
        &mut connection,
        4,
        METHOD_SESSION_CONTROL,
        serde_json::json!({
            "threadId": thread_id,
            "action": "resume_settled",
            "requestId": "active-resume-1",
        }),
    )
    .await;
    assert_eq!(settled["value"]["status"], "running");

    release.notify_one();
    wait_for_turn_finished(&mut connection).await;
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn active_turn_accepts_freeze_before_gateway_interrupts_it() {
    let root = rpc_root("session-control-active-freeze");
    let opened = SessionStore::open(&root, SessionStoreRequest::New).unwrap();
    let thread_id = opened.store.thread_id().to_string();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut connection = managed_connection_with_session(
        ScenarioModel::Timeout(release.clone()),
        root.clone(),
        opened,
    );
    initialize_connection(&mut connection, "session-control-active-freeze-test").await;

    let start = rpc_result(
        &mut connection,
        session_turn_start_request(2, &thread_id, "wait for Session freeze", None),
    )
    .await;
    assert_eq!(start["value"]["status"], "started");
    let started = loop {
        let event = next_turn_event(&mut connection).await;
        if matches!(event.event, mini_agent_protocol::Event::TurnStarted { .. }) {
            break event;
        }
    };
    let turn_id = started.turn_id.unwrap().as_str().to_string();

    let freezing = rpc_call(
        &mut connection,
        3,
        METHOD_SESSION_CONTROL,
        serde_json::json!({
            "threadId": thread_id,
            "action": "freeze",
            "requestId": "active-freeze-1",
        }),
    )
    .await;
    assert_eq!(freezing["value"]["status"], "freezing");

    let interrupt = rpc_call(
        &mut connection,
        4,
        METHOD_TURN_INTERRUPT,
        serde_json::json!({"threadId": thread_id, "turnId": turn_id}),
    )
    .await;
    assert_eq!(interrupt["value"]["accepted"], true);
    release.notify_one();
    wait_for_turn_finished(&mut connection).await;

    let frozen = rpc_call(
        &mut connection,
        5,
        METHOD_SESSION_CONTROL,
        serde_json::json!({
            "threadId": thread_id,
            "action": "freeze_settled",
            "requestId": "active-freeze-1",
        }),
    )
    .await;
    assert_eq!(frozen["value"]["status"], "frozen");

    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exposes_codex_shaped_thread_goal_lifecycle() {
    let (mut connection, root) = managed_connection("thread-goal-rpc");
    initialize_connection(&mut connection, "thread-goal-test").await;

    let result = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_GOAL_GET,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert!(result["value"]["goal"].is_null());

    let result = set_goal(
        &mut connection,
        3,
        Some("ship the next iteration"),
        None,
        Some(1200),
    )
    .await;
    assert_eq!(
        result["value"]["goal"]["objective"],
        "ship the next iteration"
    );
    let set_revision = result["stateRevision"].as_u64().unwrap();
    assert!(set_revision > 0);
    assert_eq!(result["value"]["goal"]["status"], "active");
    assert_eq!(result["value"]["goal"]["tokenBudget"], 1200);
    assert!(result["value"]["goal"].get("path").is_none());
    let mut active_turn_seen = false;
    let mut verification_failed_seen = false;
    let notification = loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(3), connection.next_notification())
                .await
                .expect("Goal preparation failure should settle within the test deadline")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == "active" && params["turnId"] == "turn-thread-1-1" {
                active_turn_seen = true;
            }
            if params["goal"]["status"] == "blocked" {
                assert!(params["stateRevision"].as_u64().unwrap() >= set_revision);
                break params;
            }
        } else if notification.method
            == mini_agent_app_server_protocol::METHOD_GOAL_VERIFICATION_FAILED
        {
            verification_failed_seen = true;
            assert!(notification.params.unwrap()["error"].is_string());
        }
    };
    assert!(active_turn_seen);
    assert!(verification_failed_seen);
    let runtime_status = rpc_call(
        &mut connection,
        50,
        METHOD_RUNTIME_STATUS,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(runtime_status["phase"], "failed");
    assert!(runtime_status["error"].is_string());
    assert_eq!(notification["goal"]["objective"], "ship the next iteration");
    assert_eq!(notification["turnId"], "turn-thread-1-1");

    let result = set_goal(
        &mut connection,
        4,
        Some("replace after verifier preparation failure"),
        None,
        None,
    )
    .await;
    assert_eq!(
        result["value"]["goal"]["objective"],
        "replace after verifier preparation failure"
    );
    wait_for_goal_status(&mut connection, "blocked").await;

    let result = rpc_call(
        &mut connection,
        5,
        METHOD_THREAD_GOAL_CLEAR,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert_eq!(result["value"]["cleared"], true);
    let clear_revision = result["stateRevision"].as_u64().unwrap();
    loop {
        let notification = connection.next_notification().await.unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_CLEARED {
            assert_eq!(
                notification.params.unwrap()["stateRevision"],
                clear_revision
            );
            break;
        }
    }

    let result = rpc_call(
        &mut connection,
        6,
        METHOD_THREAD_GOAL_GET,
        serde_json::json!({"threadId": "thread-1"}),
    )
    .await;
    assert!(result["value"]["goal"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn resumes_settled_goal_without_replaying_turn() {
    let root = rpc_root("goal-resume-settled");
    let store = mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    );
    let state = store.set_goal("resume settled goal", None).unwrap();
    store
        .mark_goal_turn_started(&state.goal_id, "turn-1")
        .unwrap();
    store
        .mark_goal_turn_settled(&state.goal_id, "turn-1")
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut connection, root) = managed_connection_at(
        ScenarioModel::Counting(calls.clone()),
        root,
        crate::goal_service::GoalLimits::default(),
    );
    initialize_connection(&mut connection, "goal-resume-test").await;

    let notification = loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(3), connection.next_notification())
                .await
                .expect("settled Goal resume should reach a terminal notification")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == "blocked" {
                break params;
            }
        }
    };
    assert_eq!(notification["turnId"], "turn-1");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.load_goal_state().unwrap().unwrap().status,
        mini_agent_host::GoalStatus::Failed
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn exposes_goal_pause_and_resume_through_thread_protocol() {
    let release = Arc::new(tokio::sync::Notify::new());
    let (mut connection, root) = managed_connection_with(
        ScenarioModel::Timeout(release.clone()),
        "thread-goal-pause-resume",
        crate::goal_service::GoalLimits::default(),
    );
    initialize_connection(&mut connection, "thread-goal-pause-resume-test").await;

    let started = set_goal(
        &mut connection,
        2,
        Some("pause before the next milestone"),
        None,
        None,
    )
    .await;
    assert_eq!(started["value"]["goal"]["status"], "active");
    loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(3), connection.next_notification())
                .await
                .expect("Goal turn should start before pause")
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED {
            let params = notification.params.unwrap();
            if params["goal"]["status"] == "active" && params["turnId"] == "turn-thread-1-1" {
                break;
            }
        }
    }

    let paused = set_goal(&mut connection, 3, None, Some("paused"), None).await;
    assert_eq!(paused["value"]["goal"]["status"], "paused");

    release.notify_one();
    let _ = next_turn_event(&mut connection).await;

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn resumes_a_paused_goal_through_thread_protocol() {
    let root = rpc_root("thread-goal-resume");
    let store = mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    );
    let state = store
        .set_goal("resume before the next milestone", None)
        .unwrap();
    store.pause_goal().unwrap();
    let (mut connection, root) =
        managed_connection_at(DoneModel, root, crate::goal_service::GoalLimits::default());
    initialize_connection(&mut connection, "thread-goal-resume-test").await;

    let resumed = set_goal(&mut connection, 2, None, Some("active"), None).await;
    assert_eq!(resumed["value"]["goal"]["status"], "active");
    assert_eq!(resumed["value"]["goal"]["objective"], state.objective);
    wait_for_goal_status(&mut connection, "blocked").await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn enforces_goal_step_budget_at_the_core_boundary() {
    let (mut connection, root) = managed_connection_with(
        ScenarioModel::StepLimit,
        "goal-step-budget",
        crate::goal_service::GoalLimits {
            milestone_step_budget: 1,
            ..crate::goal_service::GoalLimits::default()
        },
    );
    initialize_connection(&mut connection, "goal-step-budget-test").await;
    let _ = set_goal(
        &mut connection,
        2,
        Some("stop after one model step"),
        None,
        None,
    )
    .await;

    let notification = wait_for_goal_status(&mut connection, "usageLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn records_goal_usage_and_enforces_token_budget() {
    let (mut connection, root) = managed_connection_with(
        ScenarioModel::Budget,
        "goal-token-budget",
        crate::goal_service::GoalLimits::default(),
    );
    initialize_connection(&mut connection, "goal-token-budget-test").await;
    let _ = set_goal(
        &mut connection,
        2,
        Some("stop at the token budget"),
        None,
        Some(5),
    )
    .await;

    let notification = wait_for_goal_status(&mut connection, "budgetLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 5);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn enforces_goal_timeout_with_cooperative_cancellation() {
    let release = Arc::new(tokio::sync::Notify::new());
    let (mut connection, root) = managed_connection_with(
        ScenarioModel::Timeout(release.clone()),
        "goal-timeout",
        crate::goal_service::GoalLimits {
            milestone_timeout_secs: 1,
            ..crate::goal_service::GoalLimits::default()
        },
    );
    initialize_connection(&mut connection, "goal-timeout-test").await;
    let _ = set_goal(
        &mut connection,
        2,
        Some("stop when the milestone times out"),
        None,
        None,
    )
    .await;

    loop {
        let notification =
            tokio::time::timeout(Duration::from_secs(2), connection.next_notification())
                .await
                .unwrap()
                .unwrap();
        if notification.method == mini_agent_app_server_protocol::METHOD_THREAD_GOAL_UPDATED
            && notification.params.as_ref().is_some_and(|params| {
                params["turnId"] == "turn-thread-1-1" && params["goal"]["status"] == "active"
            })
        {
            break;
        }
    }
    assert!(matches!(
        connection.shutdown().await,
        Err(AppServerError::Busy)
    ));
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    release.notify_one();

    let notification = wait_for_goal_status(&mut connection, "usageLimited").await;
    assert_eq!(notification["goal"]["tokensUsed"], 0);
    connection.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn binds_active_goal_workspace_to_approval_controller() {
    let root = rpc_root("goal-approval-path");
    mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    )
    .set_goal("bind the goal workspace", None)
    .unwrap();
    mini_agent_host::HostWorkflowStore::new(
        root.clone(),
        crate::goal_service::GoalLimits::default(),
    )
    .pause_goal()
    .unwrap();
    let approval = ApprovalController::with_preset(ApprovalPolicy::Automatic, Default::default());
    let observed_approval = approval.clone();
    let server = crate::tests::server(DoneModel);
    let management = RuntimeManagementService::new(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect_with_roots(
            &root,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        approval,
    );
    let thread_settings = ThreadSettingsService::new();
    let goals =
        ThreadGoalRequestProcessor::new(root.clone(), crate::goal_service::GoalLimits::default());
    let _connection = AppServerConnection::new(server)
        .with_runtime_services(RuntimeServices::new(management, thread_settings, goals).unwrap());
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
    let approval = ApprovalController::with_policy_and_callback(
        ApprovalPolicy::Interactive,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        move |request| {
            approval_broker
                .request_resolution(request)
                .map(|resolution| mini_agent_protocol::ToolApprovalResolution {
                    outcome: resolution.outcome,
                    grant_scope: resolution
                        .grant_scope
                        .unwrap_or(mini_agent_protocol::ActionGrantScope::Once),
                    reason: resolution.reason,
                })
                .map_err(mini_agent_protocol::ToolError)
        },
    );
    let tools = workspace_tools_with_read_roots_and_results(
        root.clone(),
        approval.clone(),
        Vec::new(),
        Vec::new(),
        SandboxKind::Native,
        ImageStore::memory_only(),
        ResultStore::default(),
    )
    .unwrap();
    let registry = ToolRouter::with_executor(
        tools,
        Arc::new(mini_agent_host::ToolOrchestrator::new(approval)),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(
            ThreadId::new("initial"),
            Harness::new(
                ScenarioModel::ShellApproval,
                registry,
                HarnessConfig::default(),
            ),
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

    let turn_response = start_turn(&mut connection, 2, "run shell").await;
    assert_eq!(turn_response["value"]["turn_id"], "turn-thread-1-1");

    let pending = tokio::time::timeout(Duration::from_secs(3), broker.next_request())
        .await
        .expect("Shell approval should reach the App Server broker");
    assert!(pending.request_id.ends_with("-shell-call-1"));
    assert_eq!(
        pending.action,
        format!("shell command `{}`", shell_approval_command())
    );
    assert_eq!(pending.call_id.as_deref(), Some("shell-call-1"));
    assert_eq!(pending.thread_id, Some(ThreadId::new("thread-1")));
    assert_eq!(
        pending.turn_id,
        Some(mini_agent_protocol::TurnId::new("turn-thread-1-1"))
    );

    let approval_response = rpc_call(
        &mut connection,
        3,
        METHOD_APPROVAL_RESPOND,
        serde_json::to_value(ApprovalRespondParams {
            request_id: pending.request_id.clone(),
            decision: mini_agent_app_server_protocol::ApprovalDecision::Approve,
            grant_scope: Some(mini_agent_app_server_protocol::ActionGrantScope::Once),
            reason: None,
        })
        .unwrap(),
    )
    .await;
    assert_eq!(approval_response["accepted"], true);

    let resolution = tokio::time::timeout(Duration::from_secs(3), broker.next_event())
        .await
        .expect("Shell approval resolution should be recorded");
    let resolution = match resolution {
        ApprovalEvent::Resolved(resolution) => resolution,
        ApprovalEvent::Requested(_) => panic!("expected approval resolution"),
    };
    assert_eq!(resolution.request_id, pending.request_id);
    assert_eq!(resolution.call_id.as_deref(), Some("shell-call-1"));
    assert_eq!(resolution.thread_id, Some(ThreadId::new("thread-1")));
    assert_eq!(
        resolution.turn_id,
        Some(mini_agent_protocol::TurnId::new("turn-thread-1-1"))
    );
    assert_eq!(
        resolution.outcome,
        mini_agent_app_server_protocol::ApprovalOutcome::Approved
    );

    let mut tool_finished_seen = false;
    loop {
        let notification = next_turn_event(&mut connection).await;
        assert_eq!(
            notification.turn_id,
            Some(mini_agent_protocol::TurnId::new("turn-thread-1-1"))
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
async fn serves_approval_response_while_turn_steer_is_pending() {
    let root = rpc_root("shell-approval-steer-transport");
    let broker = ApprovalBroker::new();
    let approval_broker = broker.clone();
    let approval = ApprovalController::with_policy_and_callback(
        ApprovalPolicy::Interactive,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        move |request| {
            approval_broker
                .request_resolution(request)
                .map(|resolution| mini_agent_protocol::ToolApprovalResolution {
                    outcome: resolution.outcome,
                    grant_scope: resolution
                        .grant_scope
                        .unwrap_or(mini_agent_protocol::ActionGrantScope::Once),
                    reason: resolution.reason,
                })
                .map_err(mini_agent_protocol::ToolError)
        },
    );
    let tools = workspace_tools_with_read_roots_and_results(
        root.clone(),
        approval.clone(),
        Vec::new(),
        Vec::new(),
        SandboxKind::Native,
        ImageStore::memory_only(),
        ResultStore::default(),
    )
    .unwrap();
    let registry = ToolRouter::with_executor(
        tools,
        Arc::new(mini_agent_host::ToolOrchestrator::new(approval)),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(
            ThreadId::new("initial"),
            Harness::new(
                ScenarioModel::ShellApproval,
                registry,
                HarnessConfig::default(),
            ),
        ),
    );
    let (mut input, server_input) = tokio::io::duplex(16 * 1024);
    let (server_output, output) = tokio::io::duplex(16 * 1024);
    let task = tokio::spawn(serve_stdio_with_approval_and_manifest(
        server,
        broker.clone(),
        default_capability_manifest(),
        tokio::io::BufReader::new(server_input),
        server_output,
    ));
    let mut output = tokio::io::BufReader::new(output);

    input
        .write_all(
            format!(
                "{}\n",
                serde_json::to_string(&initialize_request(1, "transport-steer-test")).unwrap()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut line = String::new();
    output.read_line(&mut line).await.unwrap();
    let initialize: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(initialize["id"], 1);

    input
        .write_all(
            format!(
                "{}\n{}\n",
                serde_json::to_string(&JsonRpcRequest::notification(METHOD_INITIALIZED, None,))
                    .unwrap(),
                serde_json::to_string(&turn_start_request(2, "run shell")).unwrap(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut approval_request = None;
    let mut turn_started = false;
    while approval_request.is_none() || !turn_started {
        line.clear();
        output.read_line(&mut line).await.unwrap();
        let message: Value = serde_json::from_str(line.trim()).unwrap();
        if message["id"] == 2 {
            turn_started = true;
        }
        if message["method"] == mini_agent_app_server_protocol::METHOD_APPROVAL_REQUEST {
            approval_request = message["params"]["requestId"].as_str().map(str::to_owned);
        }
    }
    let request_id = approval_request.unwrap();
    let steer = JsonRpcRequest::request(
        3,
        METHOD_TURN_STEER,
        serde_json::to_value(TurnSteerParams {
            thread_id: ThreadId::new("thread-1"),
            turn_id: mini_agent_protocol::TurnId::new("turn-1"),
            text: "continue after approval".to_string(),
            request_id: None,
        })
        .unwrap(),
    );
    let approval_response = JsonRpcRequest::request(
        4,
        METHOD_APPROVAL_RESPOND,
        serde_json::to_value(ApprovalRespondParams {
            request_id,
            decision: mini_agent_app_server_protocol::ApprovalDecision::Approve,
            grant_scope: Some(mini_agent_app_server_protocol::ActionGrantScope::Once),
            reason: None,
        })
        .unwrap(),
    );
    input
        .write_all(
            format!(
                "{}\n{}\n",
                serde_json::to_string(&steer).unwrap(),
                serde_json::to_string(&approval_response).unwrap(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();

    let approval_ack = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            line.clear();
            output.read_line(&mut line).await.unwrap();
            let message: Value = serde_json::from_str(line.trim()).unwrap();
            if message["id"] == 4 {
                break message;
            }
        }
    })
    .await
    .expect("approval response must not wait behind turn/steer");
    assert_eq!(approval_ack["result"]["accepted"], true);

    let steer_response = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            line.clear();
            output.read_line(&mut line).await.unwrap();
            let message: Value = serde_json::from_str(line.trim()).unwrap();
            if message["id"] == 3 {
                break message;
            }
        }
    })
    .await
    .expect("turn/steer should settle after approval");
    assert_eq!(steer_response["id"], 3);

    input.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
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
    let _ = start_turn(&mut json_rpc, 2, "hello").await;
    let mut json_events = Vec::new();
    loop {
        let event = next_turn_event(&mut json_rpc).await;
        let mut envelope =
            EventEnvelope::new(event.thread_id, event.turn_id, event.sequence, event.event);
        envelope.item_id = event.item_id;
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
    let started = start_turn(&mut connection, 2, "hello").await;
    let turn_id: mini_agent_protocol::TurnId =
        serde_json::from_value(started["value"]["turn_id"].clone()).unwrap();
    wait_for_turn_finished(&mut connection).await;
    let turn = rpc_call(
        &mut connection,
        3,
        METHOD_TURN_READ,
        serde_json::json!(TurnReadParams {
            turn_id: turn_id.clone(),
        }),
    )
    .await;
    assert_eq!(turn["value"]["finalText"], "done");

    let thread = rpc_call(
        &mut connection,
        4,
        METHOD_THREAD_READ,
        serde_json::json!(ThreadReadParams {
            thread_id: ThreadId::new("thread-1"),
        }),
    )
    .await;
    assert_eq!(thread["value"]["status"], "idle");
}

#[tokio::test]
async fn exposes_factory_backed_thread_lifecycle_methods() {
    let initial_harness = harness(DoneModel);
    let server = AppServer::with_thread_factory(
        ThreadStart::new(ThreadId::new("thread-1")),
        vec![Thread::new(ThreadId::new("initial"), initial_harness)],
        |id| Ok(Thread::new(id, harness(DoneModel))),
    );
    let mut connection = AppServerConnection::new(server);
    let _ = connection
        .handle_request(initialize_request(1, "lifecycle-test"))
        .await;
    let result = rpc_call(
        &mut connection,
        2,
        METHOD_THREAD_START,
        serde_json::json!(ThreadStartParams {
            thread_id: Some(ThreadId::new("thread-2")),
        }),
    )
    .await;
    assert_eq!(result["value"]["threadId"], "thread-2");
    assert_eq!(result["actionId"], 1);
    assert_eq!(result["actionSequence"], 1);
    assert_eq!(result["stateRevision"], 0);
    let result = rpc_call(
        &mut connection,
        3,
        METHOD_THREAD_FORK,
        serde_json::json!(ThreadForkParams {
            source_thread_id: ThreadId::new("thread-1"),
            new_thread_id: ThreadId::new("thread-3"),
        }),
    )
    .await;
    assert_eq!(result["value"]["threadId"], "thread-3");
    assert_eq!(result["actionId"], 2);
    assert_eq!(result["actionSequence"], 2);
    assert_eq!(result["stateRevision"], 0);
    let listed = rpc_call(
        &mut connection,
        4,
        METHOD_THREAD_LIST,
        serde_json::json!(ThreadListParams::default()),
    )
    .await;
    assert_eq!(listed["data"].as_array().unwrap().len(), 3);
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
