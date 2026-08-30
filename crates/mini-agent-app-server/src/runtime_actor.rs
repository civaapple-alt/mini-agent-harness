use crate::AppServerError;
use crate::action::ActionReceipt;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::management::RuntimeActorState;
pub(super) use crate::runtime_command::RuntimeCommand;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::McpLoadResult;
use mini_agent_capabilities::load_mcp;
use mini_agent_core::Thread;
use mini_agent_host::tool_outcome::classify_tools;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::oneshot;

pub(super) fn handle<M>(
    command: RuntimeCommand,
    receipt: ActionReceipt,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
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
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    let current = state.management.world();
                    let refreshed = mini_agent_host::WorldState::detect(
                        current.workspace(),
                        current.approval(),
                        current.copilot(),
                        current.sandbox(),
                    );
                    update_world(threads, state, refreshed)
                });
            respond(reply, receipt, result);
        }
        RuntimeCommand::SetExecution {
            approval,
            copilot,
            reply,
        } => {
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    let current = state.management.world();
                    update_world(
                        threads,
                        state,
                        current.with_execution(approval, copilot, current.sandbox()),
                    )
                });
            respond(reply, receipt, result);
        }
        RuntimeCommand::UpdateWorld { updated, reply } => {
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| update_world(threads, state, updated));
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
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| retry_mcp(threads, state, approval));
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
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| start_new_thread(threads, thread_ids, state));
            respond(reply, receipt, result);
        }
        RuntimeCommand::RecordContext { checkpoint, reply } => {
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.management.record_context(&checkpoint));
            respond(reply, receipt, result);
        }
        RuntimeCommand::RecordTurn {
            started_at_ms,
            prompt,
            result,
            messages,
            checkpoint,
            reply,
        } => {
            let result = runtime
                .as_mut()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    state.management.record_turn(
                        started_at_ms,
                        &prompt,
                        &result,
                        &messages,
                        &checkpoint,
                    )
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
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    if active {
                        state
                            .workflow
                            .init_plan_mode(prompt.as_deref())
                            .map(|_| ())
                            .map_err(workflow_error)
                    } else {
                        state.workflow.disable_plan_mode().map_err(workflow_error)
                    }
                });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowInitGoal { objective, reply } => {
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.workflow.init_goal(&objective).map_err(workflow_error));
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
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| {
                    state
                        .workflow
                        .record_verifier_verdict(checkpoint_seq, &output)
                        .map_err(workflow_error)
                });
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowAdvance { verdict, reply } => {
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.workflow.advance_goal(verdict).map_err(workflow_error));
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowPause { reply } => {
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.workflow.pause_goal().map_err(workflow_error));
            respond(reply, receipt, result);
        }
        RuntimeCommand::WorkflowFail { reply } => {
            let result = runtime
                .as_ref()
                .ok_or(AppServerError::RuntimeUnavailable)
                .and_then(|state| state.workflow.fail_goal().map_err(workflow_error));
            respond(reply, receipt, result);
        }
    }
}

pub(super) fn reject_running(command: RuntimeCommand, receipt: ActionReceipt) {
    match command {
        RuntimeCommand::SessionInfo { reply } => respond(reply, receipt, Err(AppServerError::Busy)),
        RuntimeCommand::CheckpointSeq { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::ThreadId { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::World { reply } => respond(reply, receipt, Err(AppServerError::Busy)),
        RuntimeCommand::RefreshWorld { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::SetExecution { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::UpdateWorld { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::McpStatus { reply } => respond(reply, receipt, Err(AppServerError::Busy)),
        RuntimeCommand::RetryMcp { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::ReadCheckpoint { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::StartNewThread { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::RecordContext { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::RecordTurn { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowState { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowSetPlan { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowInitGoal { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowLoadGoal { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowCriteria { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowRecordVerdict { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowAdvance { reply, .. } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowPause { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
        RuntimeCommand::WorkflowFail { reply } => {
            respond(reply, receipt, Err(AppServerError::Busy))
        }
    }
}

pub(super) fn handle_running<M>(
    command: RuntimeCommand,
    receipt: ActionReceipt,
    runtime: &mut Option<RuntimeActorState>,
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
) where
    M: Model,
{
    match command {
        RuntimeCommand::SessionInfo { .. }
        | RuntimeCommand::CheckpointSeq { .. }
        | RuntimeCommand::ThreadId { .. }
        | RuntimeCommand::World { .. }
        | RuntimeCommand::McpStatus { .. }
        | RuntimeCommand::WorkflowState { .. }
        | RuntimeCommand::WorkflowSetPlan { .. }
        | RuntimeCommand::WorkflowInitGoal { .. }
        | RuntimeCommand::WorkflowLoadGoal { .. }
        | RuntimeCommand::WorkflowCriteria { .. }
        | RuntimeCommand::WorkflowRecordVerdict { .. }
        | RuntimeCommand::WorkflowAdvance { .. }
        | RuntimeCommand::WorkflowPause { .. }
        | RuntimeCommand::WorkflowFail { .. } => {
            handle(command, receipt, runtime, threads, thread_ids)
        }
        command => reject_running(command, receipt),
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
    let thread_id = state.management.thread_id();
    let thread = threads
        .get_mut(thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(thread_id.clone()))?;
    crate::worker::apply_thread_update(thread, crate::ThreadUpdate::AppendContext(context))?;
    let checkpoint = thread
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    state.management.record_context(&checkpoint)?;
    state.management.set_world(updated);
    Ok(true)
}

fn retry_mcp<M>(
    threads: &mut HashMap<String, Thread<M>>,
    state: &mut RuntimeActorState,
    approval: ApprovalController,
) -> Result<crate::McpRetryResult, AppServerError>
where
    M: Model,
{
    let servers = state.management.retry_mcp_servers();
    if servers.is_empty() {
        return Ok(crate::McpRetryResult {
            enabled_servers: Vec::new(),
            inactive_servers: Vec::new(),
            diagnostics: Vec::new(),
            tool_count: 0,
        });
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
    crate::worker::apply_thread_update(
        thread,
        crate::ThreadUpdate::ExtendTools(classify_tools(tools)),
    )?;
    state
        .management
        .record_mcp_retry(&loaded_server_names, &enabled_servers, tool_count);
    Ok(crate::McpRetryResult {
        enabled_servers,
        inactive_servers,
        diagnostics,
        tool_count,
    })
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

fn respond<T>(
    reply: oneshot::Sender<ActionResult<T>>,
    receipt: ActionReceipt,
    result: Result<T, AppServerError>,
) {
    let _ = reply.send(result.map(|value| ActionResponse { value, receipt }));
}
