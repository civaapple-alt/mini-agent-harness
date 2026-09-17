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
use mini_agent_core::{SteeringMode, TurnResult};
use mini_agent_protocol::{Event, EventEnvelope, EventSink, ModelUsage, SkillLoadPhase};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

const EVENT_REPLAY_BUFFER: usize = 512;

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
        reply: oneshot::Sender<ActionResult<ThreadCheckpoint>>,
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
    event_replay: Arc<Mutex<VecDeque<EventEnvelope>>>,
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
    tokens_used: u64,
}

impl ThreadListener {
    fn take_pending_finish(&mut self) -> Option<EventEnvelope> {
        self.pending_finish.take()
    }

    fn take_tool_arguments(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.tool_arguments)
    }

    fn send_event(&self, event: EventEnvelope) {
        {
            let mut replay = self.event_replay.lock().unwrap();
            if replay.len() == EVENT_REPLAY_BUFFER {
                replay.pop_front();
            }
            replay.push_back(event.clone());
        }
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
        &self,
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
        self.send_event(envelope);
        *next_sequence = (*next_sequence).saturating_add(1);
    }
}

impl EventSink for ThreadListener {
    fn emit(&mut self, event: EventEnvelope) {
        self.update_status_for_event(&event);
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
    event_replay: Arc<Mutex<VecDeque<EventEnvelope>>>,
    runtime_status: RuntimeStatusHandle,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: Arc<AtomicU64>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
    control: Arc<RunControl>,
) where
    M: Model + Send + 'static,
{
    let mut action_sequencer = ActionSequencer::new();
    let mut runtime = None;
    let mut threads = ThreadManager::new(threads, thread_ids.clone(), factory.clone());
    let mut settled_turns = HashMap::new();
    let mut deferred_goal_verifications = VecDeque::new();
    while let Some(command) = commands.recv().await {
        if let Command::Shutdown { reply } = command {
            let _ = reply.send(Ok(()));
            break;
        }
        if let Command::InstallRuntime { state } = command {
            runtime = Some(*state);
            if runtime
                .as_ref()
                .is_some_and(|state| state.goal_runtime_handle.plan_active())
                && let Some(state) = runtime.as_mut()
                && let Err(error) =
                    runtime_actor::set_thread_settings(&mut threads, state, true, None, None)
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
                request,
                expected_turn_id,
                origin,
                reply,
            } => {
                if expected_turn_id.is_some() {
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

                let mut next_input = Some(request.input);
                let mut initial_reply = Some(reply);
                let mut origin = origin;
                let goal_turn = matches!(&origin, TurnOrigin::Goal { .. });
                loop {
                    let input = next_input
                        .take()
                        .expect("app-server turn input must exist before execution");
                    let turn_id = thread.next_turn_id();
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
                    let original_config = thread.harness().config().clone();
                    if let Some(goal) = goal_state.as_ref() {
                        let mut config = original_config.clone().with_copilot_loop();
                        if goal.milestone_step_budget != 0 {
                            config.max_steps = goal.milestone_step_budget;
                        }
                        thread.harness_mut().replace_config(config);
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
                    let group_error = workflow.as_ref().and_then(|workflow| {
                        let result = match workflow.kind {
                            mini_agent_protocol::TurnWorkflowKind::SkillGroup => runtime
                                .as_ref()
                                .and_then(|state| state.management.skill_discovery.as_ref())
                                .ok_or_else(|| {
                                    "skill catalog is unavailable for this runtime".to_string()
                                })
                                .and_then(|discovery| discovery.activate_skill_group(&workflow.id)),
                        };
                        result.err()
                    });
                    let group_valid = group_error.is_none();
                    let (loaded_skills, skill_error) = if let Some(error) = group_error {
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
                        let body = loaded_skills
                            .iter()
                            .map(|skill| format!("### {}\n{}", skill.qualified_name, skill.body))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let mut config = thread.harness().config().clone();
                        config.system_prompt = format!(
                            "{}\n\n## Explicitly activated skills\n{}",
                            config.system_prompt, body
                        );
                        thread.harness_mut().replace_config(config);
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
                    if workflow.is_some() && skill_error.is_none() {
                        let mut config = thread.harness().config().clone();
                        config.system_prompt = format!(
                            "{}\n\n## Active skill group: pstack\n\
                             Use the available pstack Skill metadata to choose relevant \
                             instructions, then read matching SKILL.md files with read_file \
                             before acting. Do not load unrelated Skill bodies.",
                            config.system_prompt
                        );
                        thread.harness_mut().replace_config(config);
                    }
                    let started_at_ms = timestamp_ms();
                    let prompt = input.text.clone();
                    let previous_message_count = thread.harness().messages().len();
                    let stopping = Arc::new(AtomicBool::new(false));
                    if let Some(reply) = initial_reply.take() {
                        runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                        respond(
                            reply,
                            receipt.clone(),
                            Ok(TurnSubmission::Started {
                                turn_id: turn_id.clone(),
                            }),
                        );
                    }
                    let mut input = TurnInput::new(TurnInputMode::Start, input.text);
                    input.selected_skills = selected_skills;
                    input.workflow = workflow;
                    let skill_paths = runtime
                        .as_ref()
                        .and_then(|state| state.management.skill_discovery.as_ref())
                        .map(|discovery| discovery.skill_path_records())
                        .unwrap_or_default();
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
                        tokens_used: 0,
                    };
                    let mut turn = Box::pin(thread.run_turn_with_events_and_preflight(
                        input,
                        &mut sink,
                        &control,
                        SteeringMode::StopAtCheckpoint,
                        &skill_prelude,
                        skill_error.as_deref(),
                    ));
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
                            result = &mut turn => break result,
                            _ = tokio::time::sleep_until(timeout_deadline), if timeout_active => {
                                timeout_requested = true;
                                control.request_cancel();
                            },
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
                            else => {
                                drop(turn);
                                thread.harness_mut().replace_config(original_config);
                                threads.insert(thread);
                                return;
                            },
                        }
                    };
                    drop(turn);
                    thread.harness_mut().replace_config(original_config);
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
                                &projected,
                                turn_messages,
                                &tool_arguments,
                            )
                            .err()
                            .map(|error| error.to_string());
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
                            };
                            let persistence_error = runtime_actor::persist_turn(
                                &mut runtime,
                                &thread,
                                started_at_ms,
                                &prompt,
                                &projected,
                                &projected.messages,
                                &[],
                            )
                            .err()
                            .map(|persist_error| {
                                format!("{error}; session persistence failed: {persist_error}")
                            });
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
                                    error: Some(persistence_error.unwrap_or(error)),
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
                let result = threads
                    .get_mut(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| {
                        thread
                            .close()
                            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                    });
                respond_after_revision(&mut runtime, &runtime_revision, reply, receipt, result);
            }
            Command::ReadTurn { turn_id, reply } => {
                respond(
                    reply,
                    receipt,
                    Ok(settled_turns.get(turn_id.as_str()).cloned()),
                );
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
        ThreadUpdate::ReplaceConfig(config) => thread.harness_mut().replace_config(config),
        ThreadUpdate::ExtendTools(tools) => thread.harness_mut().extend_tools(tools),
    }
    Ok(())
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
        Command::ReadTurn { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy));
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
