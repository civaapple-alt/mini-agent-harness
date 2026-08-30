use super::*;
use crate::action::ActionEnvelope;
use crate::action::ActionReceipt;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::action::ActionSequencer;
use crate::management::RuntimeActorState;
use crate::runtime_actor::RuntimeRequest;
use mini_agent_core::SteeringMode;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;

pub(super) enum Command {
    InstallRuntime {
        state: Box<RuntimeActorState>,
    },
    Runtime(RuntimeRequest),
    Start {
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
        reply: oneshot::Sender<ActionResult<TurnSubmission>>,
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

struct BroadcastSink {
    events: broadcast::Sender<EventEnvelope>,
}

struct RunningCommandContext<'a, M> {
    runtime: &'a mut Option<RuntimeActorState>,
    threads: &'a mut HashMap<String, Thread<M>>,
    thread_ids: &'a Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: &'a Arc<AtomicU64>,
}

impl EventSink for BroadcastSink {
    fn emit(&mut self, event: EventEnvelope) {
        let _ = self.events.send(event);
    }
}

pub(super) async fn worker_loop<M>(
    threads: Vec<Thread<M>>,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<EventEnvelope>,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: Arc<AtomicU64>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
    control: Arc<RunControl>,
) where
    M: Model + Send + 'static,
{
    let mut action_sequencer = ActionSequencer::new();
    let mut runtime = None;
    let mut threads = threads
        .into_iter()
        .map(|thread| (thread.id().as_str().to_string(), thread))
        .collect::<HashMap<_, _>>();
    let mut settled_turns = HashMap::new();
    while let Some(command) = commands.recv().await {
        let base_revision = runtime
            .as_ref()
            .map(RuntimeActorState::revision)
            .unwrap_or_default();
        let action = action_sequencer.admit(command, base_revision);
        let receipt = action.receipt();
        match action.command {
            Command::InstallRuntime { state } => {
                runtime = Some(*state);
            }
            Command::Runtime(request) => {
                runtime_actor::handle_request(
                    request,
                    receipt,
                    &mut runtime,
                    &mut threads,
                    &thread_ids,
                    &runtime_revision,
                );
            }
            Command::Start {
                thread_id,
                request,
                expected_turn_id,
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
                    threads.insert(key, thread);
                    continue;
                }
                if thread.status() == mini_agent_protocol::ThreadStatus::Closed {
                    respond(reply, receipt, Err(AppServerError::Closed));
                    threads.insert(key, thread);
                    continue;
                }
                if thread.status() == mini_agent_protocol::ThreadStatus::Running {
                    respond(reply, receipt, Err(AppServerError::Busy));
                    threads.insert(key, thread);
                    continue;
                }

                let mut next_input = Some(request.input);
                let mut initial_reply = Some(reply);
                loop {
                    let input = next_input
                        .take()
                        .expect("app-server turn input must exist before execution");
                    let turn_id = thread.next_turn_id();
                    if let Some(reply) = initial_reply.take() {
                        runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                        respond(
                            reply,
                            receipt,
                            Ok(TurnSubmission::Started {
                                turn_id: turn_id.clone(),
                            }),
                        );
                    }
                    let input = TurnInput::new(TurnInputMode::Start, input.text);
                    let mut sink = BroadcastSink {
                        events: events.clone(),
                    };
                    let mut turn = Box::pin(thread.run_turn_with_events(
                        input,
                        &mut sink,
                        &control,
                        SteeringMode::StopAtCheckpoint,
                    ));
                    let turn_result = loop {
                        tokio::select! {
                            result = &mut turn => break result,
                            Some(command) = commands.recv() => {
                                let base_revision = runtime
                                    .as_ref()
                                    .map(RuntimeActorState::revision)
                                    .unwrap_or_default();
                                let action = action_sequencer.admit(command, base_revision);
                                handle_running_command(
                                    action,
                                    &control,
                                    &thread_id,
                                    &turn_id,
                                    RunningCommandContext {
                                        runtime: &mut runtime,
                                        threads: &mut threads,
                                        thread_ids: &thread_ids,
                                        runtime_revision: &runtime_revision,
                                    },
                                );
                            },
                            else => {
                                drop(turn);
                                threads.insert(key.clone(), thread);
                                return;
                            },
                        }
                    };
                    match turn_result {
                        Ok(result) => {
                            settled_turns.insert(
                                result.id.as_str().to_string(),
                                SettledTurn {
                                    id: result.id,
                                    status: result.status,
                                    outcome: Some(result.outcome),
                                    error: None,
                                },
                            );
                        }
                        Err(error) => {
                            settled_turns.insert(
                                turn_id.as_str().to_string(),
                                SettledTurn {
                                    id: turn_id.clone(),
                                    status: mini_agent_protocol::TurnStatus::Failed,
                                    outcome: None,
                                    error: Some(error.to_string()),
                                },
                            );
                        }
                    }
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                    next_input = control
                        .take_steer_input()
                        .or_else(|| control.take_follow_up_input());
                    if next_input.is_none() {
                        break;
                    }
                }
                threads.insert(key, thread);
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
                let old_key = thread_id.as_str().to_string();
                let result = if threads.contains_key(new_thread_id.as_str()) {
                    Err(AppServerError::ThreadAlreadyExists(new_thread_id))
                } else if let Some(mut thread) = threads.remove(&old_key) {
                    thread.set_id(new_thread_id.clone());
                    thread.set_next_turn_number(next_turn_number);
                    threads.insert(new_thread_id.as_str().to_string(), thread);
                    if let Some(known) = thread_ids
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .find(|known| **known == thread_id)
                    {
                        *known = new_thread_id.clone();
                    }
                    Ok(new_thread_id)
                } else {
                    Err(AppServerError::ThreadNotFound(thread_id))
                };
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
                let result = create_thread(&mut threads, &thread_ids, factory.as_ref(), thread_id);
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
                let result = fork_thread(
                    &mut threads,
                    &thread_ids,
                    factory.as_ref(),
                    source_thread_id,
                    new_thread_id,
                );
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
                let result = resume_thread(
                    &mut threads,
                    &thread_ids,
                    factory.as_ref(),
                    thread_id,
                    checkpoint,
                );
                if result.is_ok() {
                    runtime_actor::advance_revision(&mut runtime, &runtime_revision);
                }
                respond(reply, receipt, result);
            }
        }
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

fn create_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    thread_id: ThreadId,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    let key = thread_id.as_str().to_string();
    if threads.contains_key(&key) {
        return Err(AppServerError::ThreadAlreadyExists(thread_id));
    }
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut thread = factory.create(thread_id.clone())?;
    thread.set_id(thread_id.clone());
    threads.insert(key, thread);
    thread_ids.lock().unwrap().push(thread_id.clone());
    Ok(thread_id)
}

fn fork_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    source_thread_id: ThreadId,
    new_thread_id: ThreadId,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    let new_key = new_thread_id.as_str().to_string();
    if threads.contains_key(&new_key) {
        return Err(AppServerError::ThreadAlreadyExists(new_thread_id));
    }
    let checkpoint = threads
        .get(source_thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(source_thread_id.clone()))?
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut fork = factory.create(new_thread_id.clone())?;
    let mut checkpoint = checkpoint;
    checkpoint.thread_id = new_thread_id.clone();
    fork.restore_checkpoint(checkpoint)
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    threads.insert(new_key, fork);
    thread_ids.lock().unwrap().push(new_thread_id.clone());
    Ok(new_thread_id)
}

fn resume_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    thread_id: ThreadId,
    mut checkpoint: ThreadCheckpoint,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    checkpoint.thread_id = thread_id.clone();
    let key = thread_id.as_str().to_string();
    if let Some(thread) = threads.get_mut(&key) {
        thread
            .restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        return Ok(thread_id);
    }
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut thread = factory.create(thread_id.clone())?;
    thread
        .restore_checkpoint(checkpoint)
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    threads.insert(key, thread);
    thread_ids.lock().unwrap().push(thread_id.clone());
    Ok(thread_id)
}

fn respond<T>(
    reply: oneshot::Sender<ActionResult<T>>,
    receipt: ActionReceipt,
    result: Result<T, AppServerError>,
) {
    let _ = reply.send(result.map(|value| ActionResponse { value, receipt }));
}

fn handle_running_command<M>(
    action: ActionEnvelope<Command>,
    control: &RunControl,
    active_thread_id: &ThreadId,
    turn_id: &TurnId,
    context: RunningCommandContext<'_, M>,
) where
    M: Model,
{
    let receipt = action.receipt();
    match action.command {
        Command::Start {
            thread_id,
            request,
            expected_turn_id,
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
        Command::InstallRuntime { .. } => {}
        Command::Runtime(request) => runtime_actor::handle_running(
            request,
            receipt,
            context.runtime,
            context.threads,
            context.thread_ids,
            context.runtime_revision,
        ),
    }
}
