use mini_agent_protocol::Event;
use mini_agent_protocol::Observer;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::ToolExecutionContext;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::TurnId;

use crate::RunControl;
use crate::SessionState;
use crate::ToolRouter;

pub(super) struct ToolBatchOptions<'context, 'journal, 'sink> {
    pub max_output_bytes: usize,
    pub context: Option<&'context ToolExecutionContext>,
    pub turn_id: Option<&'context TurnId>,
    pub step: usize,
    pub journal: &'journal mut Option<&'sink mut dyn crate::ExecutionJournalSink>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ToolBatchRecoveryError {
    NeedsReconciliation(String),
    Journal(String),
}

pub(super) fn validate_tool_batch_recovery(
    tools: &ToolRouter,
    batch: &crate::ExecutionToolBatch,
) -> Result<(), String> {
    for call in &batch.calls {
        if call.started && call.outcome.is_none() {
            let request = ToolExecutionRequest::from(call.call.clone());
            if tools.recovery_replay_safety(&request) != mini_agent_protocol::ToolReplaySafety::Safe
            {
                return Err(format!(
                    "tool call {} ({}) started without a durable result",
                    call.call.id, call.call.name
                ));
            }
        }
    }
    Ok(())
}

/// Executes one complete tool batch and records its bounded outputs.
pub(super) fn execute_tool_batch<O: Observer>(
    tools: &ToolRouter,
    calls: Vec<ToolCall>,
    session: &mut SessionState,
    observer: &mut O,
    control: &RunControl,
    options: ToolBatchOptions<'_, '_, '_>,
) -> Result<Vec<(String, serde_json::Value, String)>, String> {
    let ToolBatchOptions {
        max_output_bytes,
        context,
        turn_id,
        step,
        journal,
    } = options;
    let mut executed = Vec::with_capacity(calls.len());
    let mut context_messages = Vec::new();
    let mut context_injections = Vec::new();
    let mut replan_after_injection = false;
    for call in calls {
        if control.is_cancel_requested() {
            record_tool_result(
                &call,
                ToolExecutionOutcome::cancelled(
                    "Turn cancelled before this tool call started; the call was not executed.",
                ),
                max_output_bytes,
                session,
                observer,
                turn_id,
                step,
                journal,
                &mut executed,
            )?;
            continue;
        }

        if let Some(turn_id) = turn_id {
            crate::execution::append_if_present(
                journal,
                crate::ExecutionJournalEntry::ToolCallStarted {
                    turn_id: turn_id.clone(),
                    step,
                    call_id: call.id.clone(),
                },
            )?;
        }
        observer.observe(&Event::ToolStarted { call: call.clone() });
        let request = ToolExecutionRequest::from(call.clone());
        let request = if let Some(context) = context {
            request.with_context(context.clone())
        } else {
            request
        }
        .with_known_context_injections(session.context_injections())
        .with_cancellation(control.cancellation_token());
        let outcome = if control.is_cancel_requested() {
            ToolExecutionOutcome::cancelled(
                "Turn cancelled before this tool call executed; the call was not run.",
            )
        } else if replan_after_injection {
            ToolExecutionOutcome::deferred(
                "Host added workspace instructions. Review them, then retry the remaining operation.",
            )
        } else {
            tools.execute_outcome(&request)
        };
        replan_after_injection |= !outcome.context_messages.is_empty();
        context_messages.extend(outcome.context_messages.iter().cloned());
        context_injections.extend(outcome.context_injections.iter().cloned());
        record_tool_result(
            &call,
            outcome,
            max_output_bytes,
            session,
            observer,
            turn_id,
            step,
            journal,
            &mut executed,
        )?;
    }
    let candidate_injections = std::mem::take(&mut context_injections);
    for (index, text) in context_messages.into_iter().enumerate() {
        if let Some(record) = candidate_injections.get(index).cloned() {
            if let Some(record) = session.append_context_injection(text, record) {
                context_injections.push(record);
            }
        } else {
            session.push(mini_agent_protocol::Message::Context { text });
        }
    }
    if !context_injections.is_empty() {
        observer.observe(&Event::ContextInjected {
            records: context_injections,
        });
    }
    Ok(executed)
}

/// Restores a journaled batch from durable outcomes and safely replayable calls.
pub(super) fn recover_tool_batch<O: Observer>(
    tools: &ToolRouter,
    batch: crate::ExecutionToolBatch,
    session: &mut SessionState,
    observer: &mut O,
    control: &RunControl,
    options: ToolBatchOptions<'_, '_, '_>,
) -> Result<Vec<(String, serde_json::Value, String)>, ToolBatchRecoveryError> {
    let ToolBatchOptions {
        max_output_bytes,
        context,
        journal,
        ..
    } = options;
    if let Err(reason) = validate_tool_batch_recovery(tools, &batch) {
        return Err(ToolBatchRecoveryError::NeedsReconciliation(reason));
    }

    let intent = batch.intent;
    let mut executed = Vec::with_capacity(batch.calls.len());
    let mut context_messages = Vec::new();
    let mut context_injections = Vec::new();
    for call in batch.calls {
        let (outcome, started) = if let Some(outcome) = call.outcome {
            (outcome, call.started)
        } else {
            crate::execution::append_if_present(
                journal,
                crate::ExecutionJournalEntry::ToolCallStarted {
                    turn_id: intent.turn_id.clone(),
                    step: intent.step,
                    call_id: call.call.id.clone(),
                },
            )
            .map_err(ToolBatchRecoveryError::Journal)?;
            let request = recovery_request(&call.call, context, control)
                .with_known_context_injections(session.context_injections());
            (tools.execute_outcome(&request), true)
        };
        append_recovered_call(
            &intent,
            call.call,
            outcome,
            started,
            max_output_bytes,
            session,
            observer,
            journal,
            &mut context_messages,
            &mut context_injections,
            &mut executed,
        )?;
    }
    let candidate_injections = std::mem::take(&mut context_injections);
    for (index, text) in context_messages.into_iter().enumerate() {
        if let Some(record) = candidate_injections.get(index).cloned() {
            if let Some(record) = session.append_context_injection(text, record) {
                context_injections.push(record);
            }
        } else {
            session.push(mini_agent_protocol::Message::Context { text });
        }
    }
    if !context_injections.is_empty() {
        observer.observe(&Event::ContextInjected {
            records: context_injections,
        });
    }
    Ok(executed)
}

fn recovery_request(
    call: &ToolCall,
    context: Option<&ToolExecutionContext>,
    control: &RunControl,
) -> ToolExecutionRequest {
    let request = ToolExecutionRequest::from(call.clone());
    let request = if let Some(context) = context {
        request.with_context(context.clone())
    } else {
        request
    };
    request.with_cancellation(control.cancellation_token())
}

#[allow(clippy::too_many_arguments)]
fn append_recovered_call<O: Observer>(
    intent: &crate::ToolBatchIntent,
    call: ToolCall,
    outcome: ToolExecutionOutcome,
    started: bool,
    max_output_bytes: usize,
    session: &mut SessionState,
    observer: &mut O,
    journal: &mut Option<&mut dyn crate::ExecutionJournalSink>,
    context_messages: &mut Vec<String>,
    context_injections: &mut Vec<mini_agent_protocol::ContextInjectionRecord>,
    executed: &mut Vec<(String, serde_json::Value, String)>,
) -> Result<(), ToolBatchRecoveryError> {
    let durable_outcome = outcome.clone();
    context_messages.extend(outcome.context_messages.iter().cloned());
    context_injections.extend(outcome.context_injections.iter().cloned());
    let content = outcome.content;
    let is_error = outcome.status.is_error();
    let truncated = outcome.output_truncated || content.len() > max_output_bytes;
    let content = truncate_utf8(content, max_output_bytes);
    if started {
        observer.observe(&Event::ToolStarted { call: call.clone() });
    }
    crate::execution::append_if_present(
        journal,
        crate::ExecutionJournalEntry::ToolCallFinished {
            turn_id: intent.turn_id.clone(),
            step: intent.step,
            call_id: call.id.clone(),
            outcome: durable_outcome,
        },
    )
    .map_err(ToolBatchRecoveryError::Journal)?;
    observer.observe(&Event::ToolFinished {
        call_id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        content: content.clone(),
        is_error,
        truncated,
        outcome: Some(outcome.status),
    });
    session.push(mini_agent_protocol::Message::Tool {
        call_id: call.id,
        name: call.name.clone(),
        content: content.clone(),
        is_error,
        outcome: Some(outcome.status),
    });
    executed.push((call.name, call.arguments, content));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_tool_result<O: Observer>(
    call: &ToolCall,
    outcome: ToolExecutionOutcome,
    max_output_bytes: usize,
    session: &mut SessionState,
    observer: &mut O,
    turn_id: Option<&TurnId>,
    step: usize,
    journal: &mut Option<&mut dyn crate::ExecutionJournalSink>,
    executed: &mut Vec<(String, serde_json::Value, String)>,
) -> Result<(), String> {
    if let Some(turn_id) = turn_id {
        crate::execution::append_if_present(
            journal,
            crate::ExecutionJournalEntry::ToolCallFinished {
                turn_id: turn_id.clone(),
                step,
                call_id: call.id.clone(),
                outcome: outcome.clone(),
            },
        )?;
    }
    let is_error = outcome.status.is_error();
    let content = outcome.content.clone();
    let truncated = outcome.output_truncated || content.len() > max_output_bytes;
    let content = truncate_utf8(content, max_output_bytes);
    observer.observe(&Event::ToolFinished {
        call_id: call.id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
        content: content.clone(),
        is_error,
        truncated,
        outcome: Some(outcome.status),
    });
    session.push(mini_agent_protocol::Message::Tool {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: content.clone(),
        is_error,
        outcome: Some(outcome.status),
    });
    executed.push((call.name.clone(), call.arguments.clone(), content));
    Ok(())
}

pub(super) fn truncate_utf8(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }

    const MARKER: &str = "\n[truncated]";
    if max_bytes <= MARKER.len() {
        content.truncate(floor_char_boundary(&content, max_bytes));
        return content;
    }

    let retained_bytes = max_bytes - MARKER.len();
    let head_bytes = retained_bytes.div_ceil(2);
    let tail_bytes = retained_bytes - head_bytes;
    let head_end = floor_char_boundary(&content, head_bytes);
    let tail_start = ceil_char_boundary(&content, content.len() - tail_bytes);
    let mut output = String::with_capacity(max_bytes);
    output.push_str(&content[..head_end]);
    output.push_str(MARKER);
    output.push_str(&content[tail_start..]);
    output
}

pub(super) fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}
