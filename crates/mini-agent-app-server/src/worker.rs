use super::*;
use crate::action::{ActionEnvelope, ActionReceipt, ActionResult, ActionSequencer, respond};
use crate::management::RuntimeActorState;
use crate::notification::RuntimeNotification;
use crate::runtime_actor::RuntimeRequest;
use crate::runtime_command::RuntimeCommand;
use crate::status::{self, RuntimeStatusHandle};
use crate::thread_manager::ThreadManager;
use mini_agent_app_server_protocol::{
    ItemCompletedNotification, ItemSortDirection, ItemStartedNotification, RuntimePhase,
    ThreadItem, ThreadItemEntry, ThreadItemsListParams, ThreadItemsListResult, TurnReadResult,
};
use mini_agent_core::{
    ExecutionJournalEntry, ExecutionJournalSink, ExecutionPhase, SteeringMode, TurnResult,
};
use mini_agent_protocol::{Event, EventEnvelope, EventSink, ModelUsage, SkillLoadPhase};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone)]
struct ExecutionProgressMonitor {
    phase: Arc<AtomicU8>,
    last_progress_ms: Arc<AtomicU64>,
}

impl ExecutionProgressMonitor {
    fn new() -> Self {
        Self {
            phase: Arc::new(AtomicU8::new(1)),
            last_progress_ms: Arc::new(AtomicU64::new(timestamp_ms())),
        }
    }

    fn observe(&self, event: &Event) {
        let phase = match event {
            Event::ToolStarted { .. } | Event::ToolFinished { .. } => Some(2),
            Event::ModelStarted { .. }
            | Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::ModelResponded { .. } => Some(1),
            Event::ContextCompactionStarted { .. } | Event::ContextCompactionFinished { .. } => {
                Some(3)
            }
            _ => None,
        };
        if let Some(phase) = phase {
            self.phase.store(phase, Ordering::Release);
            self.last_progress_ms
                .store(timestamp_ms(), Ordering::Release);
        }
    }

    fn phase(&self) -> ExecutionPhase {
        match self.phase.load(Ordering::Acquire) {
            2 => ExecutionPhase::ToolBatch,
            _ => ExecutionPhase::ModelRequest,
        }
    }

    fn last_progress_ms(&self) -> u64 {
        self.last_progress_ms.load(Ordering::Acquire)
    }
}

fn skill_display_name(skill: &mini_agent_protocol::SkillLoadRecord) -> String {
    skill
        .qualified_name
        .clone()
        .unwrap_or_else(|| skill.name.clone())
}

#[allow(clippy::too_many_arguments)]
fn operation_record(
    operation_id: &str,
    status: &str,
    turn_id: Option<&str>,
    attempt: u32,
    attempt_kind: Option<mini_agent_protocol::ChildTaskAttemptKind>,
    result: Option<&str>,
    error: Option<&str>,
    group_id: Option<&str>,
    execution_mode: Option<&str>,
    sequence: Option<u32>,
    prompt: Option<&str>,
) -> mini_agent_capabilities::SessionOperation {
    let mut operation = mini_agent_capabilities::SessionOperation::new(
        operation_id.to_string(),
        "child_task".to_string(),
        status.to_string(),
    );
    operation.turn_id = turn_id.map(str::to_string);
    operation.attempt = attempt;
    operation.attempt_kind = attempt_kind;
    operation.result = result.map(mini_agent_capabilities::SessionOperation::bounded_result);
    operation.error = error.map(str::to_string);
    operation.group_id = group_id.map(str::to_string);
    operation.execution_mode = execution_mode.map(str::to_string);
    operation.sequence = sequence;
    operation.prompt = prompt.map(str::to_string);
    operation
}

#[derive(Clone)]
pub(super) enum TurnOrigin {
    Client,
    Goal { goal_id: String },
}

pub(super) enum Command {
    Shutdown {
        reply: oneshot::Sender<Result<(), AppServerError>>,
    },
    InstallRuntime {
        state: Box<RuntimeActorState>,
    },
    Runtime(RuntimeRequest),
    Start {
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
        origin: TurnOrigin,
        turn_source: Option<mini_agent_protocol::TurnSource>,
        execution_resume: Option<mini_agent_app_server_protocol::TurnResumeParams>,
        reply: oneshot::Sender<ActionResult<TurnSubmission>>,
    },
    GoalVerificationCompleted {
        thread_id: ThreadId,
        goal_id: String,
        turn_id: TurnId,
        checkpoint_seq: u64,
        result: Result<(String, crate::goal_service::VerifierVerdict), String>,
    },
    Cancel {
        thread_id: ThreadId,
        request: TurnCancel,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    ReadThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<ActionResult<mini_agent_app_server_protocol::ThreadReadResult>>,
    },
    UpdateThread {
        thread_id: ThreadId,
        update: ThreadUpdate,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    ResetThread {
        thread_id: ThreadId,
        new_thread_id: ThreadId,
        next_turn_number: u64,
        reply: oneshot::Sender<ActionResult<ThreadId>>,
    },
    CloseThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<ActionResult<()>>,
    },
    ReadTurn {
        turn_id: TurnId,
        reply: oneshot::Sender<ActionResult<Option<SettledTurn>>>,
    },
    ReadItems {
        params: ThreadItemsListParams,
        reply: oneshot::Sender<ActionResult<ThreadItemsListResult>>,
    },
    CreateThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<ActionResult<ThreadId>>,
    },
    ForkThread {
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
        reply: oneshot::Sender<ActionResult<ThreadId>>,
    },
    ResumeThread {
        thread_id: ThreadId,
        checkpoint: ThreadCheckpoint,
        reply: oneshot::Sender<ActionResult<ThreadId>>,
    },
}

/// Orders Thread events and runtime notifications around a settled Turn.
struct ThreadListener {
    events: broadcast::Sender<EventEnvelope>,
    notifications: broadcast::Sender<RuntimeNotification>,
    event_replay: Arc<Mutex<EventReplayBuffer>>,
    runtime_status: RuntimeStatusHandle,
    runtime_revision: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    pending_finish: Option<EventEnvelope>,
    tool_arguments: Vec<(String, Value)>,
    skill_paths: Vec<mini_agent_capabilities::SkillPathRecord>,
    pending_skill_reads: BTreeMap<String, mini_agent_protocol::SkillLoadRecord>,
    started_skill_reads: BTreeSet<String>,
    loaded_skill_reads: BTreeSet<String>,
    failed_skill_reads: BTreeSet<String>,
    presentation: mini_agent_capabilities::TurnPresentation,
    turn_source: Option<mini_agent_protocol::TurnSource>,
    assistant_segments: u32,
    tokens_used: u64,
    execution_progress: ExecutionProgressMonitor,
}

impl ThreadListener {
    fn take_pending_finish(&mut self) -> Option<EventEnvelope> {
        self.pending_finish.take()
    }

    fn take_tool_arguments(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.tool_arguments)
    }

    fn take_presentation(&mut self) -> mini_agent_capabilities::TurnPresentation {
        std::mem::take(&mut self.presentation)
    }

    fn record_presentation_event(&mut self, event: &Event) {
        use mini_agent_capabilities::TurnPresentationActivity;

        match event {
            Event::ModelResponded {
                usage,
                context_bytes,
                ..
            } => {
                self.assistant_segments = self.assistant_segments.saturating_add(1);
                self.presentation.set_context_usage(*usage, *context_bytes);
            }
            Event::ContextInjected { records } => {
                self.presentation
                    .push(TurnPresentationActivity::context_injected(
                        self.assistant_segments,
                        records.iter().cloned(),
                    ));
            }
            Event::SkillGroupActivated { group, source } => {
                self.presentation
                    .push(TurnPresentationActivity::skill_group_activated(
                        self.assistant_segments,
                        group,
                        source,
                    ));
            }
            Event::SkillsLoaded {
                phase,
                activation,
                skills,
            } => {
                self.presentation
                    .push(TurnPresentationActivity::skills_loaded(
                        self.assistant_segments,
                        match phase {
                            SkillLoadPhase::Started => "started",
                            SkillLoadPhase::Loaded => "loaded",
                        },
                        activation.as_deref(),
                        skills.iter().map(skill_display_name),
                    ));
            }
            Event::SkillsLoadFailed {
                activation,
                skills,
                reason_code,
            } => {
                self.presentation
                    .push(TurnPresentationActivity::skills_load_failed(
                        self.assistant_segments,
                        activation.as_deref(),
                        skills.iter().cloned(),
                        reason_code,
                    ));
            }
            _ => {}
        }
    }

    fn send_event(&self, mut event: EventEnvelope) {
        event.turn_source = self.turn_source;
        self.event_replay.lock().unwrap().push(event.clone());
        let turn_id = event.turn_id.clone();
        if let Some(turn_id) = turn_id.clone() {
            for item in ThreadItem::started_from_event(&event) {
                let _ = self.notifications.send(RuntimeNotification::ItemStarted(
                    ItemStartedNotification {
                        thread_id: event.thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item,
                        started_at_ms: timestamp_ms(),
                    },
                ));
            }
        }
        let _ = self.events.send(event.clone());
        let _ = self
            .notifications
            .send(RuntimeNotification::Event(event.clone()));
        if let Some(turn_id) = turn_id {
            for item in ThreadItem::completed_from_event(&event) {
                let _ = self.notifications.send(RuntimeNotification::ItemCompleted(
                    ItemCompletedNotification {
                        thread_id: event.thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item,
                        completed_at_ms: timestamp_ms(),
                    },
                ));
            }
        }
    }

    fn record_usage(&mut self, usage: Option<ModelUsage>) {
        if let Some(usage) = usage {
            self.tokens_used = self
                .tokens_used
                .saturating_add(usage.input_tokens)
                .saturating_add(usage.output_tokens);
        }
    }

    fn update_status_for_event(&self, event: &EventEnvelope) {
        if self.stopping.load(Ordering::Acquire)
            && !matches!(event.event, Event::TurnFinished { .. })
        {
            let checkpoint_seq = self.runtime_status.lock().unwrap().checkpoint_seq;
            status::publish(
                &self.runtime_status,
                &self.notifications,
                event.thread_id.clone(),
                RuntimePhase::Stopping,
                event.turn_id.clone(),
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
                checkpoint_seq,
                &self.runtime_revision,
                None,
            );
            return;
        }
        let (phase, operation_id) = match &event.event {
            Event::TurnStarted { .. } => (
                RuntimePhase::StartingTurn,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::SkillGroupActivated { .. } | Event::SkillsLoaded { .. } => (
                RuntimePhase::StartingTurn,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::SkillsLoadFailed { .. } => (
                RuntimePhase::Failed,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::RunStarted { .. } | Event::ModelStarted { .. } => (
                RuntimePhase::Model,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::ToolStarted { call } => (
                RuntimePhase::Tool,
                Some(status::operation("tool", &call.id)),
            ),
            Event::ContextCompactionStarted { .. } => (
                RuntimePhase::Compaction,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::RunFinished { .. } | Event::TurnFinished { .. } => (
                RuntimePhase::Persisting,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::RunFailed { .. } => (
                RuntimePhase::Failed,
                event
                    .turn_id
                    .as_ref()
                    .map(|turn_id| status::operation("turn", turn_id.as_str())),
            ),
            Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::ContextInjected { .. }
            | Event::ModelResponded { .. }
            | Event::ToolFinished { .. }
            | Event::ContextCompactionFinished { .. } => return,
        };
        let checkpoint_seq = self.runtime_status.lock().unwrap().checkpoint_seq;
        status::publish(
            &self.runtime_status,
            &self.notifications,
            event.thread_id.clone(),
            phase,
            event.turn_id.clone(),
            operation_id,
            checkpoint_seq,
            &self.runtime_revision,
            None,
        );
    }
}

struct RunningCommandContext<'a, M> {
    runtime: &'a mut Option<RuntimeActorState>,
    threads: &'a mut ThreadManager<M>,
    runtime_revision: &'a Arc<AtomicU64>,
    runtime_status: &'a RuntimeStatusHandle,
    notifications: &'a broadcast::Sender<RuntimeNotification>,
    stopping: &'a Arc<AtomicBool>,
}

impl ThreadListener {
    fn emit_skill_event(
        &mut self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        phase: SkillLoadPhase,
        skill: Option<mini_agent_protocol::SkillLoadRecord>,
        failure: Option<(String, &str)>,
        next_sequence: &mut u64,
    ) {
        let event = if let Some((name, reason_code)) = failure {
            Event::SkillsLoadFailed {
                activation: Some("on_demand".to_string()),
                skills: vec![name],
                reason_code: reason_code.to_string(),
            }
        } else {
            Event::SkillsLoaded {
                phase,
                activation: Some("on_demand".to_string()),
                skills: skill.into_iter().collect(),
            }
        };
        let mut envelope = EventEnvelope::new(
            thread_id.clone(),
            Some(turn_id.clone()),
            *next_sequence,
            event,
        );
        envelope.item_id = Some(format!("{}:skills", turn_id.as_str()));
        self.record_presentation_event(&envelope.event);
        self.send_event(envelope);
        *next_sequence = (*next_sequence).saturating_add(1);
    }
}

impl EventSink for ThreadListener {
    fn emit(&mut self, event: EventEnvelope) {
        self.execution_progress.observe(&event.event);
        self.update_status_for_event(&event);
        self.record_presentation_event(&event.event);
        if matches!(&event.event, Event::ToolStarted { .. }) {
            for item in ThreadItem::from_event(&event) {
                if let ThreadItem::ToolCall { id, arguments, .. } = item {
                    self.tool_arguments.push((id, arguments));
                }
            }
        }
        match &event.event {
            Event::ModelResponded { usage, .. }
            | Event::ContextCompactionFinished { usage, .. } => self.record_usage(*usage),
            _ => {}
        }
        if matches!(event.event, Event::TurnFinished { .. }) {
            self.pending_finish = Some(event);
        } else {
            self.send_event(event);
        }
    }

    fn before_event(
        &mut self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        event: &Event,
        next_sequence: &mut u64,
    ) {
        let Event::ToolStarted { call } = event else {
            return;
        };
        if call.name != "read_file" {
            return;
        }
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return;
        };
        let requested = path
            .replace('\\', "/")
            .strip_prefix("./")
            .map_or_else(|| path.replace('\\', "/"), str::to_string);
        let canonical = PathBuf::from(path).canonicalize().ok();
        let Some(skill) = self
            .skill_paths
            .iter()
            .find(|skill| canonical.as_ref() == Some(&skill.path) || requested == skill.location)
        else {
            return;
        };
        let record = mini_agent_protocol::SkillLoadRecord {
            name: skill.name.clone(),
            qualified_name: Some(skill.qualified_name.clone()),
            source: skill.source.clone(),
            group: skill.group.clone(),
        };
        self.pending_skill_reads
            .insert(call.id.clone(), record.clone());
        if self
            .started_skill_reads
            .insert(skill.qualified_name.clone())
        {
            self.emit_skill_event(
                thread_id,
                turn_id,
                SkillLoadPhase::Started,
                Some(record),
                None,
                next_sequence,
            );
        }
    }

    fn after_event(
        &mut self,
        thread_id: &ThreadId,
        turn_id: &TurnId,
        event: &Event,
        next_sequence: &mut u64,
    ) {
        let Event::ToolFinished {
            call_id, is_error, ..
        } = event
        else {
            return;
        };
        let Some(skill) = self.pending_skill_reads.remove(call_id) else {
            return;
        };
        let qualified_name = skill
            .qualified_name
            .clone()
            .unwrap_or_else(|| skill.name.clone());
        if *is_error {
            if self.failed_skill_reads.insert(qualified_name.clone()) {
                self.emit_skill_event(
                    thread_id,
                    turn_id,
                    SkillLoadPhase::Loaded,
                    None,
                    Some((qualified_name, "body_read_failed")),
                    next_sequence,
                );
            }
        } else if self.loaded_skill_reads.insert(qualified_name) {
            self.emit_skill_event(
                thread_id,
                turn_id,
                SkillLoadPhase::Loaded,
                Some(skill),
                None,
                next_sequence,
            );
        }
    }
}

fn report_runtime_failure(
    runtime_status: &RuntimeStatusHandle,
    notifications: &broadcast::Sender<RuntimeNotification>,
    runtime_revision: &AtomicU64,
    operation_id: &str,
    error: &str,
) {
    let current = runtime_status.lock().unwrap().clone();
    status::publish_at(
        runtime_status,
        notifications,
        current.thread_id,
        RuntimePhase::Failed,
        current.turn_id,
        Some(operation_id.to_string()),
        current.checkpoint_seq,
        runtime_revision.load(std::sync::atomic::Ordering::SeqCst),
        Some(error),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn worker_loop<M>(
    threads: Vec<Thread<M>>,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<EventEnvelope>,
    notifications: broadcast::Sender<RuntimeNotification>,
    event_replay: Arc<Mutex<EventReplayBuffer>>,
    runtime_status: RuntimeStatusHandle,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: Arc<AtomicU64>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
    action_sequencer: ActionSequencer,
    control: Arc<RunControl>,
) where
    M: Model + Send + 'static,
{
    let mut runtime: Option<RuntimeActorState> = None;
    let mut threads = ThreadManager::new(threads, thread_ids.clone(), factory.clone());
    let mut settled_turns = HashMap::new();
    let mut deferred_goal_verifications = VecDeque::new();
    while let Some(command) = commands.recv().await {
        if let Command::Shutdown { reply } = command {
            if let Some(state) = runtime.as_ref()
                && let Err(error) = state.background_shells.close_all()
            {
                eprintln!("warning: failed to stop background Shell tasks: {error}");
            }
            if let Some(state) = runtime.as_ref()
                && let Err(error) = state.scheduled_tasks.close_all()
            {
                eprintln!("warning: failed to clear scheduled tasks: {error}");
            }
            drop(runtime.take());
            let _ = reply.send(Ok(()));
            break;
        }
        if let Command::InstallRuntime { state } = command {
            runtime = Some(*state);
            if runtime
                .as_ref()
                .is_some_and(|state| state.goal_runtime_handle.plan_active())
                && let Some(state) = runtime.as_mut()
                && let Err(error) = runtime_actor::set_thread_settings(
                    &mut threads,
                    state,
                    Some(true),
                    None,
                    None,
                    None,
                    None,
                )
            {
                let error_text = error.to_string();
                report_runtime_failure(
                    &runtime_status,
                    &notifications,
                    &runtime_revision,
                    "restore-plan",
                    &error_text,
                );
                eprintln!("warning: failed to restore collaboration mode: {error}");
            }
            let command_sender = runtime.as_ref().map(|state| state.commands.clone());
            match runtime_actor::resume_goal(&mut runtime, &mut threads) {
                Ok(Some(request)) => {
                    if let Some(command_sender) = command_sender {
                        spawn_goal_verifier(command_sender, request);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let error_text = error.to_string();
                    report_runtime_failure(
                        &runtime_status,
                        &notifications,
                        &runtime_revision,
                        "resume-goal",
                        &error_text,
                    );
                    eprintln!("warning: failed to resume goal runtime: {error}");
                }
            }
            if let Err(error) =
                runtime_actor::restore_thread_continuation(&mut runtime, &mut threads)
            {
                let error_text = error.to_string();
                report_runtime_failure(
                    &runtime_status,
                    &notifications,
                    &runtime_revision,
                    "restore-continuation",
                    &error_text,
                );
                eprintln!("warning: failed to restore Thread continuation: {error}");
            }
            continue;
        }
        let base_revision = runtime
            .as_ref()
            .map(RuntimeActorState::revision)
            .unwrap_or_default();
        let action = action_sequencer.admit(command, base_revision, runtime_revision.clone());
        let action_base_revision = action.base_revision;
        let receipt = action.receipt();
        match action.command {
            Command::Runtime(request) => {
                let restore_continuation =
                    matches!(&request.command, RuntimeCommand::ThreadGoalClear { .. });
                if matches!(&request.command, RuntimeCommand::PrepareSessionFork { .. }) {
                    runtime_actor::handle_session_fork_request(
                        request,
                        receipt,
                        action_base_revision,
                        &mut runtime,
                        &mut threads,
                    )
                    .await;
                } else {
                    runtime_actor::handle_request(
                        request,
                        receipt,
                        action_base_revision,
                        &mut runtime,
                        &mut threads,
                        &runtime_revision,
                    );
                }
                if restore_continuation
                    && let Err(error) =
                        runtime_actor::restore_thread_continuation(&mut runtime, &mut threads)
                {
                    let error_text = error.to_string();
                    report_runtime_failure(
                        &runtime_status,
                        &notifications,
                        &runtime_revision,
                        "restore-continuation",
                        &error_text,
                    );
                    eprintln!("warning: failed to restore Thread continuation: {error}");
                }
            }
            Command::Start {
                thread_id,
                mut request,
                expected_turn_id,
                origin,
                turn_source,
                execution_resume: resume_request,
                reply,
            } => {
                if let Some(state) = runtime.as_ref() {
                    match state.management.session_control_state_if_persisted() {
                        Ok(Some(control))
                            if matches!(
                                control.status,
                                mini_agent_capabilities::SessionControlStatus::Freezing
                                    | mini_agent_capabilities::SessionControlStatus::Frozen
                            ) =>
                        {
                            respond(
                                reply,
                                receipt,
                                Ok(TurnSubmission::NotSubmitted {
                                    reason:
                                        "parent Session is frozen; an explicit continue is required"
                                            .to_string(),
                                }),
                            );
                            continue;
                        }
                        Ok(Some(control))
                            if control.status
                                == mini_agent_capabilities::SessionControlStatus::Resuming
                                && turn_source
                                    != Some(mini_agent_protocol::TurnSource::SessionResume) =>
                        {
                            respond(
                                reply,
                                receipt,
                                Ok(TurnSubmission::NotSubmitted {
                                    reason: "Session resume is in progress; an explicit continue is required".to_string(),
                                }),
                            );
                            continue;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            respond(reply, receipt, Err(error));
                            continue;
                        }
                    }
                }
                if resume_request.is_none()
                    && let Some(execution) = runtime
                        .as_ref()
                        .and_then(|state| state.management.execution_state())
                        .filter(|execution| {
                            execution.status
                                != mini_agent_capabilities::SessionExecutionStatus::Settled
                        })
                {
                    let reason = match execution.status {
                        mini_agent_capabilities::SessionExecutionStatus::WaitingForContinue => {
                            "an execution checkpoint is waiting; continue that Turn before starting another"
                        }
                        mini_agent_capabilities::SessionExecutionStatus::NeedsReconciliation => {
                            "an execution checkpoint needs reconciliation; verify the tool result before starting another Turn"
                        }
                        mini_agent_capabilities::SessionExecutionStatus::Running => {
                            "an execution checkpoint belongs to an active Turn"
                        }
                        mini_agent_capabilities::SessionExecutionStatus::Settled => unreachable!(),
                    };
                    respond(
                        reply,
                        receipt,
                        Ok(TurnSubmission::NotSubmitted {
                            reason: reason.to_string(),
                        }),
                    );
                    continue;
                }
                if expected_turn_id.is_some() && resume_request.is_none() {
                    respond(reply, receipt, Err(AppServerError::NoActiveTurn));
                    continue;
                }
                let key = thread_id.as_str().to_string();
                let Some(mut thread) = threads.remove(&key) else {
                    respond(
                        reply,
                        receipt,
                        Err(AppServerError::ThreadNotFound(thread_id)),
                    );
                    continue;
                };
                if !matches!(
                    request.input.mode,
                    TurnInputMode::Start | TurnInputMode::StartIfIdle
                ) {
                    respond(
                        reply,
                        receipt,
                        Err(AppServerError::InvalidInputMode(request.input.mode)),
                    );
                    threads.insert(thread);
                    continue;
                }
                if thread.status() == mini_agent_protocol::ThreadStatus::Closed {
                    respond(reply, receipt, Err(AppServerError::Closed));
                    threads.insert(thread);
                    continue;
                }
                if thread.status() == mini_agent_protocol::ThreadStatus::Running {
                    respond(reply, receipt, Err(AppServerError::Busy));
                    threads.insert(thread);
                    continue;
                }

                let mut execution_resume = None;
                let mut turn_source = turn_source;
                if let Some(resume_request) = resume_request {
                    if resume_request.thread_id != thread_id {
                        respond(
                            reply,
                            receipt,
                            Err(AppServerError::ThreadNotFound(resume_request.thread_id)),
                        );
                        threads.insert(thread);
                        continue;
                    }
                    let reservation = runtime
                        .as_ref()
                        .ok_or_else(|| {
                            AppServerError::Checkpoint(
                                "execution recovery requires a persisted Session".to_string(),
                            )
                        })
                        .and_then(|state| {
                            state.management.reserve_execution_resume(
                                &resume_request.turn_id,
                                resume_request.checkpoint_seq,
                                &resume_request.request_id,
                            )
                        });
                    match reservation {
                        Ok(
                            mini_agent_capabilities::ExecutionResumeReservation::AlreadyAccepted,
                        ) => {
                            respond(
                                reply,
                                receipt,
                                Ok(TurnSubmission::Started {
                                    turn_id: resume_request.turn_id,
                                }),
                            );
                            threads.insert(thread);
                            continue;
                        }
                        Ok(mini_agent_capabilities::ExecutionResumeReservation::Accepted(
                            state,
                        )) => {
                            request.input = state.checkpoint.input.clone();
                            if let Some(runtime) = runtime.as_ref() {
                                turn_source = runtime
                                    .management
                                    .session_turn_source(state.checkpoint.turn_id.as_str());
                                match runtime
                                    .management
                                    .session_operation_for_turn(state.checkpoint.turn_id.as_str())
                                {
                                    Ok(Some(operation)) => {
                                        request.operation_id = Some(operation.operation_id);
                                        request.operation_attempt = Some(operation.attempt);
                                        request.operation_attempt_kind = operation.attempt_kind;
                                        request.operation_group_id = operation.group_id;
                                        request.execution_mode = operation.execution_mode;
                                        request.group_sequence = operation.sequence;
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        respond(reply, receipt, Err(error));
                                        threads.insert(thread);
                                        continue;
                                    }
                                }
                            }
                            execution_resume = Some((state.checkpoint, state.pending_batch));
                        }
                        Err(error) => {
                            respond(reply, receipt, Err(error));
                            threads.insert(thread);
                            continue;
                        }
                    }
                }

                let operation_attempt = request.operation_attempt.unwrap_or(1);
                let operation_attempt_kind =
                    request
                        .operation_attempt_kind
                        .or(Some(if operation_attempt > 1 {
                            mini_agent_protocol::ChildTaskAttemptKind::Retry
                        } else {
                            mini_agent_protocol::ChildTaskAttemptKind::Initial
                        }));
                if execution_resume.is_none()
                    && let Some(operation_id) = request.operation_id.as_deref()
                {
                    match runtime_actor::session_operation(&runtime, operation_id) {
                        Ok(Some(operation)) => {
                            let is_queued_attempt = operation.attempt == operation_attempt
                                && operation.status == "queued"
                                && operation.turn_id.is_none()
                                && operation.prompt.as_deref() == Some(request.input.text.as_str())
                                && operation
                                    .attempt_kind
                                    .is_none_or(|kind| Some(kind) == operation_attempt_kind);
                            let is_retry_attempt = operation
                                .attempt
                                .checked_add(1)
                                .is_some_and(|next| next == operation_attempt)
                                && matches!(operation.status.as_str(), "failed" | "cancelled")
                                && operation_attempt_kind
                                    == Some(mini_agent_protocol::ChildTaskAttemptKind::Retry)
                                && operation.prompt.as_deref() == Some(request.input.text.as_str());
                            let already_started = operation.attempt == operation_attempt
                                && operation.turn_id.is_some()
                                && operation.status != "queued";
                            if already_started {
                                respond(
                                    reply,
                                    receipt,
                                    Ok(TurnSubmission::NotSubmitted {
                                        reason: "child task attempt already has a durable turn; read its current state before resubmitting".to_string(),
                                    }),
                                );
                                threads.insert(thread);
                                continue;
                            }
                            if !is_queued_attempt && !is_retry_attempt {
                                respond(
                                    reply,
                                    receipt,
                                    Ok(TurnSubmission::NotSubmitted {
                                        reason: "child task attempt does not match its persisted operation; refresh task state before starting".to_string(),
                                    }),
                                );
                                threads.insert(thread);
                                continue;
                            }
                        }
                        Ok(None) if runtime_actor::session_is_forked(&runtime) => {
                            respond(
                                reply,
                                receipt,
                                Ok(TurnSubmission::NotSubmitted {
                                    reason: "child task operation is not persisted in this Session"
                                        .to_string(),
                                }),
                            );
                            threads.insert(thread);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            respond(reply, receipt, Err(error));
                            threads.insert(thread);
                            continue;
                        }
                    }
                }

                let user_input_limit = thread.harness().config().max_user_input_bytes;
                if request.input.text.len() > user_input_limit {
                    respond(
                        reply,
                        receipt,
                        Ok(TurnSubmission::NotSubmitted {
                            reason: format!("user input exceeds the {user_input_limit} byte limit"),
                        }),
                    );
                    threads.insert(thread);
                    continue;
                }

                let mut next_input = Some(request.input);
                let mut operation_id = request.operation_id.clone();
                let operation_group_id = request.operation_group_id.clone();
                let execution_mode = request.execution_mode.clone();
                let group_sequence = request.group_sequence;
                let mut initial_reply = Some(reply);
                let mut origin = origin;
                let goal_turn = matches!(&origin, TurnOrigin::Goal { .. });
                loop {
                    let input = next_input
                        .take()
                        .expect("app-server turn input must exist before execution");
                    let current_turn_source = turn_source.take();
                    let turn_id = execution_resume.as_ref().map_or_else(
                        || thread.next_turn_id(),
                        |(checkpoint, _)| checkpoint.turn_id.clone(),
                    );
                    if let Some(state) = runtime.as_ref()
                        && state.goal_runtime_handle.plan_active()
                        && let Err(error) = state.goal_runtime_handle.set_plan_review_pending(false)
                    {
                        eprintln!("warning: failed to clear Plan review state: {error}");
                    }
                    let goal_id = match &origin {
                        TurnOrigin::Client => None,
                        TurnOrigin::Goal { goal_id } => Some(goal_id.clone()),
                    };
                    if let Some(goal_id) = goal_id.as_deref() {
                        let accepted =
                            runtime_actor::goal_turn_started(&mut runtime, goal_id, &turn_id)
                                .unwrap_or(false);
                        if !accepted {
                            break;
                        }
                    }
                    let goal_state = match (goal_id.as_deref(), runtime.as_ref()) {
                        (Some(goal_id), Some(runtime_state)) => {
                            match runtime_state.goal_runtime_handle.load_goal_state() {
                                Ok(Some(goal)) if goal.goal_id == goal_id => Some(goal),
                                Ok(_) => None,
                                Err(error) => {
                                    let reason =
                                        format!("cannot load Goal execution limits: {error}");
                                    let _ = runtime_actor::goal_turn_limited(
                                        &mut runtime,
                                        goal_id,
                                        &turn_id,
                                        mini_agent_host::GoalStatus::Failed,
                                        &reason,
                                    );
                                    break;
                                }
                            }
                        }
                        _ => None,
                    };
                    if let (Some(goal_id), Some(goal)) = (goal_id.as_deref(), goal_state.as_ref()) {
                        let operation_id = status::operation("goal-continuation", goal_id);
                        let payload = crate::runtime_actor::workflow_payload_for_worker(
                            &runtime,
                            Some(turn_id.clone()),
                            operation_id.clone(),
                            None,
                            None,
                            Some(goal),
                            None,
                        );
                        if let Some(payload) = payload {
                            let _ = notifications.send(RuntimeNotification::Workflow(
                                crate::notification::WorkflowRuntimeEvent::GoalContinuationStarted(
                                    payload,
                                ),
                            ));
                        }
                        if let Some(state) = runtime.as_ref() {
                            status::publish_at(
                                &state.status,
                                &notifications,
                                thread_id.clone(),
                                mini_agent_app_server_protocol::RuntimePhase::StartingTurn,
                                Some(turn_id.clone()),
                                Some(operation_id),
                                Some(state.management.current_checkpoint_seq()),
                                state.revision().value(),
                                None,
                            );
                        }
                    }
                    let skill_refresh_error = runtime.as_mut().and_then(|state| {
                        match state.management.refresh_skill_catalog() {
                            Ok(_) => match state.management.prepare_skill_context() {
                                Ok(Some(context)) => {
                                    let fingerprint =
                                        mini_agent_protocol::stable_digest(context.as_bytes());
                                    let record = mini_agent_protocol::ContextInjectionRecord {
                                        id: "available_extensions".to_string(),
                                        kind: mini_agent_protocol::ContextInjectionKind::Skill,
                                        source: "技能目录".to_string(),
                                        workspace: None,
                                        path: None,
                                        scope: "当前项目中可用的技能目录".to_string(),
                                        bytes: context.len() as u64,
                                        fingerprint,
                                        supersedes: None,
                                        reused: false,
                                    };
                                    let message = record.context_message(&context);
                                    match thread
                                        .harness_mut()
                                        .append_context_injection(message, record)
                                    {
                                        Ok(Some(_)) => None,
                                        Ok(None) => None,
                                        Err(error) => Some(error.to_string()),
                                    }
                                }
                                Ok(None) => None,
                                Err(error) => Some(error.to_string()),
                            },
                            Err(error) => Some(error.to_string()),
                        }
                    });
                    let original_config = thread.harness().config().clone();
                    let is_child_task = operation_id.is_some();
                    let mut turn_config = original_config.clone();
                    if is_child_task {
                        turn_config = turn_config.with_copilot_loop();
                    }
                    if let Some(goal) = goal_state.as_ref() {
                        turn_config = turn_config.with_copilot_loop();
                        if goal.milestone_step_budget != 0 {
                            turn_config.max_steps = goal.milestone_step_budget;
                        }
                    }
                    if is_child_task || goal_state.is_some() {
                        thread.harness_mut().replace_config(turn_config);
                    }
                    let workflow = input.workflow.clone();
                    let mut selected_skills = Vec::new();
                    for name in input
                        .selected_skills
                        .iter()
                        .take(mini_agent_capabilities::MAX_SELECTED_SKILLS)
                    {
                        if !selected_skills.contains(name) {
                            selected_skills.push(name.clone());
                        }
                    }
                    let too_many_selected_skills =
                        input.selected_skills.len() > mini_agent_capabilities::MAX_SELECTED_SKILLS;
                    let group_error = skill_refresh_error.or_else(|| {
                        workflow.as_ref().and_then(|workflow| {
                            let result = match workflow.kind {
                                mini_agent_protocol::TurnWorkflowKind::SkillGroup => runtime
                                    .as_ref()
                                    .and_then(|state| state.management.skill_discovery.as_ref())
                                    .ok_or_else(|| {
                                        "skill catalog is unavailable for this runtime".to_string()
                                    })
                                    .and_then(|discovery| {
                                        discovery.activate_skill_group(&workflow.id)
                                    }),
                            };
                            result.err()
                        })
                    });
                    let group_valid = group_error.is_none();
                    let (loaded_skills, mut skill_error) = if let Some(error) = group_error {
                        (Vec::new(), Some(error))
                    } else if too_many_selected_skills {
                        (
                            Vec::new(),
                            Some(format!(
                                "at most {} skills may be activated per turn",
                                mini_agent_capabilities::MAX_SELECTED_SKILLS
                            )),
                        )
                    } else if selected_skills.is_empty() {
                        (Vec::new(), None)
                    } else {
                        let result = runtime
                            .as_ref()
                            .and_then(|state| state.management.skill_discovery.as_ref())
                            .ok_or_else(|| {
                                "skill catalog is unavailable for this runtime".to_string()
                            })
                            .and_then(|discovery| discovery.load_skills(&selected_skills));
                        match result {
                            Ok(skills) => (skills, None),
                            Err(error) => (Vec::new(), Some(error)),
                        }
                    };
                    for skill in &loaded_skills {
                        let identity = format!("skill\n{}", skill.qualified_name);
                        let suffix = mini_agent_protocol::stable_digest(identity.as_bytes())
                            .replace('-', "_");
                        let id = format!("skill_definition_{suffix}");
                        let fingerprint = mini_agent_protocol::stable_digest(skill.body.as_bytes());
                        let record = mini_agent_protocol::ContextInjectionRecord {
                            id: id.clone(),
                            kind: mini_agent_protocol::ContextInjectionKind::Skill,
                            source: format!("Skill {}", skill.name),
                            workspace: Some(skill.source.clone()),
                            path: None,
                            scope: "已激活 Skill 的正文定义".to_string(),
                            bytes: skill.body.len() as u64,
                            fingerprint: fingerprint.clone(),
                            supersedes: None,
                            reused: false,
                        };
                        let message = record.context_message(&format!(
                            "[Skill: {} · {}]\n{}",
                            skill.qualified_name, skill.source, skill.body
                        ));
                        match thread
                            .harness_mut()
                            .append_context_injection(message, record)
                        {
                            Ok(Some(_)) => {}
                            Ok(None) => {}
                            Err(error) if skill_error.is_none() => {
                                skill_error = Some(error.to_string());
                            }
                            Err(_) => {}
                        }
                    }
                    let activated_context = skill_activation_context(
                        &loaded_skills,
                        workflow.as_ref(),
                        turn_id.as_str(),
                    );
                    if let Err(error) = thread
                        .harness_mut()
                        .append_context_slot_if_changed("activated_skills", activated_context)
                        && skill_error.is_none()
                    {
                        skill_error = Some(error.to_string());
                    }
                    let mut skill_prelude = workflow
                        .as_ref()
                        .filter(|_| group_valid)
                        .map(|workflow| {
                            vec![Event::SkillGroupActivated {
                                group: workflow.id.clone(),
                                source: "builtin".to_string(),
                            }]
                        })
                        .unwrap_or_default();
                    if let Some(error) = skill_error.as_ref() {
                        skill_prelude.push(Event::SkillsLoadFailed {
                            activation: Some("explicit".to_string()),
                            skills: selected_skills.clone(),
                            reason_code: if error.contains("read") || error.contains("file") {
                                "body_read_failed".to_string()
                            } else {
                                "activation_rejected".to_string()
                            },
                        });
                    } else if !loaded_skills.is_empty() {
                        let records: Vec<mini_agent_protocol::SkillLoadRecord> = loaded_skills
                            .iter()
                            .map(|skill| mini_agent_protocol::SkillLoadRecord {
                                name: skill.name.clone(),
                                qualified_name: Some(skill.qualified_name.clone()),
                                source: skill.source.clone(),
                                group: skill.group.clone(),
                            })
                            .collect();
                        skill_prelude.push(Event::SkillsLoaded {
                            phase: SkillLoadPhase::Started,
                            activation: Some("explicit".to_string()),
                            skills: records.clone(),
                        });
                        skill_prelude.push(Event::SkillsLoaded {
                            phase: SkillLoadPhase::Loaded,
                            activation: Some("explicit".to_string()),
                            skills: records,
                        });
                    }
                    let started_at_ms = timestamp_ms();
                    let prompt = input.text.clone();
                    if let Some(operation_id) = operation_id.as_deref()
                        && skill_error.is_none()
                        && let Err(error) = runtime_actor::record_operation(
                            &mut runtime,
                            operation_record(
                                operation_id,
                                "running",
                                Some(turn_id.as_str()),
                                operation_attempt,
                                operation_attempt_kind,
                                None,
                                None,
                                operation_group_id.as_deref(),
                                execution_mode.as_deref(),
                                group_sequence,
                                Some(prompt.as_str()),
                            ),
                        )
                    {
                        eprintln!("warning: failed to persist child operation start: {error}");
                    }
                    let previous_message_count = thread.harness().messages().len();
                    let stopping = Arc::new(AtomicBool::new(false));
                    if let Some(reply) = initial_reply.take() {
                        if current_turn_source
                            == Some(mini_agent_protocol::TurnSource::SessionResume)
                            && let Some(state) = runtime.as_mut()
                            && let Ok(Some(control)) =
                                state.management.session_control_state_if_persisted()
                            && control.status
                                == mini_agent_capabilities::SessionControlStatus::Resuming
                            && let Some(request_id) = control.request_id.as_deref()
                        {
                            let params = mini_agent_app_server_protocol::SessionControlParams {
                                thread_id: state.management.thread_id(),
                                action: mini_agent_app_server_protocol::SessionControlAction::ResumeSettled,
                                request_id: Some(request_id.to_string()),
                            };
                            if let Err(error) = state.management.session_control_action(&params) {
                                eprintln!(
                                    "warning: failed to settle Session resume state: {error}"
                                );
                            }
                        }
                        runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                        respond(
                            reply,
                            receipt.clone(),
                            Ok(TurnSubmission::Started {
                                turn_id: turn_id.clone(),
                            }),
                        );
                    }
                    let mut input = if execution_resume.is_some() {
                        input
                    } else {
                        let mut input = TurnInput::new(TurnInputMode::Start, input.text);
                        input.selected_skills = selected_skills;
                        input.workflow = workflow;
                        input
                    };
                    input.mode = TurnInputMode::Start;
                    let skill_paths = runtime
                        .as_ref()
                        .and_then(|state| state.management.skill_discovery.as_ref())
                        .map(|discovery| discovery.skill_path_records())
                        .unwrap_or_default();
                    let execution_progress = ExecutionProgressMonitor::new();
                    let mut execution_journal = runtime.as_ref().and_then(|state| {
                        state
                            .management
                            .execution_journal(thread.harness().messages())
                    });
                    let heartbeat_task = execution_journal.clone().map(|mut journal| {
                        let turn_id = turn_id.clone();
                        let progress = execution_progress.clone();
                        tokio::spawn(async move {
                            let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
                            heartbeat.tick().await;
                            loop {
                                heartbeat.tick().await;
                                let _ = journal.append(ExecutionJournalEntry::Heartbeat {
                                    turn_id: turn_id.clone(),
                                    phase: progress.phase(),
                                    at_ms: timestamp_ms(),
                                    last_progress_ms: progress.last_progress_ms(),
                                });
                            }
                        })
                    });
                    let mut sink = ThreadListener {
                        events: events.clone(),
                        notifications: notifications.clone(),
                        event_replay: event_replay.clone(),
                        runtime_status: runtime_status.clone(),
                        runtime_revision: runtime_revision.clone(),
                        stopping: stopping.clone(),
                        pending_finish: None,
                        tool_arguments: Vec::new(),
                        skill_paths,
                        pending_skill_reads: BTreeMap::new(),
                        started_skill_reads: BTreeSet::new(),
                        loaded_skill_reads: BTreeSet::new(),
                        failed_skill_reads: BTreeSet::new(),
                        presentation: mini_agent_capabilities::TurnPresentation::from_workflow(
                            input.workflow.as_ref(),
                        )
                        .with_turn_source(current_turn_source),
                        turn_source: current_turn_source,
                        assistant_segments: 0,
                        tokens_used: 0,
                        execution_progress,
                    };
                    let journal = execution_journal
                        .as_mut()
                        .map(|journal| journal as &mut dyn ExecutionJournalSink);
                    let is_execution_resume = execution_resume.is_some();
                    let mut turn = Box::pin(
                        thread.run_turn_with_events_and_preflight_and_journal_resume(
                            input,
                            &mut sink,
                            &control,
                            if is_child_task {
                                SteeringMode::ContinueSameTurn
                            } else {
                                SteeringMode::StopAtCheckpoint
                            },
                            mini_agent_core::TurnExecutionOptions {
                                prelude: &skill_prelude,
                                preflight_error: skill_error.as_deref(),
                                journal,
                                resume: execution_resume.take(),
                            },
                        ),
                    );
                    let timeout_deadline = goal_state
                        .as_ref()
                        .filter(|goal| goal.milestone_timeout_secs > 0)
                        .map(|goal| {
                            Instant::now() + Duration::from_secs(goal.milestone_timeout_secs)
                        });
                    let timeout_configured = timeout_deadline.is_some();
                    let timeout_deadline = timeout_deadline.unwrap_or_else(|| {
                        Instant::now() + Duration::from_secs(365 * 24 * 60 * 60)
                    });
                    let mut timeout_requested = false;
                    let turn_result = loop {
                        let timeout_active = timeout_configured && !timeout_requested;
                        tokio::select! {
                            biased;
                            Some(command) = commands.recv() => {
                                let base_revision = runtime
                                    .as_ref()
                                    .map(RuntimeActorState::revision)
                                    .unwrap_or_default();
                                let action = action_sequencer.admit(
                                    command,
                                    base_revision,
                                    runtime_revision.clone(),
                                );
                                handle_running_command(
                                    action,
                                    &control,
                                    &thread_id,
                                    &turn_id,
                                    &mut deferred_goal_verifications,
                                    RunningCommandContext {
                                        runtime: &mut runtime,
                                        threads: &mut threads,
                                        runtime_revision: &runtime_revision,
                                        runtime_status: &runtime_status,
                                        notifications: &notifications,
                                        stopping: &stopping,
                                    },
                                );
                            },
                            result = &mut turn => break result,
                            _ = tokio::time::sleep_until(timeout_deadline), if timeout_active => {
                                timeout_requested = true;
                                control.request_cancel();
                            },
                            else => {
                                drop(turn);
                                thread.harness_mut().replace_config(original_config);
                                threads.insert(thread);
                                return;
                            },
                        }
                    };
                    drop(turn);
                    if let Some(heartbeat_task) = heartbeat_task {
                        heartbeat_task.abort();
                    }
                    thread.harness_mut().replace_config(original_config);
                    let presentation = sink.take_presentation();
                    let mut goal_turn_completed = false;
                    let mut goal_budget_exhausted = false;
                    let mut goal_step_limited = false;
                    match turn_result {
                        Ok(result) => {
                            goal_step_limited =
                                result.status == mini_agent_protocol::TurnStatus::StepLimit;
                            let projected = project_turn_result(&result);
                            let turn_messages = projected
                                .messages
                                .get(previous_message_count..)
                                .unwrap_or(&projected.messages);
                            let tool_arguments = sink.take_tool_arguments();
                            let persistence_error = runtime_actor::persist_turn(
                                &mut runtime,
                                &thread,
                                started_at_ms,
                                &prompt,
                                crate::management::TurnPersistence {
                                    result: &projected,
                                    messages: turn_messages,
                                    tool_arguments: &tool_arguments,
                                    presentation: Some(&presentation),
                                    execution_resume: is_execution_resume,
                                },
                            )
                            .err()
                            .map(|error| error.to_string());
                            settle_execution_checkpoint(
                                &runtime,
                                &mut execution_journal,
                                &result.id,
                                persistence_error.is_none()
                                    && (result.status
                                        == mini_agent_protocol::TurnStatus::Completed
                                        || (result.status
                                            == mini_agent_protocol::TurnStatus::Cancelled
                                            && !cancelled_execution_requires_resume(
                                                &runtime,
                                                operation_id.as_deref(),
                                                &result.id,
                                            ))),
                                if persistence_error.is_some() {
                                    "session_persistence_failed"
                                } else {
                                    match result.status {
                                        mini_agent_protocol::TurnStatus::Cancelled => "interrupted",
                                        mini_agent_protocol::TurnStatus::Steered => "steered",
                                        mini_agent_protocol::TurnStatus::StepLimit => "step_limit",
                                        _ => "turn_incomplete",
                                    }
                                },
                            );
                            if let Some(operation_id) = operation_id.as_deref() {
                                let operation_status = if persistence_error.is_some() {
                                    "failed"
                                } else {
                                    match result.status {
                                        mini_agent_protocol::TurnStatus::Completed => "completed",
                                        mini_agent_protocol::TurnStatus::Cancelled => "cancelled",
                                        _ => "failed",
                                    }
                                };
                                let step_limit_error = (result.status
                                    == mini_agent_protocol::TurnStatus::StepLimit)
                                    .then(|| {
                                        format!(
                                            "Turn reached its step limit after {} steps",
                                            result.outcome.steps
                                        )
                                    });
                                let operation_error = match (
                                    persistence_error.as_deref(),
                                    step_limit_error.as_deref(),
                                ) {
                                    (Some(persistence), Some(step_limit)) => {
                                        Some(format!("{step_limit}; {persistence}"))
                                    }
                                    (Some(persistence), None) => Some(persistence.to_string()),
                                    (None, Some(step_limit)) => Some(step_limit.to_string()),
                                    (None, None) => None,
                                };
                                let operation_result = if operation_status == "completed" {
                                    Some(result.outcome.final_text.as_str())
                                } else {
                                    None
                                };
                                let operation = operation_record(
                                    operation_id,
                                    operation_status,
                                    Some(result.id.as_str()),
                                    operation_attempt,
                                    operation_attempt_kind,
                                    operation_result,
                                    operation_error.as_deref(),
                                    operation_group_id.as_deref(),
                                    execution_mode.as_deref(),
                                    group_sequence,
                                    Some(prompt.as_str()),
                                );
                                if let Err(error) =
                                    runtime_actor::record_operation(&mut runtime, operation.clone())
                                {
                                    eprintln!(
                                        "warning: failed to persist child operation result: {error}"
                                    );
                                    if operation.result.is_some() {
                                        let mut status_only = operation;
                                        status_only.result = None;
                                        if let Err(status_error) = runtime_actor::record_operation(
                                            &mut runtime,
                                            status_only,
                                        ) {
                                            eprintln!(
                                                "warning: failed to persist child operation status: {status_error}"
                                            );
                                        }
                                    }
                                }
                            }
                            if persistence_error.is_none()
                                && let Some(state) = runtime.as_ref()
                            {
                                runtime_actor::notify_checkpoint_committed(
                                    &runtime,
                                    result.id.clone(),
                                    state.management.current_checkpoint_seq(),
                                );
                                if state.goal_runtime_handle.plan_active() {
                                    if result.status == mini_agent_protocol::TurnStatus::Completed
                                        && persistence_error.is_none()
                                        && let Err(error) =
                                            state.goal_runtime_handle.set_plan_review_pending(true)
                                    {
                                        eprintln!(
                                            "warning: failed to persist Plan review state: {error}"
                                        );
                                    }
                                    runtime_actor::notify_plan_updated(&runtime, true);
                                }
                            }
                            goal_turn_completed = result.status
                                == mini_agent_protocol::TurnStatus::Completed
                                && persistence_error.is_none();
                            settled_turns.insert(
                                result.id.as_str().to_string(),
                                SettledTurn {
                                    id: result.id,
                                    status: result.status,
                                    outcome: Some(result.outcome),
                                    error: persistence_error,
                                    recovery: runtime
                                        .as_ref()
                                        .and_then(|state| state.management.execution_state())
                                        .map(execution_recovery_info),
                                },
                            );
                        }
                        Err(error) => {
                            let error = error.to_string();
                            let projected = TurnReadResult {
                                turn_id: turn_id.clone(),
                                status: mini_agent_protocol::TurnStatus::Failed,
                                stop_reason: None,
                                final_text: None,
                                steps: 0,
                                messages: Vec::new(),
                                items: Vec::new(),
                                error: Some(error.clone()),
                                recovery: runtime
                                    .as_ref()
                                    .and_then(|state| state.management.execution_state())
                                    .map(execution_recovery_info),
                            };
                            let persistence_error = runtime_actor::persist_turn(
                                &mut runtime,
                                &thread,
                                started_at_ms,
                                &prompt,
                                crate::management::TurnPersistence {
                                    result: &projected,
                                    messages: &projected.messages,
                                    tool_arguments: &[],
                                    presentation: Some(&presentation),
                                    execution_resume: is_execution_resume,
                                },
                            )
                            .err()
                            .map(|persist_error| {
                                format!("{error}; session persistence failed: {persist_error}")
                            });
                            settle_execution_checkpoint(
                                &runtime,
                                &mut execution_journal,
                                &turn_id,
                                false,
                                "turn_failed",
                            );
                            let operation_error = persistence_error.as_deref().unwrap_or(&error);
                            if let Some(operation_id) = operation_id.as_deref()
                                && let Err(persist_error) = runtime_actor::record_operation(
                                    &mut runtime,
                                    operation_record(
                                        operation_id,
                                        "failed",
                                        Some(turn_id.as_str()),
                                        operation_attempt,
                                        operation_attempt_kind,
                                        None,
                                        Some(operation_error),
                                        operation_group_id.as_deref(),
                                        execution_mode.as_deref(),
                                        group_sequence,
                                        Some(prompt.as_str()),
                                    ),
                                )
                            {
                                eprintln!(
                                    "warning: failed to persist child operation failure: {persist_error}"
                                );
                            }
                            if persistence_error.is_none()
                                && let Some(state) = runtime.as_ref()
                            {
                                runtime_actor::notify_checkpoint_committed(
                                    &runtime,
                                    turn_id.clone(),
                                    state.management.current_checkpoint_seq(),
                                );
                                if state.goal_runtime_handle.plan_active() {
                                    if let Err(error) =
                                        state.goal_runtime_handle.set_plan_review_pending(false)
                                    {
                                        eprintln!(
                                            "warning: failed to clear Plan review state: {error}"
                                        );
                                    }
                                    runtime_actor::notify_plan_updated(&runtime, true);
                                }
                            }
                            settled_turns.insert(
                                turn_id.as_str().to_string(),
                                SettledTurn {
                                    id: turn_id.clone(),
                                    status: mini_agent_protocol::TurnStatus::Failed,
                                    outcome: None,
                                    error: Some(operation_error.to_string()),
                                    recovery: runtime
                                        .as_ref()
                                        .and_then(|state| state.management.execution_state())
                                        .map(execution_recovery_info),
                                },
                            );
                        }
                    }
                    if runtime
                        .as_ref()
                        .is_some_and(|state| state.goal_runtime_handle.plan_active())
                    {
                        runtime_actor::notify_plan_cleanup(
                            &runtime,
                            Some(turn_id.clone()),
                            true,
                            None,
                        );
                        match runtime_actor::cleanup_plan_scratch(&mut runtime) {
                            Ok(()) => runtime_actor::notify_plan_cleanup(
                                &runtime,
                                Some(turn_id.clone()),
                                false,
                                None,
                            ),
                            Err(error) => {
                                runtime_actor::notify_plan_cleanup(
                                    &runtime,
                                    Some(turn_id.clone()),
                                    false,
                                    Some(&error.to_string()),
                                );
                                eprintln!("warning: Plan cleanup_pending: {error}");
                            }
                        }
                    }
                    if let Some(goal_id) = goal_id.as_deref()
                        && sink.tokens_used > 0
                        && let Ok(Some(goal)) = runtime_actor::goal_turn_usage(
                            &mut runtime,
                            goal_id,
                            &turn_id,
                            sink.tokens_used,
                        )
                    {
                        goal_budget_exhausted =
                            goal.status == mini_agent_host::GoalStatus::BudgetLimited;
                    }
                    if let Some(event) = sink.take_pending_finish() {
                        sink.send_event(event);
                    }
                    if let Some(goal_id) = goal_id {
                        if goal_turn_completed && !goal_budget_exhausted && !timeout_requested {
                            let settled =
                                runtime_actor::goal_turn_settled(&mut runtime, &goal_id, &turn_id)
                                    .unwrap_or(false);
                            if settled
                                && let Ok(Some(request)) =
                                    runtime_actor::prepare_goal_verification_or_fail(
                                        &mut runtime,
                                        &thread,
                                        &goal_id,
                                        &turn_id,
                                    )
                                && let Some(command_sender) =
                                    runtime.as_ref().map(|state| state.commands.clone())
                            {
                                spawn_goal_verifier(command_sender, request);
                            }
                        } else if !goal_budget_exhausted {
                            if timeout_requested || goal_step_limited {
                                let _ = runtime_actor::goal_turn_limited(
                                    &mut runtime,
                                    &goal_id,
                                    &turn_id,
                                    mini_agent_host::GoalStatus::UsageLimited,
                                    if timeout_requested {
                                        "goal milestone timed out"
                                    } else {
                                        "goal milestone step budget exhausted"
                                    },
                                );
                            } else {
                                let _ = runtime_actor::goal_turn_limited(
                                    &mut runtime,
                                    &goal_id,
                                    &turn_id,
                                    mini_agent_host::GoalStatus::Failed,
                                    "goal turn did not complete successfully",
                                );
                            }
                        }
                    }
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                    let current_status = runtime_status.lock().unwrap().clone();
                    let should_publish_terminal = matches!(
                        current_status.phase,
                        RuntimePhase::StartingTurn
                            | RuntimePhase::Model
                            | RuntimePhase::Tool
                            | RuntimePhase::Stopping
                            | RuntimePhase::Compaction
                            | RuntimePhase::Persisting
                    ) || (current_status.phase
                        == RuntimePhase::Failed
                        && current_status.error.is_none());
                    if should_publish_terminal
                        && let Some(settled) = settled_turns.get(turn_id.as_str())
                    {
                        status::publish(
                            &runtime_status,
                            &notifications,
                            thread_id.clone(),
                            if settled.status == mini_agent_protocol::TurnStatus::Completed {
                                RuntimePhase::Completed
                            } else {
                                RuntimePhase::Failed
                            },
                            Some(turn_id.clone()),
                            Some(status::operation("turn", turn_id.as_str())),
                            runtime
                                .as_ref()
                                .map(|state| state.management.current_checkpoint_seq()),
                            &runtime_revision,
                            settled.error.as_deref(),
                        );
                    }
                    next_input = control
                        .take_steer_input()
                        .or_else(|| control.take_follow_up_input());
                    if next_input.is_some() {
                        let session_allows_continuation = runtime.as_mut().is_none_or(|state| {
                            state
                                .management
                                .session_control_state_if_persisted()
                                .is_ok_and(|control| {
                                    control.is_none_or(|control| {
                                        control.status
                                            == mini_agent_capabilities::SessionControlStatus::Running
                                    })
                                })
                        });
                        if !session_allows_continuation {
                            // A queued steer/follow-up is another Turn on this same
                            // Session. Do not let the in-worker continuation path
                            // bypass a parent freeze that arrived during the Turn.
                            next_input = None;
                        }
                    }
                    if next_input.is_none() {
                        while let Some(command) = deferred_goal_verifications.pop_front() {
                            if let Command::GoalVerificationCompleted {
                                thread_id,
                                goal_id,
                                turn_id,
                                checkpoint_seq,
                                result,
                            } = command
                            {
                                complete_goal_verification(
                                    &mut runtime,
                                    &runtime_revision,
                                    thread_id,
                                    goal_id,
                                    turn_id,
                                    checkpoint_seq,
                                    result,
                                );
                            }
                        }
                    }
                    operation_id = None;
                    origin = TurnOrigin::Client;
                    if next_input.is_none() {
                        break;
                    }
                }
                threads.insert(thread);
                if goal_turn
                    && let Err(error) =
                        runtime_actor::restore_thread_continuation(&mut runtime, &mut threads)
                {
                    let error_text = error.to_string();
                    report_runtime_failure(
                        &runtime_status,
                        &notifications,
                        &runtime_revision,
                        "restore-continuation",
                        &error_text,
                    );
                    eprintln!("warning: failed to restore Thread continuation: {error}");
                }
            }
            Command::GoalVerificationCompleted {
                thread_id,
                goal_id,
                turn_id,
                checkpoint_seq,
                result,
            } => {
                complete_goal_verification(
                    &mut runtime,
                    &runtime_revision,
                    thread_id,
                    goal_id,
                    turn_id,
                    checkpoint_seq,
                    result,
                );
                if let Err(error) =
                    runtime_actor::restore_thread_continuation(&mut runtime, &mut threads)
                {
                    let error_text = error.to_string();
                    report_runtime_failure(
                        &runtime_status,
                        &notifications,
                        &runtime_revision,
                        "restore-continuation",
                        &error_text,
                    );
                    eprintln!("warning: failed to restore Thread continuation: {error}");
                }
            }
            Command::Cancel { reply, .. } => {
                respond(reply, receipt, Err(AppServerError::NoActiveTurn));
            }
            Command::ReadThread { thread_id, reply } => {
                let result = threads
                    .get(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| {
                        thread
                            .checkpoint()
                            .map(|checkpoint| mini_agent_app_server_protocol::ThreadReadResult {
                                thread_id: checkpoint.thread_id,
                                status: checkpoint.status,
                                messages: checkpoint.session.messages().to_vec(),
                                context_revision: checkpoint.session.context_revision(),
                                next_turn_number: checkpoint.next_turn_number,
                                last_turn_id: checkpoint.last_turn_id,
                                next_event_sequence: checkpoint.next_event_sequence,
                                execution_recovery: runtime
                                    .as_ref()
                                    .and_then(|state| state.management.execution_state())
                                    .filter(|state| {
                                        state.status
                                            != mini_agent_capabilities::SessionExecutionStatus::Settled
                                    })
                                    .map(execution_recovery_info),
                            })
                            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                    });
                respond(reply, receipt, result);
            }
            Command::UpdateThread {
                thread_id,
                update,
                reply,
            } => {
                let result = threads
                    .get_mut(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| apply_thread_update(thread, update));
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::ResetThread {
                thread_id,
                new_thread_id,
                next_turn_number,
                reply,
            } => {
                let result = threads
                    .rename(&thread_id, new_thread_id.clone(), next_turn_number)
                    .map(|()| new_thread_id);
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::CloseThread { thread_id, reply } => {
                let active_thread_id = thread_id.clone();
                let mut result = threads
                    .get_mut(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| {
                        thread
                            .close()
                            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                    });
                if result.is_ok()
                    && runtime
                        .as_ref()
                        .is_some_and(|state| state.management.thread_id() == active_thread_id)
                    && let Some(state) = runtime.as_ref()
                    && let Err(error) = state.background_shells.close_all()
                {
                    result = Err(AppServerError::Checkpoint(format!(
                        "failed to stop background Shell tasks: {error}"
                    )));
                }
                if result.is_ok()
                    && runtime
                        .as_ref()
                        .is_some_and(|state| state.management.thread_id() == active_thread_id)
                    && let Some(state) = runtime.as_ref()
                    && let Err(error) = state.scheduled_tasks.close_all()
                {
                    result = Err(AppServerError::Checkpoint(format!(
                        "failed to clear scheduled tasks: {error}"
                    )));
                }
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::ReadTurn { turn_id, reply } => {
                let recovery = runtime
                    .as_ref()
                    .and_then(|state| state.management.execution_state())
                    .filter(|state| state.checkpoint.turn_id == turn_id)
                    .map(execution_recovery_info);
                let mut result = settled_turns.get(turn_id.as_str()).cloned();
                if let Some(result) = result.as_mut() {
                    result.recovery = recovery;
                } else if let Some(recovery) = recovery.filter(|recovery| {
                    recovery.status
                        != mini_agent_app_server_protocol::ExecutionRecoveryStatus::Settled
                }) {
                    result = Some(SettledTurn {
                        id: turn_id.clone(),
                        status: mini_agent_protocol::TurnStatus::InProgress,
                        outcome: None,
                        error: None,
                        recovery: Some(recovery),
                    });
                }
                respond(reply, receipt, Ok(result));
            }
            Command::ReadItems { params, reply } => {
                let result = project_thread_items(&threads, runtime.as_ref(), &params);
                respond(reply, receipt, result);
            }
            Command::CreateThread { thread_id, reply } => {
                let result = threads.create(thread_id);
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::ForkThread {
                source_thread_id,
                new_thread_id,
                reply,
            } => {
                let result = threads.fork(source_thread_id, new_thread_id);
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::ResumeThread {
                thread_id,
                checkpoint,
                reply,
            } => {
                let result = threads.resume(thread_id, checkpoint);
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                    match runtime_actor::resume_goal(&mut runtime, &mut threads) {
                        Ok(Some(request)) => {
                            if let Some(command_sender) =
                                runtime.as_ref().map(|state| state.commands.clone())
                            {
                                spawn_goal_verifier(command_sender, request);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let error_text = error.to_string();
                            report_runtime_failure(
                                &runtime_status,
                                &notifications,
                                &runtime_revision,
                                "resume-goal",
                                &error_text,
                            );
                            eprintln!("warning: failed to resume goal runtime: {error}")
                        }
                    }
                }
                respond(reply, receipt, result);
            }
            Command::InstallRuntime { .. } => {
                unreachable!("runtime installation is handled before action admission")
            }
            Command::Shutdown { .. } => {
                unreachable!("worker shutdown is handled before action admission")
            }
        }
    }
}

fn spawn_goal_verifier(
    commands: mpsc::Sender<Command>,
    request: crate::goal_runtime::GoalVerificationRequest,
) {
    tokio::spawn(async move {
        let result = crate::verifier::verify_goal_checkpoint(
            &request.runtime_config,
            request.verifier_model_selection.as_ref(),
            &request.messages,
            &request.criteria,
        )
        .await;
        let _ = commands
            .send(Command::GoalVerificationCompleted {
                thread_id: request.thread_id,
                goal_id: request.goal_id,
                turn_id: request.turn_id,
                checkpoint_seq: request.checkpoint_seq,
                result,
            })
            .await;
    });
}

fn complete_goal_verification(
    runtime: &mut Option<RuntimeActorState>,
    runtime_revision: &AtomicU64,
    thread_id: ThreadId,
    goal_id: String,
    turn_id: TurnId,
    checkpoint_seq: u64,
    result: Result<(String, crate::goal_service::VerifierVerdict), String>,
) {
    if let Err(error) = runtime_actor::complete_goal_verification(
        runtime,
        runtime_revision,
        thread_id,
        goal_id.clone(),
        turn_id.clone(),
        checkpoint_seq,
        result,
    ) {
        runtime_actor::notify_goal_verification_failed(
            runtime,
            &goal_id,
            turn_id,
            &error.to_string(),
        );
        eprintln!("warning: Goal verification completion failed: {error}");
    }
}
pub(super) fn apply_thread_update<M>(
    thread: &mut Thread<M>,
    update: ThreadUpdate,
) -> Result<(), AppServerError>
where
    M: Model,
{
    if thread.status() == mini_agent_protocol::ThreadStatus::Running {
        return Err(AppServerError::Busy);
    }
    match update {
        ThreadUpdate::ClearHistory => thread.harness_mut().clear_history(),
        ThreadUpdate::AppendContext(text) => thread
            .harness_mut()
            .append_context(text)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?,
        ThreadUpdate::AppendContextIfChanged { slot, text } => {
            thread
                .harness_mut()
                .append_context_slot_if_changed(&slot, text)
                .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        }
        ThreadUpdate::ReplaceConfig(config) => thread.harness_mut().replace_config(config),
        ThreadUpdate::ExtendTools(tools) => thread.harness_mut().extend_tools(tools),
    }
    Ok(())
}

fn skill_activation_context(
    loaded_skills: &[mini_agent_capabilities::LoadedSkill],
    workflow: Option<&mini_agent_protocol::TurnWorkflow>,
    turn_id: &str,
) -> String {
    let mut sections = Vec::new();
    if let Some(workflow) = workflow {
        sections.push(format!(
            "Active Skill group: {}. Use its available metadata to choose relevant instructions, then read matching SKILL.md files with read_file before acting. Do not load unrelated Skill bodies.",
            workflow.id
        ));
    }
    if !loaded_skills.is_empty() {
        sections.push(
            loaded_skills
                .iter()
                .map(|skill| {
                    let identity = format!("skill\n{}", skill.qualified_name);
                    let suffix =
                        mini_agent_protocol::stable_digest(identity.as_bytes()).replace('-', "_");
                    let fingerprint = mini_agent_protocol::stable_digest(skill.body.as_bytes());
                    format!(
                        "- {} · {} · context=<skill_definition_{}> · fingerprint={fingerprint}",
                        skill.qualified_name, skill.source, suffix
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
    if sections.is_empty() {
        sections.push("No Skills are explicitly active for this turn.".to_string());
    }
    let body = format!(
        "Explicitly activated Skills apply to turn {turn_id} only. This snapshot supersedes earlier activation snapshots.\n{}",
        sections.join("\n\n")
    );
    let fingerprint = mini_agent_protocol::stable_digest(body.as_bytes());
    format!("<activated_skills fingerprint=\"{fingerprint}\">\n{body}\n</activated_skills>")
}

fn respond_after_revision<T>(
    runtime: &mut Option<RuntimeActorState>,
    runtime_revision: &AtomicU64,
    reply: oneshot::Sender<ActionResult<T>>,
    receipt: ActionReceipt,
    result: Result<T, AppServerError>,
) {
    if result.is_ok() {
        runtime_actor::advance_revision(runtime, runtime_revision);
    }
    respond(reply, receipt, result);
}

fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn settle_execution_checkpoint(
    runtime: &Option<RuntimeActorState>,
    journal: &mut Option<mini_agent_capabilities::SessionExecutionJournal>,
    turn_id: &TurnId,
    completed: bool,
    reason: &str,
) {
    let Some(state) = runtime
        .as_ref()
        .and_then(|runtime| runtime.management.execution_state())
        .filter(|state| state.checkpoint.turn_id == *turn_id)
    else {
        return;
    };
    if state.status == mini_agent_capabilities::SessionExecutionStatus::NeedsReconciliation {
        return;
    }
    let Some(journal) = journal.as_mut() else {
        return;
    };
    let entry = if completed {
        ExecutionJournalEntry::Settled {
            turn_id: turn_id.clone(),
        }
    } else {
        ExecutionJournalEntry::WaitingForContinue {
            turn_id: turn_id.clone(),
            reason: reason.to_string(),
        }
    };
    if let Err(error) = journal.append(entry) {
        eprintln!("warning: failed to persist execution recovery state: {error}");
    }
}

fn cancelled_execution_requires_resume(
    runtime: &Option<RuntimeActorState>,
    operation_id: Option<&str>,
    turn_id: &TurnId,
) -> bool {
    let Some(management) = runtime.as_ref().map(|runtime| &runtime.management) else {
        return false;
    };
    if management
        .session_control_state_if_persisted()
        .ok()
        .flatten()
        .is_some_and(|control| {
            matches!(
                control.status,
                mini_agent_capabilities::SessionControlStatus::Freezing
                    | mini_agent_capabilities::SessionControlStatus::Frozen
                    | mini_agent_capabilities::SessionControlStatus::Resuming
            )
        })
    {
        return true;
    }
    operation_id
        .and_then(|_| {
            management
                .session_operation_for_turn(turn_id.as_str())
                .ok()
                .flatten()
        })
        .is_some_and(|operation| operation.control_action.as_deref() == Some("pause"))
}

fn execution_recovery_info(
    state: mini_agent_capabilities::SessionExecutionState,
) -> mini_agent_app_server_protocol::ExecutionRecoveryInfo {
    mini_agent_app_server_protocol::ExecutionRecoveryInfo {
        turn_id: state.checkpoint.turn_id,
        status: match state.status {
            mini_agent_capabilities::SessionExecutionStatus::Running => {
                mini_agent_app_server_protocol::ExecutionRecoveryStatus::Running
            }
            mini_agent_capabilities::SessionExecutionStatus::WaitingForContinue => {
                mini_agent_app_server_protocol::ExecutionRecoveryStatus::WaitingForContinue
            }
            mini_agent_capabilities::SessionExecutionStatus::NeedsReconciliation => {
                mini_agent_app_server_protocol::ExecutionRecoveryStatus::NeedsReconciliation
            }
            mini_agent_capabilities::SessionExecutionStatus::Settled => {
                mini_agent_app_server_protocol::ExecutionRecoveryStatus::Settled
            }
        },
        phase: match state.phase {
            ExecutionPhase::ModelRequest => {
                mini_agent_app_server_protocol::ExecutionRecoveryPhase::ModelRequest
            }
            ExecutionPhase::ToolBatch => {
                mini_agent_app_server_protocol::ExecutionRecoveryPhase::ToolBatch
            }
        },
        last_heartbeat_ms: state.last_heartbeat_ms,
        last_progress_ms: state.last_progress_ms,
        checkpoint_seq: state.checkpoint_seq,
        reason: state.reason,
    }
}

fn project_turn_result(result: &TurnResult) -> TurnReadResult {
    TurnReadResult {
        turn_id: result.id.clone(),
        status: result.status,
        stop_reason: Some(result.outcome.stop_reason),
        final_text: Some(result.outcome.final_text.clone()),
        steps: result.outcome.steps,
        messages: result.outcome.messages.clone(),
        items: mini_agent_app_server_protocol::ThreadItem::from_messages(&result.outcome.messages),
        error: None,
        recovery: None,
    }
}

fn handle_running_command<M>(
    action: ActionEnvelope<Command>,
    control: &RunControl,
    active_thread_id: &ThreadId,
    turn_id: &TurnId,
    deferred_goal_verifications: &mut VecDeque<Command>,
    context: RunningCommandContext<'_, M>,
) where
    M: Model + 'static,
{
    let action_base_revision = action.base_revision;
    let receipt = action.receipt();
    match action.command {
        Command::Start {
            thread_id,
            request,
            expected_turn_id,
            origin: _,
            turn_source: _,
            execution_resume: _,
            reply,
        } => {
            if thread_id != *active_thread_id {
                respond(reply, receipt, Err(AppServerError::Busy));
                return;
            }
            if let Some(expected_turn_id) = expected_turn_id
                && expected_turn_id != *turn_id
            {
                respond(
                    reply,
                    receipt,
                    Err(AppServerError::TurnNotActive(expected_turn_id)),
                );
                return;
            }
            if context.stopping.load(Ordering::Acquire) {
                respond(
                    reply,
                    receipt,
                    Ok(TurnSubmission::NotSubmitted {
                        reason: "turn is stopping; wait for turn_finished".to_string(),
                    }),
                );
                return;
            }
            let result = match request.input.mode {
                TurnInputMode::Steer => control
                    .submit(request.input)
                    .map(|()| TurnSubmission::Steered {
                        turn_id: turn_id.clone(),
                    })
                    .map_err(|error| AppServerError::InputQueue(error.to_string())),
                TurnInputMode::FollowUp => control
                    .submit(request.input)
                    .map(|()| TurnSubmission::Queued)
                    .map_err(|error| AppServerError::InputQueue(error.to_string())),
                mode => Ok(TurnSubmission::NotSubmitted {
                    reason: format!("thread is busy; cannot submit {mode:?}"),
                }),
            };
            respond(reply, receipt, result);
        }
        Command::Cancel {
            thread_id,
            request,
            reply,
        } => {
            if thread_id != *active_thread_id {
                respond(reply, receipt, Err(AppServerError::Busy));
                return;
            }
            let result = if request.turn_id == *turn_id {
                context.stopping.store(true, Ordering::Release);
                control.request_cancel();
                let checkpoint_seq = context.runtime_status.lock().unwrap().checkpoint_seq;
                status::publish(
                    context.runtime_status,
                    context.notifications,
                    active_thread_id.clone(),
                    RuntimePhase::Stopping,
                    Some(turn_id.clone()),
                    Some(status::operation("turn", turn_id.as_str())),
                    checkpoint_seq,
                    context.runtime_revision,
                    None,
                );
                Ok(())
            } else {
                Err(AppServerError::TurnNotActive(request.turn_id))
            };
            respond(reply, receipt, result);
        }
        Command::ReadThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::UpdateThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::ResetThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::CloseThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::ReadTurn {
            turn_id: requested_turn_id,
            reply,
        } => {
            if requested_turn_id != *turn_id {
                respond(reply, receipt, Err(AppServerError::Busy));
                return;
            }
            let recovery = context
                .runtime
                .as_ref()
                .and_then(|state| state.management.execution_state())
                .filter(|state| state.checkpoint.turn_id == requested_turn_id)
                .map(execution_recovery_info);
            respond(
                reply,
                receipt,
                Ok(Some(SettledTurn {
                    id: requested_turn_id,
                    status: mini_agent_protocol::TurnStatus::InProgress,
                    outcome: None,
                    error: None,
                    recovery,
                })),
            );
        }
        Command::ReadItems { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::CreateThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::ForkThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        Command::ResumeThread { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
        }
        command @ Command::GoalVerificationCompleted { .. } => {
            deferred_goal_verifications.push_back(command);
        }
        Command::InstallRuntime { .. } => {}
        Command::Shutdown { reply } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::Runtime(request) => runtime_actor::handle_running(
            request,
            receipt,
            action_base_revision,
            context.runtime,
            context.threads,
            context.runtime_revision,
            context.stopping.load(Ordering::Acquire),
        ),
    }
}

const MAX_ITEM_LIST_LIMIT: usize = 128;

fn project_thread_items<M>(
    threads: &ThreadManager<M>,
    runtime: Option<&RuntimeActorState>,
    params: &ThreadItemsListParams,
) -> Result<ThreadItemsListResult, AppServerError>
where
    M: Model + 'static,
{
    let mut entries = runtime
        .and_then(|state| state.management.session_items())
        .map(|items| {
            items
                .iter()
                .filter(|record| record.thread_id == params.thread_id.as_str())
                .filter(|record| {
                    params
                        .turn_id
                        .as_ref()
                        .is_none_or(|turn_id| record.turn_id.as_deref() == Some(turn_id.as_str()))
                })
                .filter_map(|record| {
                    let turn_id = record.turn_id.as_ref()?.clone();
                    let turn_source =
                        runtime.and_then(|state| state.management.session_turn_source(&turn_id));
                    let turn_id = TurnId::new(turn_id);
                    Some(
                        ThreadItem::from_message_with_id_and_arguments(
                            &record.message,
                            record.item_id.clone(),
                            record.arguments.as_ref(),
                        )
                        .into_iter()
                        .map(move |item| ThreadItemEntry {
                            turn_id: turn_id.clone(),
                            turn_source,
                            item,
                        }),
                    )
                })
                .flatten()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if entries.is_empty()
        && runtime
            .and_then(|state| state.management.session_items())
            .is_none()
        && !runtime.is_some_and(|state| state.management.session_is_forked())
    {
        let thread = threads
            .get(params.thread_id.as_str())
            .ok_or_else(|| AppServerError::ThreadNotFound(params.thread_id.clone()))?;
        let checkpoint = thread
            .checkpoint()
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        if let Some(turn_id) = checkpoint.last_turn_id {
            entries = ThreadItem::from_messages(checkpoint.session.messages())
                .into_iter()
                .map(|item| ThreadItemEntry {
                    turn_id: turn_id.clone(),
                    turn_source: None,
                    item,
                })
                .collect();
        }
    }

    let descending = params.sort_direction == Some(ItemSortDirection::Desc);
    if descending {
        entries.reverse();
    }
    let start = params
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse::<usize>()
                .map_err(|_| AppServerError::InvalidItemCursor(cursor.to_string()))
        })
        .transpose()?
        .unwrap_or(0);
    let limit = params
        .limit
        .unwrap_or(MAX_ITEM_LIST_LIMIT as u32)
        .min(MAX_ITEM_LIST_LIMIT as u32) as usize;
    let data = entries
        .iter()
        .skip(start)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let next_cursor =
        (start + data.len() < entries.len()).then(|| (start + data.len()).to_string());
    let backwards_cursor = (!data.is_empty()).then(|| start.saturating_sub(limit).to_string());
    Ok(ThreadItemsListResult {
        data,
        next_cursor,
        backwards_cursor,
    })
}
