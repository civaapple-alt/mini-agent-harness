use super::AppServer;
use super::AppServerConnection;
use super::AppServerError;
use super::JsonlTrace;
use super::LocalAppServerClient;
use super::RuntimeManagementService;
use super::RuntimeServices;
use super::ThreadSettingsService;
use super::ThreadUpdate;
use super::worker::Command;
use crate::goal_service::ThreadGoalRequestProcessor;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalPolicy;
use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRouter;
use mini_agent_protocol::Event;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::SkillLoadPhase;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use mini_agent_protocol::TurnSubmission;
use mini_agent_protocol::TurnWorkflow;
use mini_agent_protocol::TurnWorkflowKind;
use mini_agent_protocol::TurnWorkflowMode;
use serde_json::Value;
use serde_json::from_str;
use serde_json::json;
use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use tokio::sync::oneshot;

pub(crate) struct DoneModel;

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

struct BlockingModel {
    release: Arc<Notify>,
}

struct ApprovalModel;

struct McpTimeoutModel;

#[derive(Clone, Debug)]
struct KnowledgeWorkObservation {
    system_prompt: String,
    tool_count: usize,
    messages: Vec<Message>,
}

#[derive(Clone)]
struct KnowledgeWorkMockModel {
    observations: Arc<Mutex<Vec<KnowledgeWorkObservation>>>,
}

struct ReferenceReadFixtureTool {
    root: PathBuf,
}

impl ToolHandler for ReferenceReadFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read the selected local Skill reference fixture.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }
    }
}

impl ToolRuntime for ReferenceReadFixtureTool {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("read_file path is required".to_string()))?;
        if path != "references/needed.md" {
            return Err(ToolError(format!("fixture path is not allowed: {path}")));
        }
        fs::read_to_string(self.root.join("needed.md"))
            .map_err(|error| ToolError(format!("fixture reference read failed: {error}")))
    }
}

impl Model for KnowledgeWorkMockModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let prompt = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        self.observations
            .lock()
            .unwrap()
            .push(KnowledgeWorkObservation {
                system_prompt: request.system_prompt.to_string(),
                tool_count: request.tools.len(),
                messages: request.messages.to_vec(),
            });
        if !request.messages.iter().any(|message| {
            matches!(
                message,
                Message::Tool {
                    name,
                    is_error: false,
                    ..
                } if name == "read_file"
            )
        }) {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "knowledge-work-reference".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({"path": "references/needed.md"}),
                }],
                usage: None,
            });
        }
        let text = if prompt.contains("统一搜索") {
            "Goals\n- 统一搜索\n\nNon-goals\n- 不自动创建任务\n\nUser Stories\n- 用户可以搜索\n\nAcceptance Criteria\n- 搜索结果可验证\n\nOpen Questions\n- 数据源范围是什么？"
        } else if prompt.contains("任务") {
            "Tasks\n- 整理输入任务\n\nBlockers\n- 会议记录缺少负责人\n\nFollow-ups\n- 确认下一步\n\nMissing Information\n- 截止时间未提供"
        } else {
            "SQL Draft\nSELECT 1;\n\nDefinitions\n- 指标口径待确认\n\nValidation Checks\n- 检查空值和重复值\n\nLimitations\n- 没有直接数据源"
        };
        Ok(ModelResponse {
            reasoning: String::new(),
            text: text.to_string(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

impl Model for ApprovalModel {
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
                    outcome: Some(ToolExecutionStatus::NeedsApproval),
                    ..
                }
            )
        }) {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "denial received".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "approval-call".to_string(),
                name: "sensitive_fixture".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        })
    }
}

impl Model for McpTimeoutModel {
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
                    content,
                    outcome: Some(ToolExecutionStatus::Failed),
                    ..
                } if name == "mcp__fixture__slow" && content == "MCP tool call timed out"
            )
        }) {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "timeout received".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "mcp-timeout-call".to_string(),
                name: "mcp__fixture__slow".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        })
    }
}

struct SensitiveFixtureTool;

struct McpTimeoutFixtureTool;

struct BlockingExecution {
    started: Arc<Notify>,
}

struct BlockingFixtureTool;

impl ToolHandler for BlockingFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "blocking_fixture".to_string(),
            description: "A synchronous tool that waits for cancellation.".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for BlockingFixtureTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError(
            "blocking fixture requires its delegate".to_string(),
        ))
    }
}

impl ToolExecutionDelegate for BlockingExecution {
    fn execute(&self, _tool: &dyn Tool, request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        self.started.notify_one();
        loop {
            if request
                .cancellation
                .as_ref()
                .is_some_and(|token| token.load(Ordering::Acquire))
            {
                return ToolExecutionOutcome::failed("cancelled");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

struct BlockingToolModel;

impl Model for BlockingToolModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        if request
            .messages
            .iter()
            .any(|message| matches!(message, Message::Tool { .. }))
        {
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "cancelled".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            });
        }
        Ok(ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "blocking-call".to_string(),
                name: "blocking_fixture".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        })
    }
}

impl ToolHandler for SensitiveFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sensitive_fixture".to_string(),
            description: "A fixture that requires approval".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for SensitiveFixtureTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("user denied: sensitive fixture".to_string()))
    }

    fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
        ToolExecutionOutcome {
            status: ToolExecutionStatus::NeedsApproval,
            content: "user denied: sensitive fixture".to_string(),
        }
    }
}

impl ToolHandler for McpTimeoutFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp__fixture__slow".to_string(),
            description: "A fixture that times out like an MCP call".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for McpTimeoutFixtureTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("MCP tool call timed out".to_string()))
    }
}

impl Model for BlockingModel {
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

pub(crate) fn harness<M: Model>(model: M) -> Harness<M> {
    Harness::new(model, ToolRouter::default(), HarnessConfig::default())
}

pub(crate) fn thread<M: Model>(thread_id: ThreadId, model: M) -> Thread<M> {
    Thread::new(thread_id, harness(model))
}

pub(crate) fn server_with_config<M: Model + Send + 'static>(
    model: M,
    config: HarnessConfig,
) -> AppServer<M> {
    let harness = Harness::new(model, ToolRouter::default(), config);
    AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    )
}

pub(crate) fn server<M: Model + Send + 'static>(model: M) -> AppServer<M> {
    server_with_config(model, HarnessConfig::default())
}

fn blocking_tool_server(started: Arc<Notify>) -> AppServer<BlockingToolModel> {
    let tools = ToolRouter::with_executor(
        vec![Box::new(BlockingFixtureTool)],
        Arc::new(BlockingExecution { started }),
    );
    let harness = Harness::new(BlockingToolModel, tools, HarnessConfig::default());
    AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    )
}

async fn run_turn_to_finished<M: Model + Send + 'static>(
    server: &AppServer<M>,
    prompt: &str,
) -> Vec<Event> {
    let mut events = server.subscribe();
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Start, prompt)),
            )
            .await
            .unwrap(),
        TurnSubmission::Started {
            turn_id: mini_agent_protocol::TurnId::new("turn-1")
        }
    );
    let mut received = Vec::new();
    while !received
        .iter()
        .any(|event| matches!(event, Event::TurnFinished { .. }))
    {
        received.push(events.recv().await.unwrap().event);
    }
    received
}

fn knowledge_work_client(
    model: KnowledgeWorkMockModel,
    workspace: &Path,
    builtin_root: &Path,
    enabled_groups: &[String],
) -> LocalAppServerClient<KnowledgeWorkMockModel> {
    let discovery = mini_agent_capabilities::discover_with_builtin_root(
        workspace,
        builtin_root,
        enabled_groups,
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(
            ThreadId::new("initial"),
            Harness::new(
                model,
                ToolRouter::new(vec![Box::new(ReferenceReadFixtureTool {
                    root: workspace.join("references"),
                })]),
                HarnessConfig::default(),
            ),
        ),
    );
    let management = RuntimeManagementService::new_with_harness_config_and_skills(
        server.clone(),
        None,
        mini_agent_host::WorldState::detect_with_roots(
            workspace,
            Vec::new(),
            SecurityPreset::Default,
            ApprovalPolicy::Automatic,
            SandboxKind::Native,
        ),
        Vec::new(),
        0,
        Vec::new(),
        ApprovalController::with_preset(ApprovalPolicy::Automatic, Default::default()),
        HarnessConfig::default(),
        Some(discovery),
    );
    let services = RuntimeServices::new(
        management,
        ThreadSettingsService::new(),
        ThreadGoalRequestProcessor::new(
            workspace.to_path_buf(),
            crate::goal_service::GoalLimits::default(),
        ),
    )
    .unwrap();
    LocalAppServerClient::new(AppServerConnection::new(server).with_runtime_services(services))
}

async fn run_turn_input(
    client: &mut LocalAppServerClient<KnowledgeWorkMockModel>,
    input: TurnInput,
) -> (mini_agent_app_server_protocol::TurnReadResult, Vec<Event>) {
    let submission = client
        .start_turn(ThreadId::new("thread-1"), input)
        .await
        .unwrap();
    let turn_id = match submission {
        TurnSubmission::Started { turn_id } => turn_id,
        other => panic!("unexpected turn submission: {other:?}"),
    };
    let mut events = Vec::new();
    loop {
        let envelope = client.next_event().await.unwrap();
        let finished = matches!(&envelope.event, Event::TurnFinished { .. });
        events.push(envelope.event);
        if finished {
            break;
        }
    }
    let result = client.read_turn(turn_id).await.unwrap();
    (result, events)
}

fn write_builtin_skill(builtin_root: &Path, name: &str, description: &str, body: &str) {
    let path = builtin_root.join("knowledge-work").join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .unwrap();
}

fn test_root(label: &str) -> PathBuf {
    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("mini-agent-app-server-{label}-{nonce}-{sequence}"));
    fs::create_dir_all(&root).unwrap();
    root
}

#[tokio::test]
async fn concurrent_commands_receive_unique_server_admission_metadata() {
    let server = server(DoneModel);
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let commands = server.commands.clone();
        tasks.push(tokio::spawn(async move {
            let (reply, response) = oneshot::channel();
            commands
                .send(Command::ReadThread {
                    thread_id: ThreadId::new("thread-1"),
                    reply,
                })
                .await
                .unwrap();
            response.await.unwrap().unwrap()
        }));
    }

    let mut sequences = Vec::new();
    let mut ids = Vec::new();
    for task in tasks {
        let response = task.await.unwrap();
        sequences.push(response.receipt.sequence);
        ids.push(response.receipt.id);
    }
    sequences.sort_unstable();
    ids.sort_unstable();
    assert_eq!(sequences.len(), 4);
    assert_eq!(ids.len(), 4);
    assert!(sequences.windows(2).all(|pair| pair[0] != pair[1]));
    assert!(ids.windows(2).all(|pair| pair[0] != pair[1]));
}

#[tokio::test]
async fn starts_turn_and_broadcasts_core_lifecycle_events() {
    let server = server(DoneModel);
    let mut events = server.subscribe();
    let submission = server
        .turn_start_for(
            ThreadId::new("thread-1"),
            TurnStart::new(TurnInput::new(TurnInputMode::Start, "inspect")),
        )
        .await
        .unwrap();
    let turn_id = match submission {
        TurnSubmission::Started { turn_id } => turn_id,
        other => panic!("unexpected submission: {other:?}"),
    };

    let mut received = Vec::new();
    for _ in 0..6 {
        received.push(events.recv().await.unwrap());
    }
    assert_eq!(
        received.first().unwrap().thread_id,
        ThreadId::new("thread-1")
    );
    assert_eq!(received.first().unwrap().turn_id, Some(turn_id.clone()));
    assert_eq!(
        received
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    assert!(matches!(received[0].event, Event::TurnStarted { .. }));
    assert!(matches!(received[1].event, Event::RunStarted { .. }));
    assert!(matches!(
        received.last().unwrap().event,
        Event::TurnFinished {
            status: mini_agent_protocol::TurnStatus::Completed
        }
    ));

    let second = server
        .turn_start_for(
            ThreadId::new("thread-1"),
            TurnStart::new(TurnInput::new(TurnInputMode::StartIfIdle, "again")),
        )
        .await
        .unwrap();
    assert_eq!(
        second,
        TurnSubmission::Started {
            turn_id: mini_agent_protocol::TurnId::new("turn-2")
        }
    );
}

#[tokio::test]
async fn explicit_skill_activation_failure_precedes_model_execution() {
    let server = server(DoneModel);
    let mut events = server.subscribe();
    let mut input = TurnInput::new(TurnInputMode::Start, "inspect");
    input.selected_skills = vec!["architect".to_string()];
    let submission = server
        .turn_start_for(ThreadId::new("thread-1"), TurnStart::new(input))
        .await
        .unwrap();
    assert!(matches!(submission, TurnSubmission::Started { .. }));

    let mut received = Vec::new();
    while !received
        .iter()
        .any(|event| matches!(event, Event::TurnFinished { .. }))
    {
        received.push(events.recv().await.unwrap().event);
    }
    assert!(matches!(received[0], Event::TurnStarted { .. }));
    assert!(matches!(received[1], Event::SkillsLoadFailed { .. }));
    assert!(matches!(received[2], Event::TurnFinished { .. }));
    assert!(
        !received
            .iter()
            .any(|event| matches!(event, Event::RunStarted { .. }))
    );
}

#[tokio::test]
async fn knowledge_work_mock_provider_covers_structured_read_only_scenarios() {
    let root = test_root("knowledge-work-scenarios");
    let builtin_root = root.join("builtin");
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(
        root.join("references/needed.md"),
        "REFERENCE BODY USED BY THE MOCK PROVIDER\n",
    )
    .unwrap();
    write_builtin_skill(
        &builtin_root,
        "product-management",
        "Create product specifications from local input.",
        "PRODUCT MANAGEMENT ENTRY",
    );
    write_builtin_skill(
        &builtin_root,
        "productivity",
        "Organize local tasks and meeting input.",
        "PRODUCTIVITY ENTRY",
    );
    write_builtin_skill(
        &builtin_root,
        "data",
        "Draft SQL and analyze user-provided data.",
        "DATA ENTRY",
    );

    let scenarios = [
        (
            "product-management",
            "为团队做一个统一搜索功能",
            vec![
                "Goals",
                "Non-goals",
                "User Stories",
                "Acceptance Criteria",
                "Open Questions",
            ],
            "PRODUCT MANAGEMENT ENTRY",
        ),
        (
            "productivity",
            "整理任务和会议记录",
            vec!["Tasks", "Blockers", "Follow-ups", "Missing Information"],
            "PRODUCTIVITY ENTRY",
        ),
        (
            "data",
            "根据业务问题和 CSV 摘要准备 SQL 分析",
            vec![
                "SQL Draft",
                "Definitions",
                "Validation Checks",
                "Limitations",
            ],
            "DATA ENTRY",
        ),
    ];

    for (skill, prompt, required_sections, marker) in scenarios {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let model = KnowledgeWorkMockModel {
            observations: observations.clone(),
        };
        let mut client =
            knowledge_work_client(model, &root, &builtin_root, &["knowledge-work".to_string()]);
        client
            .initialize("knowledge-work-test", "0.1")
            .await
            .unwrap();
        let mut input = TurnInput::new(TurnInputMode::Start, prompt);
        input.selected_skills = vec![format!("knowledge-work:{skill}")];
        let (result, events) = run_turn_input(&mut client, input).await;
        let qualified_name = format!("knowledge-work:{skill}");

        assert_eq!(result.status, mini_agent_protocol::TurnStatus::Completed);
        let output = result.final_text.as_deref().unwrap_or_default();
        for section in required_sections {
            assert!(
                output.contains(section),
                "{skill} output missing {section}: {output}"
            );
        }
        assert!(events.iter().any(|event| matches!(
            event,
            Event::SkillsLoaded {
                phase: SkillLoadPhase::Loaded,
                skills,
                ..
            } if skills.iter().any(|record| record.qualified_name.as_deref()
                == Some(qualified_name.as_str()))
        )));
        {
            let observations = observations.lock().unwrap();
            let observation = observations.last().expect("mock provider was not called");
            assert!(observation.system_prompt.contains(marker));
            assert_eq!(observation.tool_count, 1);
            assert!(observation.messages.iter().any(|message| matches!(
                message,
                Message::Tool {
                    name,
                    content,
                    is_error: false,
                    ..
                } if name == "read_file" && content.contains("REFERENCE BODY USED")
            )));
        }
        client.shutdown().await.unwrap();
    }

    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn knowledge_work_group_workflow_uses_requested_group_in_prompt() {
    let root = test_root("knowledge-work-group");
    let builtin_root = root.join("builtin");
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(
        root.join("references/needed.md"),
        "REFERENCE BODY USED BY THE MOCK PROVIDER\n",
    )
    .unwrap();
    write_builtin_skill(
        &builtin_root,
        "data",
        "Draft SQL and analyze user-provided data.",
        "DATA ENTRY",
    );
    let observations = Arc::new(Mutex::new(Vec::new()));
    let mut client = knowledge_work_client(
        KnowledgeWorkMockModel {
            observations: observations.clone(),
        },
        &root,
        &builtin_root,
        &["knowledge-work".to_string()],
    );
    client
        .initialize("knowledge-work-workflow-test", "0.1")
        .await
        .unwrap();
    let mut input = TurnInput::new(TurnInputMode::Start, "使用 data 工作流准备分析");
    input.workflow = Some(TurnWorkflow {
        kind: TurnWorkflowKind::SkillGroup,
        id: "knowledge-work".to_string(),
        mode: TurnWorkflowMode::Auto,
    });
    let (result, events) = run_turn_input(&mut client, input).await;

    assert_eq!(result.status, mini_agent_protocol::TurnStatus::Completed);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::SkillGroupActivated { group, source }
            if group == "knowledge-work" && source == "builtin"
    )));
    {
        let observations = observations.lock().unwrap();
        let system_prompt = &observations.last().unwrap().system_prompt;
        assert!(system_prompt.contains("## Active skill group: knowledge-work"));
        assert!(!system_prompt.contains("Active skill group: pstack"));
    }
    client.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn disabled_knowledge_work_group_fails_closed_before_model_execution() {
    let root = test_root("knowledge-work-disabled");
    let builtin_root = root.join("builtin");
    write_builtin_skill(
        &builtin_root,
        "data",
        "Draft SQL and analyze user-provided data.",
        "DATA ENTRY",
    );
    let observations = Arc::new(Mutex::new(Vec::new()));
    let mut client = knowledge_work_client(
        KnowledgeWorkMockModel {
            observations: observations.clone(),
        },
        &root,
        &builtin_root,
        &[],
    );
    client
        .initialize("knowledge-work-disabled-test", "0.1")
        .await
        .unwrap();
    let mut input = TurnInput::new(TurnInputMode::Start, "分析数据");
    input.selected_skills = vec!["knowledge-work:data".to_string()];
    let (result, events) = run_turn_input(&mut client, input).await;

    assert_eq!(result.status, mini_agent_protocol::TurnStatus::Failed);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::SkillsLoadFailed {
            activation,
            skills,
            reason_code,
            ..
        } if activation.as_deref() == Some("explicit")
            && reason_code == "activation_rejected"
            && skills == &["knowledge-work:data".to_string()]
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::RunStarted { .. }))
    );
    assert!(observations.lock().unwrap().is_empty());
    client.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn projects_structured_approval_denial_through_public_app_server() {
    let harness = Harness::new(
        ApprovalModel,
        ToolRouter::new(vec![Box::new(SensitiveFixtureTool)]),
        HarnessConfig::default(),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let received = run_turn_to_finished(&server, "run the fixture").await;

    assert!(received.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            call_id,
            content,
            is_error: true,
            outcome: Some(ToolExecutionStatus::NeedsApproval),
            truncated: false,
            ..
        } if call_id == "approval-call" && content == "user denied: sensitive fixture"
    )));
    assert!(received.iter().any(|event| matches!(
        event,
        Event::TurnFinished {
            status: mini_agent_protocol::TurnStatus::Completed
        }
    )));

    let checkpoint = server
        .thread_read_for(ThreadId::new("thread-1"))
        .await
        .unwrap();
    assert!(checkpoint.session.messages().iter().any(|message| {
        matches!(
            message,
            Message::Tool {
                content,
                is_error: true,
                outcome: Some(ToolExecutionStatus::NeedsApproval),
                ..
            } if content == "user denied: sensitive fixture"
        )
    }));
    assert!(checkpoint.session.messages().iter().any(|message| {
        matches!(
            message,
            Message::Assistant { text, tool_calls, .. }
                if text == "denial received" && tool_calls.is_empty()
        )
    }));
}

#[tokio::test]
async fn projects_mcp_timeout_through_public_app_server() {
    let harness = Harness::new(
        McpTimeoutModel,
        ToolRouter::new(vec![Box::new(McpTimeoutFixtureTool)]),
        HarnessConfig::default(),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let received = run_turn_to_finished(&server, "call the MCP tool").await;

    assert!(received.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            call_id,
            name,
            content,
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
            truncated: false,
            ..
        } if call_id == "mcp-timeout-call"
            && name == "mcp__fixture__slow"
            && content == "MCP tool call timed out"
    )));
    assert!(received.iter().any(|event| matches!(
        event,
        Event::TurnFinished {
            status: mini_agent_protocol::TurnStatus::Completed
        }
    )));

    let checkpoint = server
        .thread_read_for(ThreadId::new("thread-1"))
        .await
        .unwrap();
    assert!(checkpoint.session.messages().iter().any(|message| {
        matches!(
            message,
            Message::Tool {
                call_id,
                name,
                content,
                is_error: true,
                outcome: Some(ToolExecutionStatus::Failed),
                ..
            } if call_id == "mcp-timeout-call"
                && name == "mcp__fixture__slow"
                && content == "MCP tool call timed out"
        )
    }));
    assert!(checkpoint.session.messages().iter().any(|message| {
        matches!(
            message,
            Message::Assistant { text, tool_calls, .. }
                if text == "timeout received" && tool_calls.is_empty()
        )
    }));
}

#[tokio::test]
async fn local_client_exports_bounded_redacted_trace() {
    let server = server(DoneModel);
    let mut client = LocalAppServerClient::new(AppServerConnection::new(server));
    client.initialize("trace-test", "0").await.unwrap();
    let mut bytes = Vec::new();
    let mut trace = JsonlTrace::new("trace-1", &mut bytes).unwrap();

    client
        .run_turn_batch("secret prompt", &mut trace)
        .await
        .unwrap();
    let _ = trace.finish().unwrap();

    let output = String::from_utf8(bytes).unwrap();
    assert!(!output.contains("secret prompt"));
    let records = output
        .lines()
        .map(|line| from_str::<super::TraceRecord>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(records.iter().any(|record| record.event == "model_started"));
    let model_started = records
        .iter()
        .find(|record| record.event == "model_started")
        .unwrap();
    assert_eq!(model_started.round_index, 1);
    assert!(model_started.input_bytes.is_some());
    assert!(model_started.input_hash.is_some());
    assert!(model_started.tool_manifest_hash.is_some());
    assert!(records.iter().any(|record| record.event == "turn_finished"));
}

#[tokio::test]
async fn applies_thread_updates_without_exposing_the_harness_to_clients() {
    let server = server(DoneModel);
    server
        .thread_update_for(
            ThreadId::new("thread-1"),
            ThreadUpdate::AppendContext("host context".to_string()),
        )
        .await
        .unwrap();
    let checkpoint = server
        .thread_read_for(ThreadId::new("thread-1"))
        .await
        .unwrap();
    assert_eq!(
        checkpoint.session.messages(),
        &[mini_agent_protocol::Message::Context {
            text: "host context".to_string()
        }]
    );

    server
        .thread_update_for(ThreadId::new("thread-1"), ThreadUpdate::ClearHistory)
        .await
        .unwrap();
    assert!(
        server
            .thread_read_for(ThreadId::new("thread-1"))
            .await
            .unwrap()
            .session
            .messages()
            .is_empty()
    );
}

#[tokio::test]
async fn routes_follow_up_steer_and_cancel_while_turn_is_running() {
    let release = Arc::new(Notify::new());
    let server = server(BlockingModel {
        release: release.clone(),
    });
    let mut events = server.subscribe();
    let started = server
        .turn_start_for(
            ThreadId::new("thread-1"),
            TurnStart::new(TurnInput::new(TurnInputMode::Start, "long task")),
        )
        .await
        .unwrap();
    let turn_id = match started {
        TurnSubmission::Started { turn_id } => turn_id,
        other => panic!("unexpected submission: {other:?}"),
    };
    while !matches!(
        events.recv().await.unwrap().event,
        Event::ModelStarted { .. }
    ) {}

    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::FollowUp, "later")),
            )
            .await
            .unwrap(),
        TurnSubmission::Queued
    );
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Steer, "correct now")),
            )
            .await
            .unwrap(),
        TurnSubmission::Steered {
            turn_id: turn_id.clone()
        }
    );
    assert!(matches!(
        server
            .thread_fork(
                ThreadId::new("thread-1"),
                ThreadId::new("fork-while-running")
            )
            .await,
        Err(AppServerError::Busy)
    ));
    server
        .turn_cancel_for(ThreadId::new("thread-1"), TurnCancel::new(turn_id))
        .await
        .unwrap();
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::FollowUp, "too late")),
            )
            .await
            .unwrap(),
        TurnSubmission::NotSubmitted {
            reason: "turn is stopping; wait for turn_finished".to_string(),
        }
    );
    assert_eq!(
        server.runtime_status().phase,
        mini_agent_app_server_protocol::RuntimePhase::Stopping
    );
    release.notify_one();

    let mut statuses = Vec::new();
    for _ in 0..24 {
        if let Event::TurnFinished { status } = events.recv().await.unwrap().event {
            statuses.push(status);
            match statuses.len() {
                1 | 2 => release.notify_one(),
                3 => break,
                _ => {}
            }
        }
    }
    assert_eq!(
        statuses,
        [
            mini_agent_protocol::TurnStatus::Cancelled,
            mini_agent_protocol::TurnStatus::Completed,
            mini_agent_protocol::TurnStatus::Completed,
        ]
    );
    assert_eq!(
        server.runtime_status().phase,
        mini_agent_app_server_protocol::RuntimePhase::Completed
    );
}

#[tokio::test]
async fn interrupt_reaches_a_synchronous_tool_before_the_request_timeout() {
    let started = Arc::new(Notify::new());
    let server = blocking_tool_server(started.clone());
    let mut events = server.subscribe();
    let started_submission = server
        .turn_start_for(
            ThreadId::new("thread-1"),
            TurnStart::new(TurnInput::new(TurnInputMode::Start, "long shell")),
        )
        .await
        .unwrap();
    let turn_id = match started_submission {
        TurnSubmission::Started { turn_id } => turn_id,
        other => panic!("unexpected submission: {other:?}"),
    };

    started.notified().await;
    let cancel = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        server.turn_cancel_for(ThreadId::new("thread-1"), TurnCancel::new(turn_id.clone())),
    )
    .await
    .expect("interrupt should not wait for the blocking tool deadline");
    cancel.unwrap();

    let mut finished = None;
    while finished.is_none() {
        if let Event::TurnFinished { status } = events.recv().await.unwrap().event {
            finished = Some(status);
        }
    }
    assert_eq!(finished, Some(mini_agent_protocol::TurnStatus::Cancelled));
}

#[tokio::test]
async fn rejects_idle_steer_and_cancel_without_starting_a_second_loop() {
    let server = server(DoneModel);
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Steer, "invalid")),
            )
            .await,
        Err(AppServerError::InvalidInputMode(TurnInputMode::Steer))
    );
    assert_eq!(
        server
            .turn_cancel_for(
                ThreadId::new("thread-1"),
                TurnCancel::new(mini_agent_protocol::TurnId::new("turn-1")),
            )
            .await,
        Err(AppServerError::NoActiveTurn)
    );
}

#[tokio::test]
async fn exposes_a_restored_core_checkpoint_without_replaying_the_first_turn() {
    let mut initial = thread(ThreadId::new("thread-1"), DoneModel);
    initial
        .run_turn(
            TurnInput::new(TurnInputMode::Start, "first"),
            &mut (),
            &mini_agent_core::RunControl::new(),
            mini_agent_core::SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap();
    let checkpoint = initial.checkpoint().unwrap();

    let mut restored = thread(ThreadId::new("placeholder"), DoneModel);
    restored.restore_checkpoint(checkpoint).unwrap();
    let server = AppServer::new(ThreadStart::new(ThreadId::new("thread-1")), restored);
    let mut events = server.subscribe();

    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Start, "second")),
            )
            .await
            .unwrap(),
        TurnSubmission::Started {
            turn_id: mini_agent_protocol::TurnId::new("turn-2")
        }
    );

    let mut turn_ids = Vec::new();
    for _ in 0..6 {
        let event = events.recv().await.unwrap();
        if matches!(event.event, Event::TurnStarted { .. }) {
            turn_ids.push(event.turn_id);
        }
    }
    assert_eq!(turn_ids, [Some(mini_agent_protocol::TurnId::new("turn-2"))]);
}

#[tokio::test]
async fn routes_multiple_preconfigured_threads_by_identity() {
    let first = harness(DoneModel);
    let second = harness(DoneModel);
    let server = AppServer::with_threads(
        ThreadStart::new(ThreadId::new("thread-1")),
        vec![
            Thread::new(ThreadId::new("placeholder"), first),
            Thread::new(ThreadId::new("thread-2"), second),
        ],
    );
    assert_eq!(
        server.thread_ids(),
        vec![ThreadId::new("thread-1"), ThreadId::new("thread-2")]
    );
    let mut events = server.subscribe();
    let submission = server
        .turn_start_for(
            ThreadId::new("thread-2"),
            TurnStart::new(TurnInput::new(TurnInputMode::Start, "second")),
        )
        .await
        .unwrap();
    assert_eq!(
        submission,
        TurnSubmission::Started {
            turn_id: mini_agent_protocol::TurnId::new("turn-1")
        }
    );
    for _ in 0..6 {
        assert_eq!(
            events.recv().await.unwrap().thread_id,
            ThreadId::new("thread-2")
        );
    }
    assert_eq!(
        server
            .thread_read_for(ThreadId::new("thread-2"))
            .await
            .unwrap()
            .thread_id,
        ThreadId::new("thread-2")
    );
}

#[tokio::test]
async fn factory_supports_dynamic_start_fork_and_resume() {
    let initial = harness(DoneModel);
    let server = AppServer::with_thread_factory(
        ThreadStart::new(ThreadId::new("thread-1")),
        vec![Thread::new(ThreadId::new("placeholder"), initial)],
        |id| Ok(Thread::new(id, harness(DoneModel))),
    );
    assert_eq!(
        server
            .thread_start(ThreadId::new("thread-2"))
            .await
            .unwrap(),
        ThreadId::new("thread-2")
    );
    let mut events = server.subscribe();
    server
        .turn_start_for(
            ThreadId::new("thread-1"),
            TurnStart::new(TurnInput::new(TurnInputMode::Start, "seed")),
        )
        .await
        .unwrap();
    for _ in 0..6 {
        let _ = events.recv().await.unwrap();
    }
    assert_eq!(
        server
            .thread_fork(ThreadId::new("thread-1"), ThreadId::new("thread-3"))
            .await
            .unwrap(),
        ThreadId::new("thread-3")
    );
    let checkpoint = server
        .thread_read_for(ThreadId::new("thread-3"))
        .await
        .unwrap();
    assert_eq!(
        server
            .thread_resume(ThreadId::new("thread-3"), checkpoint)
            .await
            .unwrap(),
        ThreadId::new("thread-3")
    );
    assert!(server.has_thread(&ThreadId::new("thread-3")));
}
