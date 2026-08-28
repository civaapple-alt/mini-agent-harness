use super::Thread;
use crate::Event;
use crate::EventEnvelope;
use crate::EventSink;
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
use crate::Tool;
use crate::ToolCall;
use crate::ToolError;
use crate::ToolRegistry;
use crate::ToolSpec;
use crate::TurnCancel;
use crate::TurnId;
use crate::TurnInput;
use crate::TurnInputMode;
use crate::TurnStatus;
use serde_json::Value;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

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

#[derive(Default)]
struct RecordingSink(Vec<EventEnvelope>);

impl EventSink for RecordingSink {
    fn emit(&mut self, event: EventEnvelope) {
        self.0.push(event);
    }
}

struct ToolBatchModel;

impl Model for ToolBatchModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        Ok(ModelResponse {
            reasoning: String::new(),
            text: "tools".to_string(),
            tool_calls: vec![
                ToolCall {
                    id: "call-1".to_string(),
                    name: "count".to_string(),
                    arguments: json!({}),
                },
                ToolCall {
                    id: "call-2".to_string(),
                    name: "count".to_string(),
                    arguments: json!({}),
                },
            ],
            usage: None,
        })
    }
}

struct CancelingTool {
    control: RunControl,
    executions: Arc<AtomicUsize>,
}

impl Tool for CancelingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "count".to_string(),
            description: "counts calls".to_string(),
            parameters: json!({"type": "object"}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        self.executions.fetch_add(1, Ordering::Relaxed);
        self.control.request_cancel();
        Ok("counted".to_string())
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
    assert!(matches!(
        thread.cancel_turn(TurnCancel::new(TurnId::new("turn-1")), &control),
        Err(ThreadError::NoActiveTurn)
    ));
    thread.close().unwrap();
    assert_eq!(thread.status(), ThreadStatus::Closed);
}

#[tokio::test]
async fn thread_cancellation_emits_ordered_events_and_settles() {
    let harness = Harness::new(DoneModel, ToolRegistry::default(), HarnessConfig::default());
    let mut thread = Thread::new(ThreadId::new("thread-1"), harness);
    let control = RunControl::new();
    control.request_cancel();
    let mut sink = RecordingSink::default();

    let result = thread
        .run_turn_with_events(
            TurnInput::new(TurnInputMode::Start, "cancel"),
            &mut sink,
            &control,
            SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap();

    assert_eq!(result.status, TurnStatus::Cancelled);
    assert_eq!(result.outcome.stop_reason, crate::StopReason::Cancelled);
    assert_eq!(thread.status(), ThreadStatus::Idle);
    assert_eq!(sink.0.len(), 2);
    assert_eq!(sink.0[0].sequence, 1);
    assert_eq!(sink.0[1].sequence, 2);
    assert_eq!(sink.0[0].thread_id, ThreadId::new("thread-1"));
    assert_eq!(sink.0[0].turn_id, Some(TurnId::new("turn-1")));
    assert!(matches!(sink.0[0].event, Event::RunStarted { .. }));
    assert!(matches!(
        sink.0[1].event,
        Event::RunFinished {
            stop_reason: crate::StopReason::Cancelled,
            ..
        }
    ));
}

#[tokio::test]
async fn cancellation_waits_for_a_complete_tool_batch() {
    let control = RunControl::new();
    let executions = Arc::new(AtomicUsize::new(0));
    let tool = CancelingTool {
        control: control.clone(),
        executions: executions.clone(),
    };
    let harness = Harness::new(
        ToolBatchModel,
        ToolRegistry::new(vec![Box::new(tool)]),
        HarnessConfig::default(),
    );
    let mut thread = Thread::new(ThreadId::new("thread-1"), harness);

    let result = thread
        .run_turn(
            TurnInput::new(TurnInputMode::Start, "cancel after model"),
            &mut (),
            &control,
            SteeringMode::StopAtCheckpoint,
        )
        .await
        .unwrap();

    assert_eq!(result.status, TurnStatus::Cancelled);
    assert_eq!(executions.load(Ordering::Relaxed), 2);
}
