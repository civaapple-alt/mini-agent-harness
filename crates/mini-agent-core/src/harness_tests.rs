use super::*;
use crate::context_controller::COMPACTION_PREFIX;
use crate::context_controller::assemble_compacted;
use crate::context_controller::bounded_compaction_prompt;
use crate::context_controller::compaction_prompt;
use crate::context_controller::split_prefix_tail;
use crate::context_controller::trim_prefix_to_fit;
use crate::tool_batch_executor::truncate_utf8;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelUsage;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolError;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolExecutionStatus;
use mini_agent_protocol::ToolHandler;
use mini_agent_protocol::ToolReplaySafety;
use mini_agent_protocol::ToolRuntime;
use mini_agent_protocol::ToolSpec;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ScriptedModel {
    responses: VecDeque<ModelResponse>,
}

impl Model for ScriptedModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        Ok(self
            .responses
            .pop_front()
            .expect("missing scripted response"))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RecordedRequest {
    system_prompt: String,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
}

struct RecordingModel {
    responses: VecDeque<ModelResponse>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl Model for RecordingModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.requests.lock().unwrap().push(RecordedRequest {
            system_prompt: request.system_prompt.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.to_vec(),
        });
        Ok(self
            .responses
            .pop_front()
            .expect("missing recorded response"))
    }
}

#[derive(Debug)]
struct ContextWindowExceeded;

impl std::fmt::Display for ContextWindowExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("provider context window exceeded")
    }
}

impl std::error::Error for ContextWindowExceeded {}

struct ContextWindowRetryModel {
    responses: VecDeque<Result<ModelResponse, ContextWindowExceeded>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl Model for ContextWindowRetryModel {
    type Error = ContextWindowExceeded;

    fn is_context_window_error(&self, _error: &Self::Error) -> bool {
        true
    }

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.requests.lock().unwrap().push(RecordedRequest {
            system_prompt: request.system_prompt.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.to_vec(),
        });
        self.responses
            .pop_front()
            .expect("missing context-window scenario response")
    }
}

fn text_response(text: impl Into<String>) -> ModelResponse {
    ModelResponse {
        reasoning: String::new(),
        text: text.into(),
        tool_calls: Vec::new(),
        usage: None,
    }
}

fn tool_response(call_id: &str, name: &str, arguments: Value) -> ModelResponse {
    ModelResponse {
        reasoning: String::new(),
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments,
        }],
        usage: None,
    }
}

struct SubmitSteerBeforeToolBatch {
    control: RunControl,
}

impl Model for SubmitSteerBeforeToolBatch {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        _request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.control
            .submit(TurnInput::new(
                TurnInputMode::Steer,
                "focus after the pending tool call",
            ))
            .unwrap();
        Ok(tool_response(
            "call-before-steer",
            "request_steer",
            json!({}),
        ))
    }
}

struct Uppercase;

impl ToolHandler for Uppercase {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "uppercase".to_string(),
            description: "Convert text to uppercase".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }
}

impl ToolRuntime for Uppercase {
    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("text must be a string".to_string()))?;
        Ok(text.to_uppercase())
    }
}

struct ApprovalTool;

impl ToolHandler for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "needs_approval".to_string(),
            description: "A tool whose host policy requires approval".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for ApprovalTool {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError("approval required".to_string()))
    }

    fn execute_outcome(&self, _arguments: &Value) -> ToolExecutionOutcome {
        ToolExecutionOutcome {
            status: ToolExecutionStatus::NeedsApproval,
            content: "approval required".to_string(),
            output_truncated: false,
            context_messages: Vec::new(),
            context_injections: Vec::new(),
        }
    }
}

#[tokio::test]
async fn runs_model_tool_model_path() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({"text": "quiet"}),
                }],
                usage: Some(ModelUsage {
                    input_tokens: 10,
                    cached_input_tokens: Some(2),
                    output_tokens: 3,
                }),
            },
            ModelResponse {
                reasoning: String::new(),
                text: "The result is QUIET.".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let tools = ToolRouter::new(vec![Box::new(Uppercase)]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    let outcome = harness
        .run("make it loud", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "The result is QUIET.");
    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "uppercase".to_string(),
            content: "QUIET".to_string(),
            is_error: false,
            outcome: Some(ToolExecutionStatus::Completed),
        }
    );
    assert_eq!(
        events.last(),
        Some(&Event::RunFinished {
            stop_reason: StopReason::Completed,
            steps: 2,
        })
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ModelResponded {
            usage: Some(ModelUsage {
                input_tokens: 10,
                cached_input_tokens: Some(2),
                output_tokens: 3,
            }),
            ..
        }
    )));
}

#[tokio::test]
async fn scripted_50_turn_goal_keeps_tool_manifest_stable() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: (0..50)
            .map(|turn| text_response(format!("goal checkpoint {turn}")))
            .collect(),
        requests: requests.clone(),
    };
    let mut harness = Harness::new(
        model,
        ToolRouter::new(vec![Box::new(Uppercase)]),
        HarnessConfig::default(),
    );
    let mut events = Vec::new();
    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    for turn in 0..50 {
        harness
            .run(
                format!("continue scripted Goal step {turn}"),
                &mut Recorder(&mut events),
            )
            .await
            .unwrap();
    }

    let hashes = events
        .iter()
        .filter_map(|event| match event {
            Event::ModelStarted {
                tool_manifest_hash, ..
            } => Some(tool_manifest_hash),
            _ => None,
        })
        .collect::<Vec<_>>();
    let recorded = requests.lock().unwrap();
    let serialized_specs = recorded
        .iter()
        .map(|request| serde_json::to_vec(&request.tools).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(hashes.len(), 50);
    assert!(hashes.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(serialized_specs.len(), 50);
    assert!(serialized_specs.windows(2).all(|pair| pair[0] == pair[1]));
}

struct Shell127Fixture;

impl ToolHandler for Shell127Fixture {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: "Run a scripted command fixture".to_string(),
            parameters: json!({"type": "object"}),
        }
    }
}

impl ToolRuntime for Shell127Fixture {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        Err(ToolError(
            "command exited with code 127: mini-agent-missing-command: not found".to_string(),
        ))
    }
}

struct AdaptAfterShell127 {
    seen_error: Arc<Mutex<bool>>,
}

impl Model for AdaptAfterShell127 {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let failure = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Tool {
                    name,
                    content,
                    is_error: true,
                    ..
                } if name == "shell" => Some(content),
                _ => None,
            });
        if let Some(failure) = failure {
            assert!(failure.contains("code 127"));
            *self.seen_error.lock().unwrap() = true;
            return Ok(text_response(
                "Shell was unavailable; switched to read-only inspection.",
            ));
        }
        Ok(tool_response(
            "shell-127",
            "shell",
            json!({"command": "fixture"}),
        ))
    }
}

#[tokio::test]
async fn shell_exit_127_error_evidence_drives_the_next_strategy() {
    let seen_error = Arc::new(Mutex::new(false));
    let mut harness = Harness::new(
        AdaptAfterShell127 {
            seen_error: seen_error.clone(),
        },
        ToolRouter::new(vec![Box::new(Shell127Fixture)]),
        HarnessConfig::default(),
    );
    let mut events = Vec::new();
    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    let outcome = harness
        .run("inspect the workspace", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert!(*seen_error.lock().unwrap());
    assert_eq!(
        outcome.final_text,
        "Shell was unavailable; switched to read-only inspection."
    );
    assert!(outcome.messages.iter().any(|message| matches!(
        message,
        Message::Tool { name, content, is_error: true, .. }
            if name == "shell" && content.contains("code 127")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolFinished { name, content, is_error: true, .. }
            if name == "shell" && content.contains("code 127")
    )));
}

#[tokio::test]
async fn resumes_from_checkpoint_with_the_durable_tool_result_without_replaying_it() {
    struct RecordingExecutionJournal(Vec<crate::ExecutionJournalEntry>);
    impl crate::ExecutionJournalSink for RecordingExecutionJournal {
        fn append(&mut self, entry: crate::ExecutionJournalEntry) -> Result<u64, String> {
            self.0.push(entry);
            Ok(self.0.len() as u64)
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([text_response("resumed answer")]),
        requests: Arc::clone(&requests),
    };
    let mut harness = Harness::new(
        model,
        ToolRouter::new(vec![Box::new(Uppercase)]),
        HarnessConfig::default(),
    );
    let input = TurnInput::new(TurnInputMode::Start, "continue this work");
    let turn_id = mini_agent_protocol::TurnId::new("turn-recovery");
    let call = ToolCall {
        id: "call-recovered".to_string(),
        name: "uppercase".to_string(),
        arguments: json!({"text": "quiet"}),
    };
    let checkpoint = crate::ExecutionCheckpoint {
        turn_id: turn_id.clone(),
        input: input.clone(),
        messages: vec![Message::User {
            text: input.text.clone(),
        }],
        next_model_step: 1,
        final_text: String::new(),
        phase: crate::ExecutionPhase::ModelRequest,
        applied_steer_request_ids: Vec::new(),
    };
    let batch = crate::ExecutionToolBatch {
        intent: crate::ToolBatchIntent {
            turn_id: turn_id.clone(),
            step: 1,
            reasoning: String::new(),
            text: String::new(),
            calls: vec![call.clone()],
        },
        calls: vec![crate::ExecutionToolCall {
            call,
            started: true,
            outcome: Some(ToolExecutionOutcome::completed("durable tool result")),
        }],
    };
    let mut journal = RecordingExecutionJournal(Vec::new());
    let mut events = RecordingObserver::default();

    let outcome = harness
        .run_with_control_mode_and_tool_context(
            input.text.clone(),
            &mut events,
            &RunControl::new(),
            SteeringMode::StopAtCheckpoint,
            crate::ExecutionRunOptions {
                execution_context: Some(crate::ExecutionRunContext {
                    turn_id: turn_id.clone(),
                    input,
                    applied_steer_request_ids: Vec::new(),
                }),
                journal: Some(&mut journal),
                resume: Some((checkpoint, Some(batch))),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.final_text, "resumed answer");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        Message::Tool { content, .. } if content == "durable tool result"
    )));
    assert!(!requests[0].messages.iter().any(|message| matches!(
        message,
        Message::Tool { content, .. } if content == "QUIET"
    )));
    assert!(journal.0.iter().any(|entry| matches!(
        entry,
        crate::ExecutionJournalEntry::Checkpoint { checkpoint }
            if checkpoint.turn_id == turn_id && checkpoint.next_model_step == 2
    )));
}

#[tokio::test]
async fn does_not_replay_an_uncertain_side_effecting_tool_call() {
    struct RecordingExecutionJournal(Vec<crate::ExecutionJournalEntry>);
    impl crate::ExecutionJournalSink for RecordingExecutionJournal {
        fn append(&mut self, entry: crate::ExecutionJournalEntry) -> Result<u64, String> {
            self.0.push(entry);
            Ok(self.0.len() as u64)
        }
    }

    struct SideEffect(Arc<AtomicUsize>);
    impl ToolHandler for SideEffect {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "side_effect".to_string(),
                description: "A non-replayable side effect".to_string(),
                parameters: json!({"type": "object"}),
            }
        }
    }
    impl ToolRuntime for SideEffect {
        fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("effect performed".to_string())
        }

        fn recovery_replay_safety(&self, _request: &ToolExecutionRequest) -> ToolReplaySafety {
            ToolReplaySafety::Never
        }
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut harness = Harness::new(
        RecordingModel {
            responses: VecDeque::from([text_response("must not run yet")]),
            requests: Arc::clone(&requests),
        },
        ToolRouter::new(vec![Box::new(SideEffect(Arc::clone(&calls)))]),
        HarnessConfig::default(),
    );
    let call = ToolCall {
        id: "call-uncertain".to_string(),
        name: "side_effect".to_string(),
        arguments: json!({"destination": "receiver"}),
    };
    let input = TurnInput::new(TurnInputMode::Start, "continue the pending operation");
    let turn_id = mini_agent_protocol::TurnId::new("turn-uncertain");
    let checkpoint = crate::ExecutionCheckpoint {
        turn_id: turn_id.clone(),
        input: input.clone(),
        messages: vec![Message::User {
            text: input.text.clone(),
        }],
        next_model_step: 1,
        final_text: String::new(),
        phase: crate::ExecutionPhase::ToolBatch,
        applied_steer_request_ids: Vec::new(),
    };
    let batch = crate::ExecutionToolBatch {
        intent: crate::ToolBatchIntent {
            turn_id: turn_id.clone(),
            step: 1,
            reasoning: String::new(),
            text: String::new(),
            calls: vec![call.clone()],
        },
        calls: vec![crate::ExecutionToolCall {
            call,
            started: true,
            outcome: None,
        }],
    };
    let mut journal = RecordingExecutionJournal(Vec::new());
    let result = harness
        .run_with_control_mode_and_tool_context(
            input.text.clone(),
            &mut (),
            &RunControl::new(),
            SteeringMode::StopAtCheckpoint,
            crate::ExecutionRunOptions {
                execution_context: Some(crate::ExecutionRunContext {
                    turn_id,
                    input,
                    applied_steer_request_ids: Vec::new(),
                }),
                journal: Some(&mut journal),
                resume: Some((checkpoint, Some(batch))),
                ..Default::default()
            },
        )
        .await;

    assert!(matches!(result, Err(HarnessError::NeedsReconciliation(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(requests.lock().unwrap().is_empty());
    assert!(journal.0.iter().any(|entry| matches!(
        entry,
        crate::ExecutionJournalEntry::NeedsReconciliation { .. }
    )));
}

#[tokio::test]
async fn recovers_after_partial_tool_batch_without_erasing_completed_action() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "call-completed".to_string(),
                        name: "uppercase".to_string(),
                        arguments: json!({"text": "kept"}),
                    },
                    ToolCall {
                        id: "call-failed".to_string(),
                        name: "uppercase".to_string(),
                        arguments: json!({}),
                    },
                ],
                usage: None,
            },
            text_response("recovered after partial batch"),
        ]),
    };
    let mut harness = Harness::new(
        model,
        ToolRouter::new(vec![Box::new(Uppercase)]),
        HarnessConfig::default(),
    );
    let mut events = RecordingObserver::default();

    let outcome = harness
        .run("complete the bounded batch", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "recovered after partial batch");
    assert!(matches!(
        outcome.messages.get(2),
        Some(Message::Tool {
            call_id,
            content,
            is_error: false,
            outcome: Some(ToolExecutionStatus::Completed),
            ..
        }) if call_id == "call-completed" && content == "KEPT"
    ));
    assert!(matches!(
        outcome.messages.get(3),
        Some(Message::Tool {
            call_id,
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
            ..
        }) if call_id == "call-failed"
    ));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            call_id,
            outcome: Some(ToolExecutionStatus::Failed),
            is_error: true,
            ..
        } if call_id == "call-failed"
    )));
}

#[tokio::test]
async fn steering_stops_after_a_complete_tool_batch() {
    let control = RunControl::new();
    let model = ScriptedModel {
        responses: VecDeque::from([tool_response("call-1", "request_steer", json!({}))]),
    };
    let tools = ToolRouter::new(vec![Box::new(RequestSteer(control.clone()))]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());

    let outcome = harness
        .run_with_control("correct me", &mut (), &control)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Steered);
    assert_eq!(outcome.steps, 1);
    assert!(matches!(
        outcome.messages.last(),
        Some(Message::Tool { .. })
    ));
}

#[tokio::test]
async fn steering_after_model_tool_call_keeps_history_complete() {
    let control = RunControl::new();
    let model = SubmitSteerBeforeToolBatch {
        control: control.clone(),
    };
    let tools = ToolRouter::new(vec![Box::new(RequestSteer(control.clone()))]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());

    let outcome = harness
        .run_with_control("inspect the project", &mut (), &control)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Steered);
    assert!(matches!(
        outcome.messages.last(),
        Some(Message::Tool {
            call_id,
            outcome: Some(ToolExecutionStatus::Completed),
            ..
        }) if call_id == "call-before-steer"
    ));
}

#[tokio::test]
async fn repairs_legacy_tool_history_before_an_in_memory_retry() {
    let model = ScriptedModel {
        responses: VecDeque::from([text_response("recovered")]),
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());
    harness.session.push(Message::User {
        text: "inspect".to_string(),
    });
    harness.session.push(Message::Assistant {
        reasoning: String::new(),
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "call-legacy-orphan".to_string(),
            name: "shell".to_string(),
            arguments: json!({}),
        }],
    });

    let outcome = harness.run("retry", &mut ()).await.unwrap();

    assert_eq!(outcome.final_text, "recovered");
    assert!(harness.messages().iter().all(|message| {
        !matches!(
            message,
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty()
        )
    }));
}

struct SubmitSteerDuringSampling {
    control: RunControl,
    calls: usize,
}

impl Model for SubmitSteerDuringSampling {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let call = self.calls;
        self.calls = self.calls.saturating_add(1);
        if call == 0 {
            self.control
                .submit(TurnInput::new(
                    TurnInputMode::Steer,
                    "focus on the actual bug",
                ))
                .unwrap();
            return Ok(text_response("the first answer drifted"));
        }
        assert!(request.messages.iter().any(|message| matches!(
            message,
            Message::User { text } if text == "focus on the actual bug"
        )));
        Ok(text_response("the corrected answer"))
    }
}

#[tokio::test]
async fn same_turn_steering_consumes_input_after_sampling() {
    let control = RunControl::new();
    let model = SubmitSteerDuringSampling {
        control: control.clone(),
        calls: 0,
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());

    let outcome = harness
        .run_with_control_mode(
            "initial request",
            &mut (),
            &control,
            SteeringMode::ContinueSameTurn,
        )
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "the corrected answer");
    assert_eq!(
        outcome
            .messages
            .iter()
            .filter(|message| matches!(message, Message::User { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn returns_unknown_tool_failure_to_model() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            tool_response("call-1", "missing", json!({})),
            text_response("I could not run that tool."),
        ]),
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());

    let outcome = harness.run("try it", &mut ()).await.unwrap();

    assert_eq!(
        outcome.messages[2],
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "missing".to_string(),
            content: "unknown tool: missing".to_string(),
            is_error: true,
            outcome: Some(ToolExecutionStatus::Failed),
        }
    );
}

#[tokio::test]
async fn preserves_structured_tool_policy_outcome_in_events() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            tool_response("call-approval", "needs_approval", json!({})),
            text_response("waiting for approval"),
        ]),
    };
    let mut harness = Harness::new(
        model,
        ToolRouter::new(vec![Box::new(ApprovalTool)]),
        HarnessConfig::default(),
    );
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    harness
        .run("use the protected tool", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            is_error: true,
            outcome: Some(ToolExecutionStatus::NeedsApproval),
            ..
        }
    )));
    assert!(harness.messages().iter().any(|message| matches!(
        message,
        Message::Tool {
            outcome: Some(ToolExecutionStatus::NeedsApproval),
            is_error: true,
            ..
        }
    )));
}

#[tokio::test]
async fn records_tool_output_truncation_explicitly() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            tool_response("call-1", "uppercase", json!({"text": "abcdefghij"})),
            text_response("done"),
        ]),
    };
    let config = HarnessConfig {
        max_tool_output_bytes: 5,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::new(vec![Box::new(Uppercase)]), config);
    let mut events = Vec::new();

    struct Recorder<'a>(&'a mut Vec<Event>);
    impl Observer for Recorder<'_> {
        fn observe(&mut self, event: &Event) {
            self.0.push(event.clone());
        }
    }

    harness
        .run("produce long output", &mut Recorder(&mut events))
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            content,
            truncated: true,
            ..
        } if content == "ABCDE"
    )));
}

#[tokio::test]
async fn stops_at_step_limit() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "still working".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "missing".to_string(),
                arguments: json!({}),
            }],
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_steps: 1,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);

    let outcome = harness.run("continue forever", &mut ()).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::StepLimit);
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.final_text, "still working");
}

#[tokio::test]
async fn zero_step_limit_means_unlimited() {
    let model = ScriptedModel {
        responses: VecDeque::from([text_response("done")]),
    };
    let config = HarnessConfig {
        max_steps: 0,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);

    let outcome = harness.run("finish this", &mut ()).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.final_text, "done");
}

#[test]
fn copilot_loop_uses_compaction_and_unlimited_default() {
    let config = HarnessConfig::default().with_copilot_loop();

    assert_eq!(config.max_steps, 0);
    assert_eq!(config.context_limit_behavior, ContextLimitBehavior::Compact);
}

#[test]
fn default_harness_compacts_context() {
    assert_eq!(
        HarnessConfig::default().context_limit_behavior,
        ContextLimitBehavior::Compact
    );
}

#[tokio::test]
async fn preserves_history_across_runs_and_can_clear_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([
            ModelResponse {
                reasoning: String::new(),
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
            ModelResponse {
                reasoning: String::new(),
                text: "second answer".to_string(),
                tool_calls: Vec::new(),
                usage: None,
            },
        ]),
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());

    harness.run("first question", &mut ()).await.unwrap();
    let outcome = harness.run("second question", &mut ()).await.unwrap();

    assert_eq!(outcome.messages.len(), 4);
    assert_eq!(harness.messages(), outcome.messages);
    assert_eq!(
        outcome.messages,
        vec![
            Message::User {
                text: "first question".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "first answer".to_string(),
                tool_calls: Vec::new(),
            },
            Message::User {
                text: "second question".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: "second answer".to_string(),
                tool_calls: Vec::new(),
            },
        ]
    );

    harness.clear_history();
    assert!(harness.messages().is_empty());
}

#[test]
fn context_items_have_an_independent_hard_limit() {
    let config = HarnessConfig {
        max_context_item_bytes: 4,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRouter::default(),
        config,
    );

    let error = harness.append_context("12345").unwrap_err();

    assert_eq!(
        error,
        LimitExceeded {
            kind: LimitKind::ContextItemBytes,
            limit: 4,
            actual: 5,
        }
    );
    assert!(harness.messages().is_empty());
}

#[test]
fn host_context_injections_have_a_separate_bounded_item_limit() {
    let config = HarnessConfig {
        max_context_item_bytes: 4,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRouter::default(),
        config,
    );
    let body = "x".repeat(32 * 1024);
    let record = mini_agent_protocol::ContextInjectionRecord {
        id: "skill_definition_fixture".to_string(),
        kind: mini_agent_protocol::ContextInjectionKind::Skill,
        source: "Skill fixture".to_string(),
        workspace: Some("builtin".to_string()),
        path: None,
        scope: "selected Skill instructions".to_string(),
        bytes: body.len() as u64,
        fingerprint: mini_agent_protocol::stable_digest(body.as_bytes()),
        supersedes: None,
        reused: false,
    };
    let message = record.context_message(&body);

    assert!(
        harness
            .append_context_injection(message.clone(), record)
            .unwrap()
            .is_some()
    );
    harness
        .restore_session(SessionState::from_messages(vec![Message::Context {
            text: message,
        }]))
        .unwrap();

    let body = "x".repeat(MAX_CONTEXT_INJECTION_BYTES);
    let oversized = mini_agent_protocol::ContextInjectionRecord {
        id: "workspace_instruction_oversized".to_string(),
        kind: mini_agent_protocol::ContextInjectionKind::ProjectInstructions,
        source: "AGENTS.md".to_string(),
        workspace: Some("main".to_string()),
        path: Some("AGENTS.md".to_string()),
        scope: "workspace".to_string(),
        bytes: body.len() as u64,
        fingerprint: mini_agent_protocol::stable_digest(body.as_bytes()),
        supersedes: None,
        reused: false,
    }
    .context_message(&body);
    let error = harness.append_context_injection(
        oversized.clone(),
        mini_agent_protocol::ContextInjectionRecord::from_context_message(&oversized).unwrap(),
    );
    let error = error.unwrap_err();
    assert_eq!(error.limit, MAX_CONTEXT_INJECTION_BYTES);
    assert_eq!(error.actual, oversized.len());
}

#[test]
fn restores_only_history_that_fits_the_current_harness() {
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRouter::default(),
        HarnessConfig::default(),
    );
    let messages = vec![Message::Context {
        text: "persisted world".to_string(),
    }];

    harness.restore_history(messages.clone()).unwrap();

    assert_eq!(harness.messages(), messages);
    let error = harness
        .restore_history(vec![Message::Context {
            text: "x".repeat(HarnessConfig::default().max_context_item_bytes + 1),
        }])
        .unwrap_err();
    assert_eq!(error.kind, LimitKind::ContextItemBytes);
    assert_eq!(harness.messages(), messages);

    let user_err = harness
        .restore_history(vec![Message::User {
            text: "x".repeat(HarnessConfig::default().max_user_input_bytes + 1),
        }])
        .unwrap_err();
    assert_eq!(user_err.kind, LimitKind::UserInputBytes);

    let tool_err = harness
        .restore_history(vec![Message::Tool {
            call_id: "call-1".to_string(),
            name: "uppercase".to_string(),
            content: "x".repeat(HarnessConfig::default().max_tool_output_bytes + 1),
            is_error: false,
            outcome: None,
        }])
        .unwrap_err();
    assert_eq!(tool_err.kind, LimitKind::ToolOutputBytes);

    let assistant_err = harness
        .restore_history(vec![Message::Assistant {
            reasoning: String::new(),
            text: "x".repeat(HarnessConfig::default().max_model_response_bytes + 1),
            tool_calls: vec![],
        }])
        .unwrap_err();
    assert_eq!(assistant_err.kind, LimitKind::ModelResponseBytes);
}

#[tokio::test]
async fn accepts_model_responses_above_the_previous_default_limit() {
    let text = "x".repeat(65 * 1024);
    let model = ScriptedModel {
        responses: VecDeque::from([text_response(text.clone())]),
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());

    let outcome = harness.run("answer", &mut ()).await.unwrap();

    assert_eq!(outcome.final_text.len(), text.len());
}

#[tokio::test]
async fn accepts_request_context_above_the_previous_default_limit() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([text_response("ok")]),
        requests: Arc::clone(&requests),
    };
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());
    let history = (0..140)
        .map(|_| Message::Context {
            text: "x".repeat(8 * 1024),
        })
        .collect();
    harness.restore_history(history).unwrap();

    harness.run("capture context", &mut ()).await.unwrap();

    let requests = requests.lock().unwrap();
    let serialized = serde_json::to_vec(&requests[0].messages).unwrap();
    assert!(serialized.len() > 1024 * 1024);
}

struct RequestSteer(RunControl);

impl ToolHandler for RequestSteer {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "request_steer".to_string(),
            description: "Request a cooperative turn stop".to_string(),
            parameters: json!({"type": "object", "additionalProperties": false}),
        }
    }
}

impl ToolRuntime for RequestSteer {
    fn execute(&self, _arguments: &Value) -> Result<String, ToolError> {
        self.0.request_steer();
        Ok("steer requested".to_string())
    }
}

#[test]
fn verifier_can_restore_tool_history_before_disabling_new_tool_calls() {
    let history = vec![
        Message::User {
            text: "inspect the release".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: json!({"key": "release"}),
            }],
        },
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "lookup".to_string(),
            content: "release is ready".to_string(),
            is_error: false,
            outcome: None,
        },
    ];
    let mut harness = Harness::new(
        ScriptedModel {
            responses: VecDeque::new(),
        },
        ToolRouter::default(),
        HarnessConfig::default(),
    );

    harness.restore_history(history.clone()).unwrap();
    harness.replace_config(HarnessConfig {
        max_tool_calls_per_step: 0,
        ..HarnessConfig::default()
    });

    assert_eq!(harness.messages(), history);
}

#[tokio::test]
async fn compacts_context_and_continues_the_tool_loop() {
    let long_tool_value = "x".repeat(300);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = ContextWindowRetryModel {
        responses: VecDeque::from([
            Ok(tool_response("call-1", "uppercase", json!({"text": long_tool_value}))),
            Err(ContextWindowExceeded),
            Ok(ModelResponse {
                reasoning: String::new(),
                text: "The user asked for a long operation. The uppercase tool completed successfully. Continue by reporting completion.".to_string(),
                tool_calls: Vec::new(),
                usage: Some(ModelUsage {
                    input_tokens: 100,
                    cached_input_tokens: Some(0),
                    output_tokens: 20,
                }),
            }),
            Ok(text_response("Long operation completed.")),
        ]),
        requests: Arc::clone(&requests),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 1024,
        max_tool_output_bytes: 512,
        max_context_bytes: 2000,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::new(vec![Box::new(Uppercase)]), config);
    harness
        .append_context("<world_state>rust,cargo</world_state>")
        .unwrap();
    let mut events = RecordingObserver::default();
    let prompt = format!("perform the long operation {}", "n".repeat(400));

    let outcome = harness.run(&prompt, &mut events).await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "Long operation completed.");
    assert!(matches!(
        outcome.messages.as_slice(),
        [
            Message::User { text },
            Message::Context { text: context },
            Message::Assistant { tool_calls, .. },
            Message::Tool { name, content, .. },
            Message::Assistant {
                text: answer,
                tool_calls: final_calls,
                ..
            }
        ] if text.starts_with(COMPACTION_PREFIX)
            && context == "<world_state>rust,cargo</world_state>"
            && !tool_calls.is_empty()
            && name == "uppercase"
            && content.contains('X')
            && answer == "Long operation completed."
            && final_calls.is_empty()
    ));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            usage: Some(ModelUsage { input_tokens: 100, .. }),
        } if *before_bytes >= 1400 && after_bytes < before_bytes
    )));
    let first_model_start = events
        .0
        .iter()
        .position(|event| matches!(event, Event::ModelStarted { .. }))
        .unwrap();
    let first_compaction = events
        .0
        .iter()
        .position(|event| matches!(event, Event::ContextCompactionStarted { .. }))
        .unwrap();
    assert!(first_model_start < first_compaction);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let first = &requests[0];
    let rejected = &requests[1];
    let compaction = &requests[2];
    let continuation = &requests[3];
    assert!(!rejected.tools.is_empty());
    assert!(
        rejected.messages.iter().any(
            |message| matches!(message, Message::Tool { content, .. } if content.contains('X'))
        )
    );
    assert_eq!(compaction.system_prompt, first.system_prompt);
    assert!(compaction.tools.is_empty());
    assert_eq!(continuation.system_prompt, first.system_prompt);
    assert_eq!(continuation.tools, first.tools);
    assert!(matches!(
        compaction.messages.as_slice(),
        [
            Message::User { text: compacted_prompt },
            Message::User { text: instruction },
        ] if compacted_prompt == prompt.as_str() && instruction == compaction_prompt()
    ));
    assert!(matches!(
        continuation.messages.as_slice(),
        [
            Message::User { text },
            Message::Context { .. },
            Message::Assistant { tool_calls, .. },
            Message::Tool { name, content, .. },
        ] if text.starts_with(COMPACTION_PREFIX)
            && !tool_calls.is_empty()
            && name == "uppercase"
            && content.contains('X')
    ));
}

#[tokio::test]
async fn empty_summary_falls_back_to_mechanical_trim() {
    let model = ContextWindowRetryModel {
        responses: VecDeque::from([
            Ok(tool_response(
                "call-1",
                "uppercase",
                json!({"text": "x".repeat(300)}),
            )),
            Err(ContextWindowExceeded),
            Ok(text_response("   ")),
            Ok(text_response("Long operation completed.")),
        ]),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 1024,
        max_tool_output_bytes: 512,
        max_context_bytes: 2000,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::new(vec![Box::new(Uppercase)]), config);
    let mut events = RecordingObserver::default();

    let outcome = harness
        .run("perform the long operation", &mut events)
        .await
        .unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.final_text, "Long operation completed.");
    assert!(outcome.messages.iter().any(|message| matches!(
        message,
        Message::Tool { name, content, .. } if name == "uppercase" && content.contains('X')
    )));
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            ..
        } if after_bytes < before_bytes
    )));
    assert!(!events.0.iter().any(|event| matches!(
        event,
        Event::RunFailed {
            reason: mini_agent_protocol::RunFailure::Compaction,
        }
    )));
}

#[tokio::test]
async fn retries_provider_context_overflow_after_compacting_history() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = ContextWindowRetryModel {
        responses: VecDeque::from([
            Err(ContextWindowExceeded),
            Err(ContextWindowExceeded),
            Ok(text_response("Recovered after compacting history.")),
        ]),
        requests: Arc::clone(&requests),
    };
    let history = (0..4)
        .flat_map(|index| {
            [
                Message::User {
                    text: format!("old-{index}:{}", "x".repeat(2048)),
                },
                Message::Assistant {
                    reasoning: String::new(),
                    text: format!("answer-{index}"),
                    tool_calls: Vec::new(),
                },
            ]
        })
        .collect::<Vec<_>>();
    let mut harness = Harness::new(model, ToolRouter::default(), HarnessConfig::default());
    harness.restore_history(history).unwrap();
    let mut events = RecordingObserver::default();

    let outcome = harness.run("current request", &mut events).await.unwrap();

    assert_eq!(outcome.final_text, "Recovered after compacting history.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].tools.is_empty());
    assert!(requests[2].messages.len() < requests[0].messages.len());
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|message| matches!(message, Message::User { text } if text == "current request"))
    );
    assert!(
        !requests[2]
            .messages
            .iter()
            .any(|message| matches!(message, Message::User { text } if text.starts_with("old-0:")))
    );
    assert!(events.0.iter().any(|event| matches!(
        event,
        Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            usage: None,
        } if after_bytes < before_bytes
    )));
}

#[tokio::test]
async fn reject_policy_does_not_retry_provider_context_overflow() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = ContextWindowRetryModel {
        responses: VecDeque::from([Err(ContextWindowExceeded)]),
        requests: Arc::clone(&requests),
    };
    let history = vec![
        Message::User {
            text: format!("old:{}", "x".repeat(2048)),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "old answer".to_string(),
            tool_calls: Vec::new(),
        },
    ];
    let mut harness = Harness::new(
        model,
        ToolRouter::default(),
        HarnessConfig {
            context_limit_behavior: ContextLimitBehavior::Reject,
            ..HarnessConfig::default()
        },
    );
    harness.restore_history(history).unwrap();
    let mut events = RecordingObserver::default();

    let error = harness
        .run("current request", &mut events)
        .await
        .unwrap_err();

    assert!(matches!(error, HarnessError::Model(_)));
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ContextCompactionStarted { .. }))
    );
}

#[tokio::test]
async fn rejects_context_above_the_byte_ceiling_without_compacting_by_bytes() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([text_response("unused")]),
        requests: Arc::clone(&requests),
    };
    let padding = "p".repeat(300);
    let history = vec![
        Message::User {
            text: format!("old:{padding}"),
        },
        Message::User {
            text: format!("mid:{padding}"),
        },
        Message::User {
            text: format!("recent:{padding}"),
        },
    ];
    let system_prompt = HarnessConfig::default().system_prompt;
    let history_bytes = context_bytes_for(&system_prompt, &history, &[]);
    let config = HarnessConfig {
        max_context_bytes: history_bytes,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config.clone());
    harness.restore_history(history).unwrap();
    let mut events = RecordingObserver::default();

    let error = harness.run("continue", &mut events).await.unwrap_err();
    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ContextBytes,
            ..
        })
    ));
    let requests = requests.lock().unwrap();
    assert!(requests.is_empty());
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ContextCompactionStarted { .. }))
    );
}

#[tokio::test]
async fn does_not_compact_when_context_is_over_half_but_under_the_byte_ceiling() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([text_response("Continued.")]),
        requests: Arc::clone(&requests),
    };
    let history = (0..3)
        .map(|_| Message::Context {
            text: "x".repeat(7 * 1024),
        })
        .collect();
    let config = HarnessConfig {
        max_context_bytes: 32 * 1024,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config.clone());
    harness.restore_history(history).unwrap();
    let mut events = RecordingObserver::default();

    let outcome = harness.run("continue", &mut events).await.unwrap();

    assert_eq!(outcome.final_text, "Continued.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let actual = context_bytes_for(
        &config.system_prompt,
        &requests[0].messages,
        &requests[0].tools,
    );
    assert!(actual > config.max_context_bytes / 2);
    assert!(actual < config.max_context_bytes);
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ContextCompactionStarted { .. }))
    );
}

#[tokio::test]
async fn prepares_a_compacted_fork_without_mutating_the_parent() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        responses: VecDeque::from([text_response("summary of the older turns")]),
        requests: Arc::clone(&requests),
    };
    let padding = "p".repeat(500);
    let history = vec![
        Message::User {
            text: format!("old:{padding}"),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: format!("old answer:{padding}"),
            tool_calls: Vec::new(),
        },
        Message::User {
            text: format!("middle:{padding}"),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: format!("middle answer:{padding}"),
            tool_calls: Vec::new(),
        },
        Message::User {
            text: "recent question".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "recent answer".to_string(),
            tool_calls: Vec::new(),
        },
    ];
    let config = HarnessConfig {
        system_prompt: "system".to_string(),
        max_context_bytes: 4_000,
        context_limit_behavior: ContextLimitBehavior::Compact,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);
    harness.restore_history(history).unwrap();
    let parent_messages = harness.messages().to_vec();

    let prepared = harness
        .prepare_fork_checkpoint(ForkContextPolicy::Compact)
        .await
        .unwrap();

    assert_eq!(harness.messages(), parent_messages.as_slice());
    assert_eq!(prepared.method, ForkCompactionMethod::ModelSummary);
    assert!(prepared.context_after_bytes < prepared.context_before_bytes);
    assert!(prepared.session.messages().iter().any(|message| matches!(
        message,
        Message::User { text } if text.contains("summary of the older turns")
    )));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn exact_fork_preparation_keeps_history_and_skips_the_model() {
    let padding = "p".repeat(1_800);
    let history = vec![
        Message::User {
            text: format!("keep this history:{padding}"),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: format!("and do not compact it at fork time:{padding}"),
            tool_calls: Vec::new(),
        },
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut harness = Harness::new(
        RecordingModel {
            responses: VecDeque::new(),
            requests: Arc::clone(&requests),
        },
        ToolRouter::default(),
        HarnessConfig {
            system_prompt: "system".to_string(),
            max_context_bytes: 5_000,
            ..HarnessConfig::default()
        },
    );
    harness.restore_history(history.clone()).unwrap();
    let prepared = harness
        .prepare_fork_checkpoint(ForkContextPolicy::Exact)
        .await
        .unwrap();

    assert_eq!(prepared.method, ForkCompactionMethod::Exact);
    assert_eq!(prepared.session.messages(), history.as_slice());
    assert!(prepared.context_before_bytes >= 2_500);
    assert_eq!(harness.messages(), history.as_slice());
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn split_prefix_tail_keeps_last_two_assistant_groups() {
    let assistant = |text: &str| Message::Assistant {
        reasoning: String::new(),
        text: text.to_string(),
        tool_calls: Vec::new(),
    };
    let messages = vec![
        Message::User {
            text: "one".to_string(),
        },
        assistant("a1"),
        Message::User {
            text: "two".to_string(),
        },
        assistant("a2"),
        Message::User {
            text: "three".to_string(),
        },
        assistant("a3"),
    ];

    let (prefix, tail) = split_prefix_tail(&messages);

    assert_eq!(
        prefix,
        vec![
            Message::User {
                text: "one".to_string(),
            },
            assistant("a1"),
            Message::User {
                text: "two".to_string(),
            },
        ]
    );
    assert_eq!(
        tail,
        vec![
            assistant("a2"),
            Message::User {
                text: "three".to_string(),
            },
            assistant("a3"),
        ]
    );
}

#[test]
fn trim_prefix_to_fit_drops_oldest_until_request_fits() {
    let system = "sys";
    let tools: &[ToolSpec] = &[];
    let prompt = "SUMMARIZE";
    let mut prefix = vec![
        Message::User {
            text: format!("old-{}", "a".repeat(400)),
        },
        Message::User {
            text: format!("keep-{}", "b".repeat(40)),
        },
    ];
    let fitting = vec![
        prefix[1].clone(),
        Message::User {
            text: prompt.to_string(),
        },
    ];
    let max_bytes = context_bytes_for(system, &fitting, tools);

    trim_prefix_to_fit(&mut prefix, prompt, system, tools, max_bytes);

    assert_eq!(prefix.len(), 1);
    assert!(matches!(
        &prefix[0],
        Message::User { text } if text.starts_with("keep-")
    ));
}

#[test]
fn truncates_utf8_within_hard_byte_limit() {
    let output = truncate_utf8("一二三四五六七八九十".to_string(), 20);

    assert!(output.len() <= 20);
    assert!(output.is_char_boundary(output.len()));
    assert!(output.starts_with('一'));
    assert!(output.ends_with('十'));
}

#[test]
fn bounded_compaction_prompt_respects_small_utf8_limits() {
    for limit in [0, 1, 4, 17, 351] {
        let prompt = bounded_compaction_prompt(limit);
        assert!(
            prompt.len() <= limit,
            "limit={limit}, bytes={}",
            prompt.len()
        );
        assert!(prompt.is_char_boundary(prompt.len()));
    }
}

#[tokio::test]
async fn rejects_oversized_user_input_without_retaining_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "unused".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_user_input_bytes: 4,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);
    let mut events = RecordingObserver::default();

    let error = harness.run("12345", &mut events).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::UserInputBytes,
            limit: 4,
            actual: 5,
        })
    ));
    assert!(harness.messages().is_empty());
    assert!(matches!(
        events.0.as_slice(),
        [Event::RunFailed {
            reason: mini_agent_protocol::RunFailure::LimitExceeded(_)
        }]
    ));
    assert_eq!(
        serde_json::to_value(&events.0[0]).unwrap(),
        json!({
            "type": "run_failed",
            "reason": {
                "type": "limit_exceeded",
                "detail": {
                    "kind": "user_input_bytes",
                    "limit": 4,
                    "actual": 5
                }
            }
        })
    );
}

#[test]
fn compaction_summary_includes_prefix_within_user_limit() {
    let compacted = assemble_compacted(
        Some("这是一个足够长的压缩摘要，用于验证 UTF-8 截断"),
        Vec::new(),
        Vec::new(),
        32,
    );

    let Some(Message::User { text }) = compacted.first() else {
        panic!("compaction should produce a summary user message");
    };
    assert!(text.len() <= 32);
    assert!(text.is_char_boundary(text.len()));
}

#[tokio::test]
async fn rejects_context_before_calling_the_model() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: "unused".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_context_bytes: 1,
        context_limit_behavior: ContextLimitBehavior::Reject,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);

    let error = harness.run("a", &mut ()).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ContextBytes,
            limit: 1,
            ..
        })
    ));
    assert!(harness.messages().is_empty());
}

#[tokio::test]
async fn rejects_excess_tool_calls_before_executing_any() {
    let call = |id: &str| ToolCall {
        id: id.to_string(),
        name: "uppercase".to_string(),
        arguments: json!({"text": "quiet"}),
    };
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![call("call-1"), call("call-2")],
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_tool_calls_per_step: 1,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::new(vec![Box::new(Uppercase)]), config);
    let mut events = RecordingObserver::default();

    let error = harness.run("do both", &mut events).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ToolCallsPerStep,
            limit: 1,
            actual: 2,
        })
    ));
    assert!(
        !events
            .0
            .iter()
            .any(|event| matches!(event, Event::ToolStarted { .. }))
    );
}

#[tokio::test]
async fn rejects_oversized_model_response_before_retaining_it() {
    let model = ScriptedModel {
        responses: VecDeque::from([ModelResponse {
            reasoning: "why".to_string(),
            text: "x".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        }]),
    };
    let config = HarnessConfig {
        max_model_response_bytes: 5,
        ..HarnessConfig::default()
    };
    let mut harness = Harness::new(model, ToolRouter::default(), config);

    let error = harness.run("answer", &mut ()).await.unwrap_err();

    assert!(matches!(
        error,
        HarnessError::Limit(LimitExceeded {
            kind: LimitKind::ModelResponseBytes,
            limit: 5,
            ..
        })
    ));
    assert_eq!(
        harness.messages(),
        &[Message::User {
            text: "answer".to_string(),
        }]
    );
}

#[tokio::test]
async fn repetitive_tool_calls_trigger_loop_warning() {
    struct EchoTool;
    impl ToolHandler for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".to_string(),
                description: "echo input".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"],
                    "additionalProperties": false
                }),
            }
        }
    }
    impl ToolRuntime for EchoTool {
        fn execute(&self, _args: &Value) -> Result<String, ToolError> {
            Ok("same output".to_string())
        }
    }

    let tools = ToolRouter::new(vec![Box::new(EchoTool)]);

    let model = ScriptedModel {
        responses: VecDeque::from(vec![
            tool_response("call1", "echo", json!({"msg": "hello"})),
            tool_response("call2", "echo", json!({"msg": "hello"})),
            tool_response("call3", "echo", json!({"msg": "hello"})),
            text_response("done after warning"),
        ]),
    };

    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    let outcome = harness.run("start", &mut ()).await.unwrap();

    assert_eq!(outcome.final_text, "done after warning");
    // Verify that loop warning was injected into messages
    let has_loop_warning = harness.messages().iter().any(|msg| match msg {
        Message::Context { text } => text.contains("Loop warning"),
        _ => false,
    });
    assert!(has_loop_warning, "Expected loop warning in harness context");
}

#[test]
fn turn_atomic_trimming_drops_assistant_and_tool_groups_together() {
    let system = "sys";
    let tools: &[ToolSpec] = &[];
    let prompt = "SUMMARIZE";
    let mut prefix = vec![
        Message::User {
            text: "first question".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "calling tool".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "test_tool".to_string(),
                arguments: json!({"arg": "val"}),
            }],
        },
        Message::Tool {
            call_id: "call-1".to_string(),
            name: "test_tool".to_string(),
            content: "x".repeat(300),
            is_error: false,
            outcome: None,
        },
        Message::User {
            text: "second question".to_string(),
        },
    ];

    let fitting = vec![
        prefix[3].clone(),
        Message::User {
            text: prompt.to_string(),
        },
    ];
    let max_bytes = context_bytes_for(system, &fitting, tools);

    trim_prefix_to_fit(&mut prefix, prompt, system, tools, max_bytes);

    let mut pending_tool_calls: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for msg in &prefix {
        match msg {
            Message::Assistant { tool_calls, .. } => {
                for call in tool_calls {
                    pending_tool_calls.insert(call.id.clone());
                }
            }
            Message::Tool { call_id, .. } => {
                assert!(
                    pending_tool_calls.contains(call_id.as_str()),
                    "found orphan tool output without matching assistant call: {call_id}"
                );
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct RecordingObserver(Vec<Event>);

impl Observer for RecordingObserver {
    fn observe(&mut self, event: &Event) {
        self.0.push(event.clone());
    }
}
