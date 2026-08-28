use super::AppServer;
use super::AppServerError;
use mini_agent_core::Event;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_core::ToolRegistry;
use mini_agent_core::TurnCancel;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStart;
use mini_agent_core::TurnSubmission;
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
            status: mini_agent_core::TurnStatus::Completed
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
            turn_id: mini_agent_core::TurnId::new("turn-2")
        }
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
            mini_agent_core::TurnStatus::Cancelled,
            mini_agent_core::TurnStatus::Completed,
            mini_agent_core::TurnStatus::Completed,
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
            .turn_cancel(TurnCancel::new(mini_agent_core::TurnId::new("turn-1")))
            .await,
        Err(AppServerError::NoActiveTurn)
    );
}
