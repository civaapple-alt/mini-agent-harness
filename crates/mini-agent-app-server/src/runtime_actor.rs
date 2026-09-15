use crate::AppServerError;
use crate::action::{ActionReceipt, RuntimeRevision, respond};
use crate::management::{RuntimeActorState, SettingsRuntimeEvent};
use crate::notification::WorkflowRuntimeEvent;
pub(super) use crate::runtime_command::{RuntimeCommand, RuntimeRequest};
use crate::status;
use crate::thread_manager::ThreadManager;
use mini_agent_app_server_protocol::ContinuationMode;
use mini_agent_app_server_protocol::{RuntimePhase, WorkflowLifecycleNotification};
use mini_agent_capabilities::{ApprovalController, McpLoadResult, SecurityPolicy, load_mcp};
use mini_agent_core::Thread;
use mini_agent_protocol::{Message, Model, ThreadId, TurnId, TurnInput, TurnInputMode, TurnStart};
use std::sync::atomic::{AtomicU64, Ordering};
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

pub(super) async fn handle_session_fork_request<M>(
    request: RuntimeRequest,
    receipt: ActionReceipt,
    base_revision: RuntimeRevision,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
) where
    M: Model + Send + 'static,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    let RuntimeCommand::PrepareSessionFork {
        source_thread_id,
        new_thread_id,
        context_policy,
        reply,
    } = request.command
    else {
        unreachable!("session fork handler received another runtime command");
    };

    let result = prepare_session_fork(
        source_thread_id,
        new_thread_id,
        context_policy,
        runtime,
        threads,
    )
    .await;
    respond(reply, receipt, result);
}

async fn prepare_session_fork<M>(
    source_thread_id: ThreadId,
    new_thread_id: ThreadId,
    context_policy: mini_agent_app_server_protocol::ForkContextPolicy,
    runtime: &Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
) -> Result<mini_agent_app_server_protocol::SessionForkResult, AppServerError>
where
    M: Model + Send + 'static,
{
    let state = runtime.as_ref().ok_or(AppServerError::RuntimeUnavailable)?;
    if state.management.thread_id() != source_thread_id {
        return Err(AppServerError::ThreadNotFound(source_thread_id));
    }
    let parent = state
        .management
        .session_info()
        .ok_or_else(|| AppServerError::Checkpoint("session persistence is disabled".to_string()))?;
    let parent_checkpoint_seq = state.management.current_checkpoint_seq();
    let workspace = state.management.world().workspace().to_path_buf();
    let core_policy = match context_policy {
        mini_agent_app_server_protocol::ForkContextPolicy::Exact => {
            mini_agent_core::ForkContextPolicy::Exact
        }
        mini_agent_app_server_protocol::ForkContextPolicy::Compact => {
            mini_agent_core::ForkContextPolicy::Compact
        }
    };
    let thread = threads
        .get_mut(source_thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(source_thread_id.clone()))?;
    if thread.status() == mini_agent_protocol::ThreadStatus::Running {
        return Err(AppServerError::Busy);
    }
    let prepared = thread
        .harness_mut()
        .prepare_fork_checkpoint(core_policy)
        .await
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    let method = match prepared.method {
        mini_agent_core::ForkCompactionMethod::Exact => {
            mini_agent_app_server_protocol::ForkCompactionMethod::Exact
        }
        mini_agent_core::ForkCompactionMethod::ModelSummary => {
            mini_agent_app_server_protocol::ForkCompactionMethod::ModelSummary
        }
        mini_agent_core::ForkCompactionMethod::Mechanical => {
            mini_agent_app_server_protocol::ForkCompactionMethod::Mechanical
        }
    };
    let child = mini_agent_capabilities::SessionStore::fork_from_checkpoint(
        &workspace,
        &parent.session_id,
        parent_checkpoint_seq,
        new_thread_id.as_str(),
        prepared.session.messages(),
    )
    .map_err(AppServerError::Checkpoint)?;
    Ok(mini_agent_app_server_protocol::SessionForkResult {
        session_id: child.session_id,
        thread_id: child.thread_id,
        path: child.path,
        parent_session_id: child.parent_session_id,
        parent_checkpoint_seq: child.parent_checkpoint_seq,
        session_bytes: child.session_bytes,
        context_before_bytes: prepared.context_before_bytes,
        context_after_bytes: prepared.context_after_bytes,
        compacted: method != mini_agent_app_server_protocol::ForkCompactionMethod::Exact,
        method,
    })
}

pub(super) fn cleanup_plan_scratch(
    runtime: &mut Option<RuntimeActorState>,
) -> Result<(), AppServerError> {
    runtime
        .as_ref()
        .filter(|state| state.goal_runtime_handle.plan_active())
        .map(|state| state.goal_runtime_handle.cleanup_plan_scratch())
        .transpose()
        .map(|_| ())
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))
}

fn workflow_payload(
    state: &RuntimeActorState,
    turn_id: Option<TurnId>,
    operation_id: String,
    checkpoint_seq: Option<u64>,
    error: Option<&str>,
    goal: Option<&crate::goal_service::GoalState>,
    plan_active: Option<bool>,
) -> WorkflowLifecycleNotification {
    WorkflowLifecycleNotification {
        thread_id: state.management.thread_id(),
        turn_id,
        operation_id,
        checkpoint_seq,
        state_revision: state.revision().value(),
        timestamp_ms: status::timestamp_ms(),
        error: error.map(|value| value.chars().take(1024).collect()),
        goal_id: goal.map(|value| value.goal_id.clone()),
        milestone: goal.map(|value| value.current_milestone),
        total_milestones: goal.map(|value| value.total_milestones),
        plan_active,
    }
}

pub(super) fn workflow_payload_for_worker(
    runtime: &Option<RuntimeActorState>,
    turn_id: Option<TurnId>,
    operation_id: String,
    checkpoint_seq: Option<u64>,
    error: Option<&str>,
    goal: Option<&crate::goal_service::GoalState>,
    plan_active: Option<bool>,
) -> Option<WorkflowLifecycleNotification> {
    runtime.as_ref().map(|state| {
        workflow_payload(
            state,
            turn_id,
            operation_id,
            checkpoint_seq,
            error,
            goal,
            plan_active,
        )
    })
}

fn send_workflow(state: &RuntimeActorState, event: WorkflowRuntimeEvent) {
    let _ = state
        .notifications
        .send(crate::RuntimeNotification::Workflow(event));
}

pub(super) fn notify_checkpoint_committed(
    runtime: &Option<RuntimeActorState>,
    turn_id: TurnId,
    checkpoint_seq: u64,
) {
    let Some(state) = runtime.as_ref() else {
        return;
    };
    send_workflow(
        state,
        WorkflowRuntimeEvent::CheckpointCommitted(workflow_payload(
            state,
            Some(turn_id.clone()),
            status::operation("turn", turn_id.as_str()),
            Some(checkpoint_seq),
            None,
            None,
            None,
        )),
    );
    status::publish_at(
        &state.status,
        &state.notifications,
        state.management.thread_id(),
        RuntimePhase::Persisting,
        Some(turn_id.clone()),
        Some(status::operation("turn", turn_id.as_str())),
        Some(checkpoint_seq),
        state.revision().value(),
        None,
    );
}

pub(super) fn notify_plan_updated(runtime: &Option<RuntimeActorState>, active: bool) {
    let Some(state) = runtime.as_ref() else {
        return;
    };
    let operation_id = status::operation("plan", state.revision().value());
    send_workflow(
        state,
        WorkflowRuntimeEvent::PlanUpdated(workflow_payload(
            state,
            None,
            operation_id,
            Some(state.management.current_checkpoint_seq()),
            None,
            None,
            Some(active),
        )),
    );
}

pub(super) fn notify_plan_cleanup(
    runtime: &Option<RuntimeActorState>,
    turn_id: Option<TurnId>,
    started: bool,
    error: Option<&str>,
) {
    let Some(state) = runtime.as_ref() else {
        return;
    };
    notify_plan_cleanup_state(state, turn_id, started, error);
}

pub(super) fn notify_plan_cleanup_state(
    state: &RuntimeActorState,
    turn_id: Option<TurnId>,
    started: bool,
    error: Option<&str>,
) {
    let operation_id = status::operation(
        "plan-cleanup",
        turn_id.as_ref().map_or_else(|| "settings", TurnId::as_str),
    );
    let payload = workflow_payload(
        state,
        turn_id,
        operation_id,
        Some(state.management.current_checkpoint_seq()),
        error,
        None,
        Some(started),
    );
    let status_turn_id = payload.turn_id.clone();
    let status_operation_id = payload.operation_id.clone();
    let status_checkpoint_seq = payload.checkpoint_seq;
    send_workflow(
        state,
        match (started, error.is_some()) {
            (true, _) => WorkflowRuntimeEvent::PlanCleanupStarted(payload),
            (false, true) => WorkflowRuntimeEvent::PlanCleanupFailed(payload),
            (false, false) => WorkflowRuntimeEvent::PlanCleanupCompleted(payload),
        },
    );
    if !started && let Some(error) = error {
        status::publish_at(
            &state.status,
            &state.notifications,
            state.management.thread_id(),
            RuntimePhase::Failed,
            status_turn_id,
            Some(status_operation_id),
            status_checkpoint_seq,
            state.revision().value(),
            Some(error),
        );
    }
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
        RuntimeCommand::PrepareSessionFork { .. } => {
            unreachable!("session fork must use the async runtime handler")
        }
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
                let refreshed = mini_agent_host::WorldState::detect_with_roots(
                    current.workspace(),
                    current.extra_roots().to_vec(),
                    current.access(),
                    current.policy(),
                    current.sandbox(),
                );
                update_world(threads, state, refreshed).map(|changed| (changed, changed))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::SetExecution {
            access,
            policy,
            reply,
        } => {
            let result = mutate(runtime, runtime_revision, |state| {
                let current = state.management.world();
                state
                    .approval
                    .set_policy(SecurityPolicy::for_preset(access));
                state.approval.set_approval_policy(policy);
                state.approval.set_access_scope(access.name());
                update_world(
                    threads,
                    state,
                    current.with_execution(access, policy, current.sandbox()),
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
        RuntimeCommand::ThreadSettingsUpdate {
            active,
            builtin_tools,
            continuation_mode,
            reply,
        } => {
            let result = mutate::<(crate::management::ThreadSettingsRuntimeSnapshot, bool), _>(
                runtime,
                runtime_revision,
                |state| {
                    let previous_active = state.goal_runtime_handle.plan_active();
                    let previous_tools = state.builtin_tools.names().to_vec();
                    let previous_continuation = state.continuation_mode;
                    set_thread_settings(threads, state, active, builtin_tools, continuation_mode)
                        .map(|settings| {
                            let changed = previous_active != active
                                || previous_tools != settings.builtin_tools
                                || previous_continuation != settings.continuation_mode;
                            ((settings, changed), changed)
                        })
                },
            );
            let changed = result.as_ref().is_ok_and(|(_, changed)| *changed);
            if changed && let Some(state) = runtime.as_ref() {
                let event = SettingsRuntimeEvent {
                    thread_id: state.management.thread_id(),
                    active,
                    builtin_tools: state.builtin_tools.names().to_vec(),
                    continuation_mode: state.continuation_mode,
                    state_revision: state.revision().value(),
                };
                let _ = state.settings_notifications.send(event.clone());
                let _ = state
                    .notifications
                    .send(crate::RuntimeNotification::Settings(event));
                notify_plan_updated(runtime, active);
            }
            respond(reply, receipt, result.map(|(settings, _)| settings));
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
                if changed && goal.status == mini_agent_host::GoalStatus::Running {
                    schedule_goal_turn(state, &goal)?;
                }
                Ok(((goal, changed), changed))
            });
            if let Ok((goal, true)) = &result
                && let Some(state) = runtime.as_ref()
            {
                state.goal_runtime_handle.notify_updated(
                    state.management.thread_id(),
                    None,
                    goal.clone(),
                    state.revision().value(),
                );
            }
            respond(reply, receipt, result.map(|(goal, _)| goal));
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
                Ok((cleared, cleared))
            });
            if let Ok(true) = &result
                && let Some(state) = runtime.as_ref()
            {
                state
                    .goal_runtime_handle
                    .notify_cleared(state.management.thread_id(), state.revision().value());
            }
            respond(reply, receipt, result);
        }
    }
}

fn reject_runtime(command: RuntimeCommand, receipt: ActionReceipt, error: AppServerError) {
    match command {
        RuntimeCommand::SessionInfo { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::PrepareSessionFork { reply, .. } => respond(reply, receipt, Err(error)),
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
    stopping: bool,
) where
    M: Model + 'static,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    let command = request.command;
    if (stopping && command.is_mutation())
        || (command.is_mutation() && !is_safe_goal_mutation_while_running(&command))
    {
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

pub(super) fn set_thread_settings<M>(
    threads: &mut ThreadManager<M>,
    state: &mut RuntimeActorState,
    active: bool,
    builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
    continuation_mode: Option<ContinuationMode>,
) -> Result<crate::management::ThreadSettingsRuntimeSnapshot, AppServerError>
where
    M: Model + 'static,
{
    if continuation_mode.is_some()
        && state
            .goal_runtime_handle
            .load_goal_state()
            .map_err(workflow_error)?
            .is_some_and(|goal| goal.status == mini_agent_host::GoalStatus::Running)
    {
        return Err(AppServerError::GoalOwnsContinuationMode);
    }

    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(thread_id.clone()))?;
    let was_plan_active = state.goal_runtime_handle.plan_active();
    if let Some(mode) = continuation_mode {
        state.management.persist_continuation_mode(mode)?;
        let config = match mode {
            ContinuationMode::Manual => state.management.base_harness_config.clone(),
            ContinuationMode::Continuous => state
                .management
                .base_harness_config
                .clone()
                .with_copilot_loop(),
        };
        thread.harness_mut().replace_config(config);
        state.continuation_mode = mode;
    }
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
        if was_plan_active {
            notify_plan_cleanup_state(state, None, true, None);
        }
        let cleanup_result = state.goal_runtime_handle.disable_plan_mode();
        if was_plan_active {
            match cleanup_result.as_ref() {
                Ok(()) => notify_plan_cleanup_state(state, None, false, None),
                Err(error) => {
                    let error_text = error.to_string();
                    notify_plan_cleanup_state(state, None, false, Some(&error_text));
                }
            }
        }
        cleanup_result.map_err(workflow_error)?;
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
    Ok(crate::management::ThreadSettingsRuntimeSnapshot {
        builtin_tools: state.builtin_tools.names().to_vec(),
        continuation_mode: state.continuation_mode,
    })
}

pub(super) fn restore_thread_continuation<M>(
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut ThreadManager<M>,
) -> Result<(), AppServerError>
where
    M: Model + 'static,
{
    let Some(state) = runtime.as_ref() else {
        return Ok(());
    };
    if state.continuation_mode != ContinuationMode::Continuous
        || state
            .goal_runtime_handle
            .load_goal_state()
            .map_err(workflow_error)?
            .is_some_and(|goal| goal.status == mini_agent_host::GoalStatus::Running)
    {
        return Ok(());
    }
    let active = state.goal_runtime_handle.plan_active();
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    set_thread_settings(
        threads,
        state,
        active,
        None,
        Some(ContinuationMode::Continuous),
    )
    .map(|_| ())
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
    let operation_id = status::operation("goal-continuation", &goal.goal_id);
    let payload = workflow_payload(
        state,
        None,
        operation_id.clone(),
        Some(state.management.current_checkpoint_seq()),
        None,
        Some(goal),
        None,
    );
    send_workflow(state, WorkflowRuntimeEvent::GoalContinuationQueued(payload));
    status::publish_at(
        &state.status,
        &state.notifications,
        state.management.thread_id(),
        RuntimePhase::GoalContinuationQueued,
        None,
        Some(operation_id),
        Some(state.management.current_checkpoint_seq()),
        state.revision().value(),
        None,
    );
    Ok(())
}

pub(super) fn goal_turn_started(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<bool, AppServerError> {
    update_goal_turn(runtime, turn_id, |handle| {
        handle.mark_turn_started(goal_id, turn_id)
    })
}

pub(super) fn goal_turn_settled(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
) -> Result<bool, AppServerError> {
    update_goal_turn(runtime, turn_id, |handle| {
        handle.mark_turn_settled(goal_id, turn_id)
    })
}

pub(super) fn goal_turn_limited(
    runtime: &mut Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: &mini_agent_protocol::TurnId,
    status: mini_agent_host::GoalStatus,
    reason: &str,
) -> Result<bool, AppServerError> {
    update_goal_turn(runtime, turn_id, |handle| {
        handle.limit_turn(goal_id, turn_id, status, reason)
    })
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
    notify_goal_update(state, turn_id, updated.clone());
    Ok(updated)
}

fn update_goal_turn<F>(
    runtime: &mut Option<RuntimeActorState>,
    turn_id: &mini_agent_protocol::TurnId,
    update: F,
) -> Result<bool, AppServerError>
where
    F: FnOnce(
        &mut crate::goal_runtime::GoalRuntimeHandle,
    ) -> std::io::Result<Option<crate::goal_service::GoalState>>,
{
    let state = runtime.as_mut().ok_or(AppServerError::RuntimeUnavailable)?;
    let updated = update(&mut state.goal_runtime_handle).map_err(workflow_error)?;
    Ok(notify_goal_update(state, turn_id, updated))
}

fn notify_goal_update(
    state: &RuntimeActorState,
    turn_id: &mini_agent_protocol::TurnId,
    goal: Option<crate::goal_service::GoalState>,
) -> bool {
    let Some(goal) = goal else {
        return false;
    };
    state.goal_runtime_handle.notify_updated(
        state.management.thread_id(),
        Some(turn_id.clone()),
        goal,
        state.revision().next().value(),
    );
    true
}

pub(super) fn prepare_goal_verification<M>(
    runtime: &mut Option<RuntimeActorState>,
    thread: &Thread<M>,
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
    let request = state
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
        })?;
    if let Some(request) = request.as_ref()
        && let Some(goal) = state
            .goal_runtime_handle
            .load_goal_state()
            .map_err(workflow_error)?
    {
        let operation_id = status::operation(
            "goal-verification",
            format!("{}:{}", request.goal_id, request.checkpoint_seq),
        );
        let payload = workflow_payload(
            state,
            Some(request.turn_id.clone()),
            operation_id.clone(),
            Some(request.checkpoint_seq),
            None,
            Some(&goal),
            None,
        );
        send_workflow(
            state,
            WorkflowRuntimeEvent::GoalVerificationStarted(payload),
        );
        status::publish_at(
            &state.status,
            &state.notifications,
            state.management.thread_id(),
            RuntimePhase::GoalVerification,
            Some(request.turn_id.clone()),
            Some(operation_id),
            Some(request.checkpoint_seq),
            state.revision().value(),
            None,
        );
        notify_goal_update(state, &request.turn_id, Some(goal));
    }
    Ok(request)
}

pub(super) fn prepare_goal_verification_or_fail<M>(
    runtime: &mut Option<RuntimeActorState>,
    thread: &Thread<M>,
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
            notify_goal_verification_failed(runtime, goal_id, turn_id.clone(), &reason);
            goal_turn_limited(
                runtime,
                goal_id,
                turn_id,
                mini_agent_host::GoalStatus::Failed,
                &reason,
            )?;
            Ok(None)
        }
    }
}

pub(super) fn notify_goal_verification_failed(
    runtime: &Option<RuntimeActorState>,
    goal_id: &str,
    turn_id: TurnId,
    error: &str,
) {
    let Some(state) = runtime.as_ref() else {
        return;
    };
    let checkpoint_seq = state.management.current_checkpoint_seq();
    let operation_id =
        status::operation("goal-verification", format!("{goal_id}:{checkpoint_seq}"));
    let mut payload = workflow_payload(
        state,
        Some(turn_id.clone()),
        operation_id.clone(),
        Some(checkpoint_seq),
        Some(error),
        None,
        None,
    );
    payload.goal_id = Some(goal_id.to_string());
    send_workflow(state, WorkflowRuntimeEvent::GoalVerificationFailed(payload));
    status::publish_at(
        &state.status,
        &state.notifications,
        state.management.thread_id(),
        RuntimePhase::Failed,
        Some(turn_id),
        Some(operation_id),
        Some(checkpoint_seq),
        state.revision().value(),
        Some(error),
    );
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
    let verification_error = result.as_ref().err().cloned();
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
    let operation_id = status::operation(
        "goal-verification",
        format!("{}:{}", goal_id, checkpoint_seq),
    );
    let payload = workflow_payload(
        state,
        Some(turn_id.clone()),
        operation_id,
        Some(checkpoint_seq),
        verification_error.as_deref(),
        Some(&goal),
        None,
    );
    send_workflow(
        state,
        if verification_error.is_some() {
            WorkflowRuntimeEvent::GoalVerificationFailed(payload)
        } else {
            WorkflowRuntimeEvent::GoalVerificationCompleted(payload)
        },
    );
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
    state.goal_runtime_handle.notify_updated(
        thread_id.clone(),
        Some(turn_id.clone()),
        goal.clone(),
        state.revision().next().value(),
    );
    let revision = state.advance_revision();
    runtime_revision.store(revision.value(), Ordering::SeqCst);
    if goal.status != mini_agent_host::GoalStatus::Running {
        status::publish_at(
            &state.status,
            &state.notifications,
            thread_id,
            if goal.status == mini_agent_host::GoalStatus::Converged {
                RuntimePhase::Completed
            } else {
                RuntimePhase::Failed
            },
            None,
            None,
            Some(current_checkpoint_seq),
            revision.value(),
            goal.last_error.as_deref(),
        );
    }
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
    thread: &Thread<impl Model>,
    started_at_ms: u64,
    prompt: &str,
    result: &crate::RuntimeTurnResult,
    messages: &[Message],
    tool_arguments: &[(String, serde_json::Value)],
) -> Result<(), AppServerError> {
    let checkpoint = thread
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    let Some(state) = runtime.as_mut() else {
        return Ok(());
    };
    state.management.record_turn(
        started_at_ms,
        prompt,
        result,
        messages,
        tool_arguments,
        checkpoint.session.messages(),
    )
}
