use super::AppServer;
use super::AppServerError;
use super::ApprovalBroker;
use super::ThreadUpdate;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ToolRegistry;
use mini_agent_protocol::Event;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use mini_agent_protocol::TurnSubmission;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::Notify;

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

struct BlockingModel {
    release: Arc<Notify>,
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

fn server<M: Model + Send + 'static>(model: M) -> AppServer<M> {
    let harness = Harness::new(model, ToolRegistry::default(), HarnessConfig::default());
    AppServer::new(
        ThreadStart::new(ThreadId::new("thread-1")),
        Thread::new(ThreadId::new("initial"), harness),
    )
}

#[tokio::test]
async fn starts_turn_and_broadcasts_core_lifecycle_events() {
    let server = server(DoneModel);
    let mut events = server.subscribe();
    let submission = server
        .turn_start(TurnStart::new(TurnInput::new(
            TurnInputMode::Start,
            "inspect",
        )))
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
        .turn_start(TurnStart::new(TurnInput::new(
            TurnInputMode::StartIfIdle,
            "again",
        )))
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
async fn applies_thread_updates_without_exposing_the_harness_to_clients() {
    let server = server(DoneModel);
    server
        .thread_update(ThreadUpdate::AppendContext("host context".to_string()))
        .await
        .unwrap();
    let checkpoint = server.thread_read().await.unwrap();
    assert_eq!(
        checkpoint.session.messages(),
        &[mini_agent_protocol::Message::Context {
            text: "host context".to_string()
        }]
    );

    server
        .thread_update(ThreadUpdate::ClearHistory)
        .await
        .unwrap();
    assert!(
        server
            .thread_read()
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
        .turn_start(TurnStart::new(TurnInput::new(
            TurnInputMode::Start,
            "long task",
        )))
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
            .turn_start(TurnStart::new(TurnInput::new(
                TurnInputMode::FollowUp,
                "later",
            )))
            .await
            .unwrap(),
        TurnSubmission::Queued
    );
    assert_eq!(
        server
            .turn_start(TurnStart::new(TurnInput::new(
                TurnInputMode::Steer,
                "correct now",
            )))
            .await
            .unwrap(),
        TurnSubmission::Steered {
            turn_id: turn_id.clone()
        }
    );
    server.turn_cancel(TurnCancel::new(turn_id)).await.unwrap();
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
            .turn_start(TurnStart::new(TurnInput::new(
                TurnInputMode::Steer,
                "invalid",
            )))
            .await,
        Err(AppServerError::InvalidInputMode(TurnInputMode::Steer))
    );
    assert_eq!(
        server
            .turn_cancel(TurnCancel::new(mini_agent_protocol::TurnId::new("turn-1")))
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
            .turn_start(TurnStart::new(TurnInput::new(
                TurnInputMode::Start,
                "second",
            )))
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
