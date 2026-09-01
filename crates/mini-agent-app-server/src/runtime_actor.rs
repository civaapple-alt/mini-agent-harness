use crate::AppServerError;
use crate::action::ActionFailure;
use crate::action::ActionReceipt;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::action::RuntimeRevision;
use crate::management::RuntimeActorState;
pub(super) use crate::runtime_command::{RuntimeCommand, RuntimeRequest};
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::McpLoadResult;
use mini_agent_capabilities::load_mcp;
use mini_agent_core::Thread;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::oneshot;

pub(super) fn handle_request<M>(
    request: RuntimeRequest,
    receipt: ActionReceipt,
    base_revision: RuntimeRevision,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: &AtomicU64,
) where
    M: Model,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    handle(
        request.command,
        receipt,
        runtime,
        threads,
        thread_ids,
        runtime_revision,
    );
}

pub(super) fn handle<M>(
    command: RuntimeCommand,
    receipt: ActionReceipt,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: &AtomicU64,
) where
    M: Model,
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
                start_new_thread(threads, thread_ids, state).map(|()| ((), true))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowState { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    Ok((
                        state.workflow.plan_active(),
                        state.workflow.load_goal_state().map_err(workflow_error)?,
                    ))
                }),
        ),
        RuntimeCommand::WorkflowSetPlan {
            active,
            prompt,
            reply,
        } => {
            let result = mutate(runtime, runtime_revision, |state| {
                if active {
                    state
                        .workflow
                        .init_plan_mode(prompt.as_deref())
                        .map(|_| ())
                        .map_err(workflow_error)
                } else {
                    state.workflow.disable_plan_mode().map_err(workflow_error)
                }
                .map(|()| ((), true))
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowInitGoal { objective, reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                state
                    .workflow
                    .init_goal(&objective)
                    .map(|goal| (goal, true))
                    .map_err(workflow_error)
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowLoadGoal { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.workflow.load_goal_state().map_err(workflow_error)),
        ),
        RuntimeCommand::WorkflowCriteria { reply } => respond(
            reply,
            receipt,
            runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    state
                        .workflow
                        .verification_criteria()
                        .map_err(workflow_error)
                }),
        ),
        RuntimeCommand::WorkflowRecordVerdict {
            checkpoint_seq,
            output,
            reply,
        } => {
            let result = mutate(runtime, runtime_revision, |state| {
                state
                    .workflow
                    .record_verifier_verdict(checkpoint_seq, &output)
                    .map(|()| ((), true))
                    .map_err(workflow_error)
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowAdvance { verdict, reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                state
                    .workflow
                    .advance_goal(verdict)
                    .map(|goal| (goal, true))
                    .map_err(workflow_error)
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowPause { reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                state
                    .workflow
                    .pause_goal()
                    .map(|()| ((), true))
                    .map_err(workflow_error)
            });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowFail { reply } => {
            let result = mutate(runtime, runtime_revision, |state| {
                state
                    .workflow
                    .fail_goal()
                    .map(|goal| (goal, true))
                    .map_err(workflow_error)
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
        RuntimeCommand::WorkflowState { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowSetPlan { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowInitGoal { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowLoadGoal { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowCriteria { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowRecordVerdict { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowAdvance { reply, .. } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowPause { reply } => respond(reply, receipt, Err(error)),
        RuntimeCommand::WorkflowFail { reply } => respond(reply, receipt, Err(error)),
    }
}

pub(super) fn handle_running<M>(
    request: RuntimeRequest,
    receipt: ActionReceipt,
    base_revision: RuntimeRevision,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: &AtomicU64,
) where
    M: Model,
{
    if let Err(error) = check_revision(&request, runtime, base_revision) {
        reject_runtime(request.command, receipt, error);
        return;
    }
    let command = request.command;
    if command.is_mutation() {
        reject_runtime(command, receipt, AppServerError::Busy);
    } else {
        handle(
            command,
            receipt,
            runtime,
            threads,
            thread_ids,
            runtime_revision,
        );
    }
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

fn update_world<M>(
    threads: &mut HashMap<String, Thread<M>>,
    state: &mut RuntimeActorState,
    updated: mini_agent_host::WorldState,
) -> Result<bool, AppServerError>
where
    M: Model,
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

fn append_context_and_persist<M>(
    threads: &mut HashMap<String, Thread<M>>,
    state: &mut RuntimeActorState,
    context: String,
) -> Result<(), AppServerError>
where
    M: Model,
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
    threads: &mut HashMap<String, Thread<M>>,
    state: &mut RuntimeActorState,
    update: crate::ThreadUpdate,
) -> Result<(), AppServerError>
where
    M: Model,
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
    threads: &mut HashMap<String, Thread<M>>,
    state: &mut RuntimeActorState,
    approval: ApprovalController,
) -> Result<(crate::McpRetryResult, bool), AppServerError>
where
    M: Model,
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
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    state: &mut RuntimeActorState,
) -> Result<(), AppServerError>
where
    M: Model,
{
    let old_thread_id = state.management.thread_id();
    let old_key = old_thread_id.as_str().to_string();
    let mut thread = threads
        .remove(&old_key)
        .ok_or_else(|| AppServerError::ThreadNotFound(old_thread_id.clone()))?;
    let Some(session) = state.management.session_mut() else {
        threads.insert(old_key, thread);
        return Err(AppServerError::Checkpoint(
            "session persistence is disabled".to_string(),
        ));
    };
    if let Err(error) = session.store.start_thread() {
        threads.insert(old_key, thread);
        return Err(AppServerError::Checkpoint(error));
    }
    let new_thread_id = ThreadId::new(session.store.thread_id().to_string());
    thread.set_id(new_thread_id.clone());
    thread.set_next_turn_number(1);
    threads.insert(new_thread_id.as_str().to_string(), thread);
    if let Some(known) = thread_ids
        .lock()
        .unwrap()
        .iter_mut()
        .find(|known| **known == old_thread_id)
    {
        *known = new_thread_id;
    }
    Ok(())
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
