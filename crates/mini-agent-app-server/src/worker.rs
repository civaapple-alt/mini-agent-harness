use super::*;
use crate::action::ActionEnvelope;
use crate::action::ActionFailure;
use crate::action::ActionReceipt;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::action::ActionSequencer;
use crate::management::RuntimeActorState;
use crate::notification::RuntimeNotification;
use crate::runtime_actor::RuntimeRequest;
use crate::thread_manager::ThreadHandle;
use crate::thread_manager::ThreadManager;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_core::SteeringMode;
use mini_agent_core::TurnResult;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::ModelUsage;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone)]
pub(super) enum TurnOrigin {
    Client,
    Goal { goal_id: String },
}

pub(super) enum Command {
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
    pending_finish: Option<EventEnvelope>,
    tokens_used: u64,
}

impl ThreadListener {
    fn take_pending_finish(&mut self) -> Option<EventEnvelope> {
        self.pending_finish.take()
    }

    fn send_event(&self, event: EventEnvelope) {
        let _ = self.events.send(event.clone());
        let _ = self.notifications.send(RuntimeNotification::Event(event));
    }

    fn record_usage(&mut self, usage: Option<ModelUsage>) {
        if let Some(usage) = usage {
            self.tokens_used = self
                .tokens_used
                .saturating_add(usage.input_tokens)
                .saturating_add(usage.output_tokens);
        }
    }
}

struct RunningCommandContext<'a, M> {
    runtime: &'a mut Option<RuntimeActorState>,
    threads: &'a mut ThreadManager<M>,
    runtime_revision: &'a Arc<AtomicU64>,
}

impl EventSink for ThreadListener {
    fn emit(&mut self, event: EventEnvelope) {
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
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn worker_loop<M>(
    threads: Vec<Thread<M>>,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<EventEnvelope>,
    notifications: broadcast::Sender<RuntimeNotification>,
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
        if let Command::InstallRuntime { state } = command {
            runtime = Some(*state);
            if runtime
                .as_ref()
                .is_some_and(|state| state.goal_runtime_handle.plan_active())
                && let Some(state) = runtime.as_mut()
                && let Err(error) =
                    runtime_actor::set_collaboration_mode(&mut threads, state, true, None)
            {
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
                Err(error) => eprintln!("warning: failed to resume goal runtime: {error}"),
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
                runtime_actor::handle_request(
                    request,
                    receipt,
                    action_base_revision,
                    &mut runtime,
                    &mut threads,
                    &runtime_revision,
                );
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
                loop {
                    let input = next_input
                        .take()
                        .expect("app-server turn input must exist before execution");
                    let turn_id = thread.next_turn_id();
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
                                    let _ = runtime_actor::goal_turn_failed(
                                        &mut runtime,
                                        goal_id,
                                        &turn_id,
                                        &reason,
                                    );
                                    break;
                                }
                            }
                        }
                        _ => None,
                    };
                    let original_config = thread.harness().config().clone();
                    if let Some(goal) = goal_state.as_ref() {
                        let mut config = original_config.clone();
                        config.max_steps = if goal.milestone_step_budget == 0 {
                            usize::MAX
                        } else {
                            goal.milestone_step_budget
                        };
                        thread.harness_mut().replace_config(config);
                    }
                    let started_at_ms = timestamp_ms();
                    let prompt = input.text.clone();
                    let previous_message_count = thread.harness().messages().len();
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
                    let input = TurnInput::new(TurnInputMode::Start, input.text);
                    let mut sink = ThreadListener {
                        events: events.clone(),
                        notifications: notifications.clone(),
                        pending_finish: None,
                        tokens_used: 0,
                    };
                    let mut turn = Box::pin(thread.run_turn_with_events(
                        input,
                        &mut sink,
                        &control,
                        SteeringMode::StopAtCheckpoint,
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
                            let persistence_error = thread
                                .checkpoint()
                                .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                                .and_then(|checkpoint| {
                                    runtime_actor::persist_turn(
                                        &mut runtime,
                                        started_at_ms,
                                        &prompt,
                                        &projected,
                                        turn_messages,
                                        &checkpoint,
                                    )
                                })
                                .err()
                                .map(|error| error.to_string());
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
                            let persistence_error = thread
                                .checkpoint()
                                .map_err(|checkpoint_error| {
                                    AppServerError::Checkpoint(checkpoint_error.to_string())
                                })
                                .and_then(|checkpoint| {
                                    runtime_actor::persist_turn(
                                        &mut runtime,
                                        started_at_ms,
                                        &prompt,
                                        &projected,
                                        &projected.messages,
                                        &checkpoint,
                                    )
                                })
                                .err()
                                .map(|persist_error| {
                                    format!("{error}; session persistence failed: {persist_error}")
                                });
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
                                let _ = runtime_actor::goal_turn_failed(
                                    &mut runtime,
                                    &goal_id,
                                    &turn_id,
                                    "goal turn did not complete successfully",
                                );
                            }
                        }
                    }
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
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
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
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
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
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
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
            }
            Command::ReadTurn { turn_id, reply } => {
                respond(
                    reply,
                    receipt,
                    Ok(settled_turns.get(turn_id.as_str()).cloned()),
                );
            }
            Command::CreateThread { thread_id, reply } => {
                let result = threads.create(thread_id);
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
            }
            Command::ForkThread {
                source_thread_id,
                new_thread_id,
                reply,
            } => {
                let result = threads.fork(source_thread_id, new_thread_id);
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
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
                            eprintln!("warning: failed to resume goal runtime: {error}")
                        }
                    }
                }
                respond(reply, receipt, result);
            }
            Command::InstallRuntime { .. } => {
                unreachable!("runtime installation is handled before action admission")
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
        goal_id,
        turn_id,
        checkpoint_seq,
        result,
    ) {
        eprintln!("warning: Goal verification completion failed: {error}");
    }
}
pub(super) fn apply_thread_update<M>(
    thread: &mut ThreadHandle<M>,
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

fn respond<T>(
    reply: oneshot::Sender<ActionResult<T>>,
    receipt: ActionReceipt,
    result: Result<T, AppServerError>,
) {
    let state_revision = receipt.current_revision();
    let _ = reply.send(
        result
            .map(|value| ActionResponse {
                value,
                receipt: receipt.clone(),
                state_revision,
            })
            .map_err(|error| ActionFailure {
                error,
                receipt: Some(receipt),
                state_revision: Some(state_revision),
            }),
    );
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
                control.request_cancel();
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
        Command::Runtime(request) => runtime_actor::handle_running(
            request,
            receipt,
            action_base_revision,
            context.runtime,
            context.threads,
            context.runtime_revision,
        ),
    }
}
