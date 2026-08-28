use super::Thread;
use crate::Harness;
use crate::HarnessConfig;
use crate::Model;
use crate::ModelEventSink;
use crate::ModelRequest;
use crate::ModelResponse;
use crate::RunControl;
use crate::SteeringMode;
use crate::ThreadError;
use crate::ThreadId;
use crate::ThreadStatus;
use crate::ToolRegistry;
use crate::TurnInput;
use crate::TurnInputMode;
use crate::TurnStatus;
use std::convert::Infallible;

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

#[tokio::test]
async fn thread_assigns_turn_ids_and_returns_to_idle() {
    let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let mut thread = Thread::new(ThreadId::new("thread-1"), harness);
    let control = RunControl::new();

    let first = thread
        .run_turn(
            TurnInput::new(TurnInputMode::Start, "first"),
            &mut (),
            &control,
            SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap();
    let second = thread
        .run_turn(
            TurnInput::new(TurnInputMode::StartIfIdle, "second"),
            &mut (),
            &control,
            SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap();

    assert_eq!(thread.id(), &ThreadId::new("thread-1"));
    assert_eq!(thread.status(), ThreadStatus::Idle);
    assert_eq!(first.id.as_str(), "turn-1");
    assert_eq!(first.status, TurnStatus::Completed);
    assert_eq!(second.id.as_str(), "turn-2");
}

#[tokio::test]
async fn thread_rejects_queued_input_as_a_new_turn_and_stays_usable() {
    let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let mut thread = Thread::new(ThreadId::new("thread-1"), harness);
    let control = RunControl::new();

    let error = thread
        .run_turn(
            TurnInput::new(TurnInputMode::Steer, "correct"),
            &mut (),
            &control,
            SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ThreadError::InvalidInputMode(TurnInputMode::Steer)
    ));
    assert_eq!(thread.status(), ThreadStatus::Idle);
    thread.close().unwrap();
    assert_eq!(thread.status(), ThreadStatus::Closed);
}
