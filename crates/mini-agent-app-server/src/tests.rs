use super::AppServer;
use super::AppServerConnection;
use super::AppServerError;
use super::ApprovalBroker;
use super::ApprovalEvent;
use super::JsonlTrace;
use super::LocalAppServerClient;
use super::ThreadUpdate;
use super::worker::Command;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRegistry;
use mini_agent_protocol::Event;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolSpec;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use mini_agent_protocol::TurnSubmission;
use serde_json::Value;
use serde_json::from_str;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
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

impl Tool for SensitiveFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sensitive_fixture".to_string(),
            description: "A fixture that requires approval".to_string(),
            parameters: json!({"type": "object"}),
        }
    }

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

impl Tool for McpTimeoutFixtureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp__fixture__slow".to_string(),
            description: "A fixture that times out like an MCP call".to_string(),
            parameters: json!({"type": "object"}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("MCP tool call timed out".to_string()))
    }

    fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
        ToolExecutionOutcome::failed("MCP tool call timed out")
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

pub(crate) fn server<M: Model + Send + 'static>(model: M) -> AppServer<M> {
    let harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());
    AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    )
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
async fn projects_structured_approval_denial_through_public_app_server() {
    let harness = Harness::new(
        ApprovalModel,
        ToolRegistry::new(vec![Box::new(SensitiveFixtureTool)]),
        HarnessConfig::default(),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let mut events = server.subscribe();
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Start, "run the fixture")),
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
        ToolRegistry::new(vec![Box::new(McpTimeoutFixtureTool)]),
        HarnessConfig::default(),
    );
    let server = AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    );
    let mut events = server.subscribe();
    assert_eq!(
        server
            .turn_start_for(
                ThreadId::new("thread-1"),
                TurnStart::new(TurnInput::new(TurnInputMode::Start, "call the MCP tool")),
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

    assert!(received.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            call_id,
            name,
            content,
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
            truncated: false,
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
    server
        .turn_cancel_for(ThreadId::new("thread-1"), TurnCancel::new(turn_id))
        .await
        .unwrap();
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
    let initial_harness =
        Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let mut initial = Thread::new(ThreadId::new("thread-1"), initial_harness);
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

    let replacement = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let mut restored = Thread::new(ThreadId::new("placeholder"), replacement);
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
    let first = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let second = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
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
async fn approval_broker_round_trips_a_synchronous_host_callback() {
    let broker = ApprovalBroker::new();
    let requester = broker.clone();
    let task = tokio::task::spawn_blocking(move || requester.request("shell command `pwd`"));
    let request = broker.next_request().await;
    assert_eq!(request.action, "shell command `pwd`");
    broker.respond(&request.request_id, true).unwrap();
    assert!(task.await.unwrap().unwrap());
}

#[tokio::test]
async fn approval_broker_exposes_request_and_resolution_events() {
    let broker = ApprovalBroker::new();
    let requester = broker.clone();
    let task = tokio::task::spawn_blocking(move || requester.request("shell command pwd"));

    let request = match broker.next_event().await {
        ApprovalEvent::Requested(request) => request,
        ApprovalEvent::Resolved(_) => panic!("expected approval request"),
    };
    broker.respond(&request.request_id, true).unwrap();
    let resolution = match broker.next_event().await {
        ApprovalEvent::Resolved(resolution) => resolution,
        ApprovalEvent::Requested(_) => panic!("expected approval resolution"),
    };

    assert_eq!(resolution.action, "shell command pwd");
    assert!(resolution.approved);
    assert!(task.await.unwrap().unwrap());
}

#[tokio::test]
async fn factory_supports_dynamic_start_fork_and_resume() {
    let initial = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let server = AppServer::with_thread_factory(
        ThreadStart::new(ThreadId::new("thread-1")),
        vec![Thread::new(ThreadId::new("placeholder"), initial)],
        |id| {
            Ok(Thread::new(
                id,
                Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default()),
            ))
        },
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
