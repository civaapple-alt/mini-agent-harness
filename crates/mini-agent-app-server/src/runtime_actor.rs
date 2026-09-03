use crate::AppServerError;
use crate::action::ActionFailure;
use crate::action::ActionReceipt;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::action::RuntimeRevision;
use crate::management::RuntimeActorState;
use crate::management::SettingsRuntimeEvent;
pub(super) use crate::runtime_command::{RuntimeCommand, RuntimeRequest};
use crate::thread_manager::ThreadHandle;
use crate::thread_manager::ThreadManager;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::McpLoadResult;
use mini_agent_capabilities::load_mcp;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::oneshot;

pub(super) fn handle_request<M>(
    request: RuntimeRequest,
    receipt: ActionReceipt,
    base_revision: RuntimeRevision,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
    runtime_revision: &AtomicU64,
) where
    M: Model + 'static,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    handle(request.command, receipt, runtime, threads, runtime_revision);
}

pub(super) fn handle<M>(
    command: RuntimeCommand,
    receipt: ActionReceipt,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
    runtime_revision: &AtomicU64,
) where
    M: Model + 'static,
{
    match command {
        RuntimeCommand::SessionInfo { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .map(|state| Ok(state.management.session_info()))
                .unwrap_or(Err(AppServerError::RuntimeUnavailable)),
        ),
        RuntimeCommand::CheckpointSeq { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .map(|state| Ok(state.management.checkpoint_seq()))
                .unwrap_or(Err(AppServerError::RuntimeUnavailable)),
        ),
        RuntimeCommand::ThreadId { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .map(|state| state.management.thread_id())
                .ok_or(AppServerError::RuntimeUnavailable),
        ),
        RuntimeCommand::World { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .map(|state| state.management.world())
                .ok_or(AppServerError::RuntimeUnavailable),
        ),
        RuntimeCommand::RefreshWorld { reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                let current = state.management.world();
                let refreshed = mini_agent_host::WorldState::detect(
                    current.workspace(),
                    current.approval(),
                    current.copilot(),
                    current.sandbox(),
                );
                update_world(threads, state, refreshed).map(|changed| (changed, changed))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::SetExecution {
            approval,
            copilot,
            reply,
        } => {
            let result = mutate(runtime, runtime_revision, |state| {
                let current = state.management.world();
                update_world(
                    threads,
                    state,
                    current.with_execution(approval, copilot, current.sandbox()),
                )
                .map(|changed| (changed, changed))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::UpdateThread { update, reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                update_thread(threads, state, update).map(|()| ((), true))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::McpStatus { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .map(|state| state.management.mcp_status())
                .ok_or(AppServerError::RuntimeUnavailable),
        ),
        RuntimeCommand::RetryMcp { approval, reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                retry_mcp(threads, state, approval)
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::ReadCheckpoint { reply } => {
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    let thread_id = state.management.thread_id();
                    threads
                        .get(thread_id.as_str())
                        .ok_or(AppServerError::ThreadNotFound(thread_id))
                        .and_then(|thread| {
                            thread
                                .checkpoint()
                                .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                        })
                });
            respond(reply, receipt, result);
        }
        RuntimeCommand::StartNewThread { reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                start_new_thread(threads, state).map(|()| ((), true))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::RuntimeState { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    Ok((
                        state.goal_runtime_handle.plan_active(),
                        state
                            .goal_runtime_handle
                            .load_goal_state()
                            .map_err(workflow_error)?,
                        state.builtin_tools.names().to_vec(),
                    ))
                }),
        ),
        RuntimeCommand::ThreadSettingsUpdate {
            active,
            builtin_tools,
            reply,
        } => {
            let result = mutate::<(Vec<String>, bool), _>(runtime, runtime_revision, |state| {
                let previous_active = state.goal_runtime_handle.plan_active();
                let previous_tools = state.builtin_tools.names().to_vec();
                set_collaboration_mode(threads, state, active, builtin_tools).map(|selection| {
                    let changed = previous_active != active || previous_tools != selection;
                    ((selection, changed), changed)
                })
            });
            let changed = result.as_ref().is_ok_and(|(_, changed)| *changed);
            if changed && let Some(state) = runtime.as_ref() {
                let event = SettingsRuntimeEvent {
                    thread_id: state.management.thread_id(),
                    active,
                    builtin_tools: state.builtin_tools.names().to_vec(),
                    state_revision: state.revision().value(),
                };
                let _ = state.settings_notifications.send(event.clone());
                let _ = state
                    .notifications
                    .send(crate::RuntimeNotification::Settings(event));
            }
            respond(reply, receipt, result.map(|(selection, _)| selection));
        }
        RuntimeCommand::ThreadGoalSet {
            objective,
            status,
            token_budget,
            reply,
        } => {
            let result = mutate(runtime, runtime_revision, |state| {
                let outcome = state
                    .goal_runtime_handle
                    .set_goal(objective.as_deref(), status, token_budget)
                    .map_err(workflow_error)?;
                let changed = outcome.changed();
                let goal = outcome.current;
                state
                    .approval
                    .set_goal_dir(Some(state.goal_runtime_handle.goal_dir()));
                if changed {
                    let thread_id = state.management.thread_id();
                    if goal.status == mini_agent_host::GoalStatus::Running {
                        schedule_goal_turn(state, &goal)?;
                    }
                    state
                        .goal_runtime_handle
                        .notify_updated(thread_id, None, goal.clone());
                }
                Ok((goal, changed))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::ThreadGoalGet { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    state
                        .goal_runtime_handle
                        .load_goal_state()
                        .map_err(workflow_error)
                }),
        ),
        RuntimeCommand::ThreadGoalClear { reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                let cleared = state
                    .goal_runtime_handle
                    .clear_goal()
                    .map_err(workflow_error)?;
                state.approval.set_goal_dir(None);
                if cleared {
                    state
                        .goal_runtime_handle
                        .notify_cleared(state.management.thread_id());
                }
                Ok((cleared, cleared))
            });
            respond(reply, receipt, result);
        }
    }
}

fn reject_runtime(command: RuntimeCommand, receipt: ActionReceipt, error: AppServerError) {
    match command {
        RuntimeCommand::SessionInfo { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::CheckpointSeq { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ThreadId { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::World { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::RefreshWorld { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::SetExecution { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::UpdateThread { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::McpStatus { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::RetryMcp { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ReadCheckpoint { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::StartNewThread { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::RuntimeState { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ThreadSettingsUpdate { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ThreadGoalSet { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ThreadGoalGet { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::ThreadGoalClear { reply } => respond(reply, receipt, Err(error)),
    }
}

pub(super) fn handle_running<M>(
    request: RuntimeRequest,
    receipt: ActionReceipt,
    base_revision: RuntimeRevision,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
    runtime_revision: &AtomicU64,
) where
    M: Model + 'static,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    let command = request.command;
    if command.is_mutation() && !is_safe_goal_mutation_while_running(&command) {
        reject_runtime(command, receipt, AppServerError::Busy);
    } else {
        handle(command, receipt, runtime, threads, runtime_revision);
    }
}

fn is_safe_goal_mutation_while_running(command: &RuntimeCommand) -> bool {
    matches!(
        command,
        RuntimeCommand::ThreadGoalClear { .. }
            | RuntimeCommand::ThreadGoalSet {
                objective: None,
                status: Some(mini_agent_app_server_protocol::ThreadGoalStatus::Paused),
                token_budget: None,
                ..
            }
    )
}

fn check_revision(
    request: &RuntimeRequest,
    runtime: &Option<RuntimeActorState>,
    base_revision: RuntimeRevision,
) -> Result<(), AppServerError> {
    if !request.command.is_mutation() {
        return Ok(());
    }
    let actual = runtime
        .as_ref()
        .map(RuntimeActorState::revision)
        .unwrap_or_default();
    debug_assert_eq!(actual, base_revision);
    if request.expected_revision == base_revision {
        Ok(())
    } else {
        Err(AppServerError::RevisionConflict {
            expected: request.expected_revision.value(),
            actual: base_revision.value(),
        })
    }
}

pub(super) fn set_collaboration_mode<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    active: bool,
    builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
) -> Result<Vec<String>, AppServerError>
where
    M: Model + 'static,
{
    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(thread_id.clone()))?;
    if active {
        let plan_path = state
            .goal_runtime_handle
            .init_plan_mode(None)
            .map_err(workflow_error)?;
        state.approval.set_living_plan(Some(plan_path));
        let base_prompt = match state.stable_system_prompt.as_ref() {
            Some(prompt) => prompt.clone(),
            None => {
                let prompt = thread.harness().system_prompt().to_string();
                state.stable_system_prompt = Some(prompt.clone());
                prompt
            }
        };
        thread
            .harness_mut()
            .set_system_prompt(mini_agent_host::with_plan_mode_overlay(&base_prompt));
    } else {
        state
            .goal_runtime_handle
            .disable_plan_mode()
            .map_err(workflow_error)?;
        state.approval.set_living_plan(None);
        if let Some(prompt) = state.stable_system_prompt.as_deref() {
            thread.harness_mut().set_system_prompt(prompt);
        }
    }
    if let Some(selection) = builtin_tools {
        thread
            .harness_mut()
            .set_hidden_tools(selection.hidden_names());
        state.builtin_tools = selection;
    }
    Ok(state.builtin_tools.names().to_vec())
}

fn update_world<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    updated: mini_agent_host::WorldState,
) -> Result<bool, AppServerError>
where
    M: Model + 'static,
{
    if updated == state.management.world() {
        return Ok(false);
    }
    let context = updated
        .model_context()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    append_context_and_persist(threads, state, context)?;
    state.management.set_world(updated);
    Ok(true)
}

fn schedule_goal_turn(
    state: &mut RuntimeActorState,
    goal: &crate::goal_service::GoalState,
) -> Result<(), AppServerError> {
    if !state.goal_runtime_handle.reserve_turn(&goal.goal_id) {
        return Ok(());
    }
    let (reply, _response) = oneshot::channel();
    if state
        .commands
        .try_send(crate::worker::Command::Start {
            thread_id: state.management.thread_id(),
            request: TurnStart::new(TurnInput::new(
                TurnInputMode::StartIfIdle,
                mini_agent_host::goal_turn_prompt(
                    &goal.objective,
                    goal.current_milestone,
                    goal.total_milestones,
                ),
            )),
            expected_turn_id: None,
            origin: crate::worker::TurnOrigin::Goal {
                goal_id: goal.goal_id.clone(),
            },
            reply,
        })
        .is_err()
    {
        state.goal_runtime_handle.release_turn(&goal.goal_id);
        return Err(AppServerError::Disconnected);
    }
    Ok(())
}

pub(super) fn goal_turn_started(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<bool, AppServerError> {
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let updated = state
        .goal_runtime_handle
        .mark_turn_started(goal_id, turn_id)
        .map_err(workflow_error)?;
    if let Some(goal) = updated {
        state.goal_runtime_handle.notify_updated(
            state.management.thread_id(),
            Some(turn_id.clone()),
            goal,
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn goal_turn_settled(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<bool, AppServerError> {
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let updated = state
        .goal_runtime_handle
        .mark_turn_settled(goal_id, turn_id)
        .map_err(workflow_error)?;
    if let Some(goal) = updated {
        state.goal_runtime_handle.notify_updated(
            state.management.thread_id(),
            Some(turn_id.clone()),
            goal,
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn goal_turn_failed(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
    reason: &str,
) -> Result<bool, AppServerError> {
    goal_turn_limited(
        runtime,
        goal_id,
        turn_id,
        mini_agent_host::GoalStatus::Failed,
        reason,
    )
}

pub(super) fn goal_turn_limited(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
    status: mini_agent_host::GoalStatus,
    reason: &str,
) -> Result<bool, AppServerError> {
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let updated = state
        .goal_runtime_handle
        .limit_turn(goal_id, turn_id, status, reason)
        .map_err(workflow_error)?;
    if let Some(goal) = updated {
        state.goal_runtime_handle.notify_updated(
            state.management.thread_id(),
            Some(turn_id.clone()),
            goal,
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

pub(super) fn goal_turn_usage(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
    tokens: u64,
) -> Result<Option<crate::goal_service::GoalState>, AppServerError> {
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let updated = state
        .goal_runtime_handle
        .record_turn_usage(goal_id, turn_id, tokens)
        .map_err(workflow_error)?;
    if let Some(goal) = updated.as_ref() {
        state.goal_runtime_handle.notify_updated(
            state.management.thread_id(),
            Some(turn_id.clone()),
            goal.clone(),
        );
    }
    Ok(updated)
}

pub(super) fn prepare_goal_verification<M>(
    runtime: &mut Option<RuntimeActorState>,
    thread: &ThreadHandle<M>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<Option<crate::goal_runtime::GoalVerificationRequest>, AppServerError>
where
    M: Model + 'static,
{
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let checkpoint = thread
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    let thread_id = state.management.thread_id();
    state
        .goal_runtime_handle
        .prepare_verification(
            thread_id.clone(),
            goal_id,
            state.management.current_checkpoint_seq(),
            checkpoint.session.messages().to_vec(),
        )
        .map_err(workflow_error)
        .map(|request| {
            request.map(|mut request| {
                request.turn_id = turn_id.clone();
                request
            })
        })
}

pub(super) fn prepare_goal_verification_or_fail<M>(
    runtime: &mut Option<RuntimeActorState>,
    thread: &ThreadHandle<M>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<Option<crate::goal_runtime::GoalVerificationRequest>, AppServerError>
where
    M: Model + 'static,
{
    match prepare_goal_verification(runtime, thread, goal_id, turn_id) {
        Ok(request) => Ok(request),
        Err(error) => {
            let reason = format!("goal verifier preparation failed: {error}");
            goal_turn_failed(runtime, goal_id, turn_id, &reason)?;
            Ok(None)
        }
    }
}

pub(super) fn complete_goal_verification(
    runtime: &mut Option<RuntimeActorState>,
    runtime_revision: &AtomicU64,
    thread_id: ThreadId,
    goal_id: String,
    turn_id: mini_agent_protocol::TurnId,
    checkpoint_seq: u64,
    result: Result<(String, crate::goal_service::VerifierVerdict), String>,
) -> Result<(), AppServerError> {
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let current_checkpoint_seq = state.management.current_checkpoint_seq();
    let Some(goal) = state
        .goal_runtime_handle
        .complete_verification(
            &goal_id,
            &turn_id,
            checkpoint_seq,
            current_checkpoint_seq,
            result,
        )
        .map_err(workflow_error)?
    else {
        return Ok(());
    };
    let goal = if goal.status == mini_agent_host::GoalStatus::Running {
        match schedule_goal_turn(state, &goal) {
            Ok(()) => goal,
            Err(error) => state
                .goal_runtime_handle
                .fail_goal_with_reason(&error.to_string())
                .map_err(workflow_error)?,
        }
    } else {
        goal
    };
    state
        .goal_runtime_handle
        .notify_updated(thread_id, Some(turn_id), goal);
    let revision = state.advance_revision();
    runtime_revision.store(revision.value(), Ordering::SeqCst);
    Ok(())
}

pub(super) fn resume_goal<M>(
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
) -> Result<Option<crate::goal_runtime::GoalVerificationRequest>, AppServerError>
where
    M: Model + 'static,
{
    let goal = runtime
        .as_ref()
        .ok_or(AppServerError::RuntimeUnavailable)?
        .goal_runtime_handle
        .load_goal_state()
        .map_err(workflow_error)?;
    let Some(goal) = goal else {
        return Ok(None);
    };
    if goal.status != mini_agent_host::GoalStatus::Running {
        return Ok(None);
    }

    let thread_id = runtime
        .as_ref()
        .ok_or(AppServerError::RuntimeUnavailable)?
        .management
        .thread_id();
    if goal.active_turn_settled {
        let turn_id = goal
            .active_turn_id
            .as_deref()
            .map(TurnId::new)
            .ok_or_else(|| {
                AppServerError::Checkpoint("settled goal is missing its active turn".to_string())
            })?;
        let thread = threads
            .get(thread_id.as_str())
            .ok_or_else(|| AppServerError::ThreadNotFound(thread_id.clone()))?;
        return prepare_goal_verification_or_fail(runtime, thread, &goal.goal_id, &turn_id);
    }

    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    schedule_goal_turn(state, &goal)?;
    Ok(None)
}

fn append_context_and_persist<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    context: String,
) -> Result<(), AppServerError>
where
    M: Model + 'static,
{
    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(thread_id.clone()))?;
    let previous = thread
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    crate::worker::apply_thread_update(thread, crate::ThreadUpdate::AppendContext(context))?;
    let checkpoint = thread
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    if let Err(error) = state.management.record_context(&checkpoint) {
        if let Err(rollback) = thread.restore_checkpoint(previous) {
            return Err(AppServerError::Checkpoint(format!(
                "{error}; Thread rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn update_thread<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    update: crate::ThreadUpdate,
) -> Result<(), AppServerError>
where
    M: Model + 'static,
{
    if let crate::ThreadUpdate::AppendContext(context) = update {
        return append_context_and_persist(threads, state, context);
    }
    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or(AppServerError::ThreadNotFound(thread_id))?;
    crate::worker::apply_thread_update(thread, update)?;
    Ok(())
}

fn retry_mcp<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    approval: ApprovalController,
) -> Result<(crate::McpRetryResult, bool), AppServerError>
where
    M: Model + 'static,
{
    let servers = state.management.retry_mcp_servers();
    if servers.is_empty() {
        return Ok((
            crate::McpRetryResult {
                enabled_servers: Vec::new(),
                inactive_servers: Vec::new(),
                diagnostics: Vec::new(),
                tool_count: 0,
            },
            false,
        ));
    }
    let McpLoadResult {
        tools,
        loaded_servers,
        diagnostics,
    } = load_mcp(&servers, approval);
    let loaded_server_names = loaded_servers.iter().cloned().collect::<Vec<_>>();
    let inactive_servers = servers
        .iter()
        .filter(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        })
        .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
        .collect::<Vec<_>>();
    let enabled_servers = loaded_servers.iter().cloned().collect::<Vec<_>>();
    let tool_count = tools.len();
    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or(AppServerError::ThreadNotFound(thread_id))?;
    crate::worker::apply_thread_update(thread, crate::ThreadUpdate::ExtendTools(tools))?;
    state
        .management
        .record_mcp_retry(&loaded_server_names, &enabled_servers, tool_count);
    Ok((
        crate::McpRetryResult {
            enabled_servers,
            inactive_servers,
            diagnostics,
            tool_count,
        },
        true,
    ))
}

fn start_new_thread<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
) -> Result<(), AppServerError>
where
    M: Model + 'static,
{
    let old_thread_id = state.management.thread_id();
    if !threads.contains(old_thread_id.as_str()) {
        return Err(AppServerError::ThreadNotFound(old_thread_id));
    }
    let Some(session) = state.management.session_mut() else {
        return Err(AppServerError::Checkpoint(
            "session persistence is disabled".to_string(),
        ));
    };
    if let Err(error) = session.store.start_thread() {
        return Err(AppServerError::Checkpoint(error));
    }
    let new_thread_id = ThreadId::new(session.store.thread_id().to_string());
    threads.rename(&old_thread_id, new_thread_id, 1)
}

fn workflow_error(error: std::io::Error) -> AppServerError {
    AppServerError::Checkpoint(error.to_string())
}

fn mutate<T, F>(
    runtime: &mut Option<RuntimeActorState>,
    runtime_revision: &AtomicU64,
    operation: F,
) -> Result<T, AppServerError>
where
    F: FnOnce(&mut RuntimeActorState) -> Result<(T, bool), AppServerError>,
{
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let (value, changed) = operation(state)?;
    if changed {
        let revision = state.advance_revision();
        runtime_revision.store(revision.value(), Ordering::SeqCst);
    }
    Ok(value)
}

pub(super) fn advance_revision(
    runtime: &mut Option<RuntimeActorState>,
    runtime_revision: &AtomicU64,
) {
    if let Some(state) = runtime {
        let revision = state.advance_revision();
        runtime_revision.store(revision.value(), Ordering::SeqCst);
    }
}

pub(super) fn persist_turn(
    runtime: &mut Option<RuntimeActorState>,
    started_at_ms: u64,
    prompt: &str,
    result: &crate::RuntimeTurnResult,
    messages: &[Message],
    checkpoint: &ThreadCheckpoint,
) -> Result<(), AppServerError> {
    let Some(state) = runtime.as_mut() else {
        return Ok(());
    };
    state.management.record_turn(
        started_at_ms,
        prompt,
        result,
        messages,
        checkpoint.session.messages(),
    )
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
