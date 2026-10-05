use crate::ToolRouter;
use mini_agent_protocol::Event;
use mini_agent_protocol::LimitExceeded;
use mini_agent_protocol::LimitKind;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::Observer;
use mini_agent_protocol::StopReason;
use std::error::Error;
use std::fmt;

use crate::SessionState;
use crate::context_controller::assemble_compacted;
use crate::context_controller::bounded_compaction_prompt;
use crate::context_controller::mechanical_compact;
use crate::context_controller::split_compaction_parts;
use crate::context_controller::trim_prefix_to_fit;
use crate::execution::ExecutionCheckpoint;
use crate::execution::ExecutionJournalEntry;
use crate::execution::ExecutionPhase;
use crate::execution::ToolBatchIntent;
use crate::run_control::RunControl;
use crate::run_control::SteeringMode;
use crate::session::context_byte_breakdown_for;
use crate::session::context_bytes_for;
use crate::session::model_input_digest;
use crate::session::tool_manifest_digest;
use crate::tool_batch_executor::execute_tool_batch;
use crate::turn_engine::ModelEventForwarder;
use crate::turn_engine::SilentModelEvents;
use crate::turn_engine::model_response_bytes;

const MAX_CONTEXT_INJECTION_BYTES: usize = 64 * 1024;
const MAX_CONTEXT_WINDOW_COMPACTION_RETRIES: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextLimitBehavior {
    Reject,
    Compact,
}

/// Controls how a settled history is prepared for an independent Session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkContextPolicy {
    Exact,
    /// Explicitly compact the fork checkpoint before it is persisted.
    Compact,
}

/// Records how the Core prepared a fork checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkCompactionMethod {
    Exact,
    ModelSummary,
    Mechanical,
}

/// A bounded, storage-neutral checkpoint prepared for a child Session.
#[derive(Clone, Debug, PartialEq)]
pub struct ForkPreparation {
    pub session: SessionState,
    pub context_before_bytes: usize,
    pub context_after_bytes: usize,
    pub method: ForkCompactionMethod,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessConfig {
    pub system_prompt: String,
    /// Maximum model steps in one run. `0` means no step cap.
    pub max_steps: usize,
    pub max_context_item_bytes: usize,
    pub max_user_input_bytes: usize,
    pub max_model_response_bytes: usize,
    pub max_tool_calls_per_step: usize,
    pub max_tool_output_bytes: usize,
    pub max_context_bytes: usize,
    pub context_limit_behavior: ContextLimitBehavior,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            system_prompt: include_str!("../builtin/prompts/system/default.md")
                .trim_end()
                .to_string(),
            max_steps: 8,
            max_context_item_bytes: 8 * 1024,
            max_user_input_bytes: 32 * 1024,
            max_model_response_bytes: 16 * 1024 * 1024,
            max_tool_calls_per_step: 8,
            max_tool_output_bytes: 16 * 1024,
            max_context_bytes: 64 * 1024 * 1024,
            context_limit_behavior: ContextLimitBehavior::Compact,
        }
    }
}

impl HarnessConfig {
    /// Select the long-running loop semantics used by an explicit Goal/
    /// Auto-Copilot workflow. Approval policy remains a separate Host concern.
    pub fn with_copilot_loop(mut self) -> Self {
        self.max_steps = 0;
        self.context_limit_behavior = ContextLimitBehavior::Compact;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub final_text: String,
    pub messages: Vec<Message>,
    pub steps: usize,
    pub stop_reason: StopReason,
}

#[derive(Debug)]
pub enum HarnessError<E> {
    Model(E),
    Compaction(String),
    Limit(LimitExceeded),
    Thread(String),
    ExecutionJournal(String),
    NeedsReconciliation(String),
}

impl<E: fmt::Display> fmt::Display for HarnessError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "model request failed: {error}"),
            Self::Compaction(error) => write!(formatter, "context compaction failed: {error}"),
            Self::Limit(error) => error.fmt(formatter),
            Self::Thread(error) => write!(formatter, "thread operation failed: {error}"),
            Self::ExecutionJournal(error) => {
                write!(
                    formatter,
                    "execution checkpoint could not be persisted: {error}"
                )
            }
            Self::NeedsReconciliation(reason) => {
                write!(
                    formatter,
                    "execution needs reconciliation before continuing: {reason}"
                )
            }
        }
    }
}

impl<E: Error + 'static> Error for HarnessError<E> {}

pub struct Harness<M> {
    model: M,
    tools: ToolRouter,
    config: HarnessConfig,
    session: SessionState,
    pending_context_injections: Vec<(String, mini_agent_protocol::ContextInjectionRecord)>,
    model_selection: Option<mini_agent_protocol::ModelSelection>,
    reasoning_selection: Option<mini_agent_protocol::ReasoningSelection>,
    reasoning_effort: Option<String>,
}

enum ControlAction {
    Proceed,
    ContinueTurn,
    Finish(RunOutcome),
}

impl<M: Model> Harness<M> {
    pub fn new(model: M, tools: ToolRouter, config: HarnessConfig) -> Self {
        Self {
            model,
            tools,
            config,
            session: SessionState::new(),
            pending_context_injections: Vec::new(),
            model_selection: None,
            reasoning_selection: None,
            reasoning_effort: None,
        }
    }

    pub fn system_prompt(&self) -> &str {
        &self.config.system_prompt
    }

    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    pub(crate) fn validate_user_input(&self, text: &str) -> Result<(), LimitExceeded> {
        if text.len() > self.config.max_user_input_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::UserInputBytes,
                limit: self.config.max_user_input_bytes,
                actual: text.len(),
            });
        }
        Ok(())
    }

    /// Sets model metadata for the next complete turn. The concrete provider
    /// still resolves IDs and credentials outside Core.
    pub fn set_model_selection(
        &mut self,
        selection: Option<mini_agent_protocol::ModelSelection>,
        reasoning_effort: Option<String>,
    ) {
        self.model_selection = selection;
        self.reasoning_selection = None;
        self.reasoning_effort = reasoning_effort;
    }

    /// Sets model and reasoning choices for the next complete Turn.
    pub fn set_model_preferences(
        &mut self,
        selection: Option<mini_agent_protocol::ModelSelection>,
        reasoning_selection: Option<mini_agent_protocol::ReasoningSelection>,
        legacy_reasoning_effort: Option<String>,
    ) {
        self.model_selection = selection;
        self.reasoning_selection = reasoning_selection;
        self.reasoning_effort = legacy_reasoning_effort;
    }

    /// Replaces the model-visible system prompt at a settled control-plane
    /// boundary. The caller must provide a bounded, fully composed prompt;
    /// prompt source selection remains owned by the Host.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.config.system_prompt = prompt.into();
    }

    pub fn messages(&self) -> &[Message] {
        self.session.messages()
    }

    pub fn session_state(&self) -> &SessionState {
        &self.session
    }

    pub fn tool_specs(&self) -> Vec<mini_agent_protocol::ToolSpec> {
        self.tools.specs()
    }

    pub fn clear_history(&mut self) {
        self.session.clear();
    }

    pub fn append_context(&mut self, text: impl Into<String>) -> Result<(), LimitExceeded> {
        let text = text.into();
        if text.len() > self.config.max_context_item_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::ContextItemBytes,
                limit: self.config.max_context_item_bytes,
                actual: text.len(),
            });
        }
        self.session.push(Message::Context { text });
        Ok(())
    }

    /// Appends a metadata-backed Host context source without mutating any
    /// prior model message. Repeated active fingerprints are ignored.
    pub fn append_context_injection(
        &mut self,
        text: impl Into<String>,
        record: mini_agent_protocol::ContextInjectionRecord,
    ) -> Result<Option<mini_agent_protocol::ContextInjectionRecord>, LimitExceeded> {
        let text = text.into();
        if text.len() > MAX_CONTEXT_INJECTION_BYTES {
            return Err(LimitExceeded {
                kind: LimitKind::ContextItemBytes,
                limit: MAX_CONTEXT_INJECTION_BYTES,
                actual: text.len(),
            });
        }
        let appended = self.session.append_context_injection(text.clone(), record);
        if let Some(record) = &appended {
            self.pending_context_injections.push((text, record.clone()));
        }
        Ok(appended)
    }

    pub fn append_context_slot_if_changed(
        &mut self,
        slot: &str,
        text: impl Into<String>,
    ) -> Result<bool, LimitExceeded> {
        let text = text.into();
        if text.len() > self.config.max_context_item_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::ContextItemBytes,
                limit: self.config.max_context_item_bytes,
                actual: text.len(),
            });
        }
        Ok(self.session.append_context_slot_if_changed(slot, text))
    }

    pub fn restore_history(&mut self, messages: Vec<Message>) -> Result<(), LimitExceeded> {
        self.restore_session(SessionState::from_messages(messages))
    }

    pub fn restore_session(&mut self, mut session: SessionState) -> Result<(), LimitExceeded> {
        let mut pending_context_injections = Vec::new();
        for (text, record) in &self.pending_context_injections {
            if let Some(record) = session.append_context_injection(text.clone(), record.clone()) {
                pending_context_injections.push((text.clone(), record));
            }
        }
        let messages = session.messages();
        for message in messages {
            match message {
                Message::Context { text } => {
                    let limit =
                        if mini_agent_protocol::ContextInjectionRecord::from_context_message(text)
                            .is_some()
                        {
                            MAX_CONTEXT_INJECTION_BYTES
                        } else {
                            self.config.max_context_item_bytes
                        };
                    if text.len() > limit {
                        return Err(LimitExceeded {
                            kind: LimitKind::ContextItemBytes,
                            limit,
                            actual: text.len(),
                        });
                    }
                }
                Message::User { text } if text.len() > self.config.max_user_input_bytes => {
                    return Err(LimitExceeded {
                        kind: LimitKind::UserInputBytes,
                        limit: self.config.max_user_input_bytes,
                        actual: text.len(),
                    });
                }
                Message::Assistant {
                    reasoning,
                    text,
                    tool_calls,
                } => {
                    let actual = reasoning.len()
                        + text.len()
                        + serde_json::to_vec(tool_calls).map(|v| v.len()).unwrap_or(0);
                    if actual > self.config.max_model_response_bytes {
                        return Err(LimitExceeded {
                            kind: LimitKind::ModelResponseBytes,
                            limit: self.config.max_model_response_bytes,
                            actual,
                        });
                    }
                    if tool_calls.len() > self.config.max_tool_calls_per_step {
                        return Err(LimitExceeded {
                            kind: LimitKind::ToolCallsPerStep,
                            limit: self.config.max_tool_calls_per_step,
                            actual: tool_calls.len(),
                        });
                    }
                }
                Message::Tool { content, .. }
                    if content.len() > self.config.max_tool_output_bytes =>
                {
                    return Err(LimitExceeded {
                        kind: LimitKind::ToolOutputBytes,
                        limit: self.config.max_tool_output_bytes,
                        actual: content.len(),
                    });
                }
                _ => {}
            }
        }
        session.repair_incomplete_tool_groups();
        let messages = session.messages();
        let tool_specs = self.tools.specs();
        let actual = context_bytes_for(&self.config.system_prompt, messages, &tool_specs);
        if actual > self.config.max_context_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::ContextBytes,
                limit: self.config.max_context_bytes,
                actual,
            });
        }
        self.session = session;
        self.pending_context_injections = pending_context_injections;
        Ok(())
    }

    /// Prepares a settled copy of the current history for an independent
    /// Session without changing this Harness.
    ///
    /// `Exact` only clones the settled history. `Compact` may summarize it with
    /// an empty tool list and a silent observer. Tool execution, approval, and
    /// turn events cannot occur during this operation.
    pub async fn prepare_fork_checkpoint(
        &mut self,
        policy: ForkContextPolicy,
    ) -> Result<ForkPreparation, HarnessError<M::Error>> {
        let tool_specs = self.tools.specs();
        let before_bytes = self.context_bytes(&self.config.system_prompt, &tool_specs);
        if policy == ForkContextPolicy::Exact {
            self.ensure_context_limit(&tool_specs)
                .map_err(HarnessError::Limit)?;
            return Ok(ForkPreparation {
                session: self.session.clone(),
                context_before_bytes: before_bytes,
                context_after_bytes: before_bytes,
                method: ForkCompactionMethod::Exact,
            });
        }

        let original = self.session.clone();
        let method = match self.compact_context(&tool_specs, &mut ()).await {
            Ok(method) => method,
            Err(error) => {
                self.session = original;
                return Err(error);
            }
        };
        let after_bytes = self.context_bytes(&self.config.system_prompt, &tool_specs);
        let result = self
            .ensure_context_limit(&tool_specs)
            .map_err(HarnessError::Limit)
            .map(|()| ForkPreparation {
                session: self.session.clone(),
                context_before_bytes: before_bytes,
                context_after_bytes: after_bytes,
                method,
            });
        self.session = original;
        result
    }

    pub fn replace_config(&mut self, config: HarnessConfig) {
        self.config = config;
    }

    pub fn extend_tools(&mut self, tools: Vec<Box<dyn mini_agent_protocol::Tool>>) {
        self.tools.extend(tools);
    }

    /// Applies a Host-computed visibility filter while retaining the tool
    /// implementations for later Thread setting changes.
    pub fn set_hidden_tools(&mut self, names: Vec<String>) {
        self.tools.set_hidden_tools(names);
    }

    pub async fn run<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        self.run_with_control(prompt, observer, &RunControl::new())
            .await
    }

    pub async fn run_with_control<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
        control: &RunControl,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        self.run_with_control_mode(prompt, observer, control, SteeringMode::StopAtCheckpoint)
            .await
    }

    pub async fn run_with_control_mode<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        self.run_with_control_mode_and_tool_context(
            prompt,
            observer,
            control,
            steering_mode,
            crate::ExecutionRunOptions::default(),
        )
        .await
    }

    pub(crate) async fn run_with_control_mode_and_tool_context<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
        options: crate::ExecutionRunOptions<'_>,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        let crate::ExecutionRunOptions {
            tool_context,
            execution_context,
            mut journal,
            resume: execution_resume,
        } = options;
        // Recover histories produced by the pre-fix steering boundary before
        // appending another user message or making a provider request.
        self.session.repair_incomplete_tool_groups();
        let prompt = prompt.into();
        let resume_checkpoint = execution_resume
            .as_ref()
            .map(|(checkpoint, _)| checkpoint.clone());
        let prompt = resume_checkpoint
            .as_ref()
            .map_or(prompt, |checkpoint| checkpoint.input.text.clone());
        if let Err(limit) = self.validate_user_input(&prompt) {
            return Err(fail_limit(limit, observer));
        }
        observer.observe(&Event::RunStarted {
            prompt: prompt.clone(),
        });

        let previous_message_count = self.session.messages().len();
        let tool_specs = self.tools.specs();
        let (mut final_text, mut step) = if let Some(checkpoint) = resume_checkpoint.as_ref() {
            self.restore_history(checkpoint.messages.clone())
                .map_err(HarnessError::Limit)?;
            self.ensure_context_limit(&tool_specs)
                .map_err(HarnessError::Limit)?;
            (
                checkpoint.final_text.clone(),
                checkpoint.next_model_step.saturating_sub(1),
            )
        } else {
            self.session.push(Message::User { text: prompt });
            if let Some(execution) = execution_context.as_ref() {
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::Checkpoint {
                        checkpoint: ExecutionCheckpoint {
                            turn_id: execution.turn_id.clone(),
                            input: execution.input.clone(),
                            messages: self.session.messages().to_vec(),
                            next_model_step: 1,
                            final_text: String::new(),
                            phase: ExecutionPhase::ModelRequest,
                        },
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
            }
            if let Err(error) = self.prepare_context(&tool_specs, observer).await {
                if self.config.context_limit_behavior == ContextLimitBehavior::Reject {
                    self.session.truncate_messages(previous_message_count);
                }
                return Err(error);
            }
            (String::new(), 0)
        };

        if let Some((checkpoint, Some(batch))) = execution_resume {
            if checkpoint.turn_id != batch.intent.turn_id
                || checkpoint.next_model_step != batch.intent.step
            {
                return Err(HarnessError::NeedsReconciliation(
                    "checkpoint and pending tool batch do not identify the same model step"
                        .to_string(),
                ));
            }
            if let Err(reason) =
                crate::tool_batch_executor::validate_tool_batch_recovery(&self.tools, &batch)
            {
                if let Some(execution) = execution_context.as_ref() {
                    crate::execution::append_if_present(
                        &mut journal,
                        ExecutionJournalEntry::NeedsReconciliation {
                            turn_id: execution.turn_id.clone(),
                            reason: reason.clone(),
                        },
                    )
                    .map_err(HarnessError::ExecutionJournal)?;
                }
                return Err(HarnessError::NeedsReconciliation(reason));
            }
            self.session.push(Message::Assistant {
                reasoning: batch.intent.reasoning.clone(),
                text: batch.intent.text.clone(),
                tool_calls: batch.intent.calls.clone(),
            });
            final_text.clone_from(&batch.intent.text);
            step = batch.intent.step;
            let current_executed_batch = crate::tool_batch_executor::recover_tool_batch(
                &self.tools,
                batch,
                &mut self.session,
                observer,
                control,
                crate::tool_batch_executor::ToolBatchOptions {
                    max_output_bytes: self.config.max_tool_output_bytes,
                    context: tool_context.as_ref(),
                    turn_id: None,
                    step,
                    journal: &mut journal,
                },
            )
            .map_err(|error| match error {
                crate::tool_batch_executor::ToolBatchRecoveryError::NeedsReconciliation(reason) => {
                    HarnessError::NeedsReconciliation(reason)
                }
                crate::tool_batch_executor::ToolBatchRecoveryError::Journal(error) => {
                    HarnessError::ExecutionJournal(error)
                }
            })?;
            let _ = current_executed_batch;
            if let Some(execution) = execution_context.as_ref() {
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::ToolBatchSettled {
                        turn_id: execution.turn_id.clone(),
                        step,
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::Checkpoint {
                        checkpoint: ExecutionCheckpoint {
                            turn_id: execution.turn_id.clone(),
                            input: execution.input.clone(),
                            messages: self.session.messages().to_vec(),
                            next_model_step: step.saturating_add(1),
                            final_text: final_text.clone(),
                            phase: ExecutionPhase::ModelRequest,
                        },
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
            }
        }

        let mut consecutive_duplicate_tool_batches = 0usize;
        let mut last_tool_batch: Option<Vec<(String, serde_json::Value, String)>> = None;

        loop {
            match self.control_action(&mut final_text, step, control, steering_mode, observer)? {
                ControlAction::Proceed => {}
                ControlAction::ContinueTurn => continue,
                ControlAction::Finish(outcome) => return Ok(outcome),
            }
            step = step.saturating_add(1);
            if self.config.max_steps != 0 && step > self.config.max_steps {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step.saturating_sub(1),
                    StopReason::StepLimit,
                    observer,
                ));
            }
            if !self.pending_context_injections.is_empty() {
                observer.observe(&Event::ContextInjected {
                    records: std::mem::take(&mut self.pending_context_injections)
                        .into_iter()
                        .map(|(_, record)| record)
                        .collect(),
                });
            }
            self.prepare_context(&tool_specs, observer).await?;
            if let Some(execution) = execution_context.as_ref() {
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::Checkpoint {
                        checkpoint: ExecutionCheckpoint {
                            turn_id: execution.turn_id.clone(),
                            input: execution.input.clone(),
                            messages: self.session.messages().to_vec(),
                            next_model_step: step,
                            final_text: final_text.clone(),
                            phase: ExecutionPhase::ModelRequest,
                        },
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
            }
            let mut context_window_retries = 0;
            let (response, model_timing) = loop {
                observer.observe(&Event::ModelStarted {
                    step,
                    input_bytes: self.context_bytes(&self.config.system_prompt, &tool_specs),
                    input_hash: model_input_digest(
                        &self.config.system_prompt,
                        self.session.messages(),
                        &tool_specs,
                    ),
                    tool_manifest_hash: tool_manifest_digest(&tool_specs),
                });
                let model_response = {
                    let mut model_events =
                        ModelEventForwarder::new(observer, self.config.max_model_response_bytes);
                    let result = self
                        .model
                        .respond(
                            ModelRequest {
                                system_prompt: &self.config.system_prompt,
                                messages: self.session.messages(),
                                tools: &tool_specs,
                                allowed_tools: None,
                                max_response_bytes: self.config.max_model_response_bytes,
                                model_selection: self.model_selection.as_ref(),
                                reasoning_selection: self.reasoning_selection.as_ref(),
                                reasoning_effort: self.reasoning_effort.as_deref(),
                            },
                            &mut model_events,
                        )
                        .await;
                    result.map(|response| (response, model_events.timing()))
                };
                match model_response {
                    Ok(response) => break response,
                    Err(error)
                        if self.config.context_limit_behavior == ContextLimitBehavior::Compact
                            && context_window_retries < MAX_CONTEXT_WINDOW_COMPACTION_RETRIES
                            && self.model.is_context_window_error(&error) =>
                    {
                        context_window_retries += 1;
                        if self
                            .compact_context_after_window_error(&tool_specs, observer)
                            .await?
                        {
                            if let Some(execution) = execution_context.as_ref() {
                                crate::execution::append_if_present(
                                    &mut journal,
                                    ExecutionJournalEntry::Checkpoint {
                                        checkpoint: ExecutionCheckpoint {
                                            turn_id: execution.turn_id.clone(),
                                            input: execution.input.clone(),
                                            messages: self.session.messages().to_vec(),
                                            next_model_step: step,
                                            final_text: final_text.clone(),
                                            phase: ExecutionPhase::ModelRequest,
                                        },
                                    },
                                )
                                .map_err(HarnessError::ExecutionJournal)?;
                            }
                        } else {
                            observer.observe(&Event::RunFailed {
                                reason: mini_agent_protocol::RunFailure::Model,
                            });
                            return Err(HarnessError::Model(error));
                        }
                    }
                    Err(error) => {
                        observer.observe(&Event::RunFailed {
                            reason: mini_agent_protocol::RunFailure::Model,
                        });
                        return Err(HarnessError::Model(error));
                    }
                }
            };

            let response_bytes = model_response_bytes(&response);
            if response_bytes > self.config.max_model_response_bytes {
                return Err(fail_limit(
                    LimitExceeded {
                        kind: LimitKind::ModelResponseBytes,
                        limit: self.config.max_model_response_bytes,
                        actual: response_bytes,
                    },
                    observer,
                ));
            }
            if response.tool_calls.len() > self.config.max_tool_calls_per_step {
                return Err(fail_limit(
                    LimitExceeded {
                        kind: LimitKind::ToolCallsPerStep,
                        limit: self.config.max_tool_calls_per_step,
                        actual: response.tool_calls.len(),
                    },
                    observer,
                ));
            }

            observer.observe(&Event::ModelResponded {
                reasoning: response.reasoning.clone(),
                text: response.text.clone(),
                tool_calls: response.tool_calls.clone(),
                usage: response.usage,
                model_timing: Some(model_timing),
                context_bytes: Some(context_byte_breakdown_for(
                    &self.config.system_prompt,
                    self.session.messages(),
                    &tool_specs,
                )),
            });
            final_text = response.text.clone();
            self.session.push(Message::Assistant {
                reasoning: response.reasoning.clone(),
                text: response.text.clone(),
                tool_calls: response.tool_calls.clone(),
            });

            if response.tool_calls.is_empty() {
                match self.control_action(
                    &mut final_text,
                    step,
                    control,
                    steering_mode,
                    observer,
                )? {
                    ControlAction::Proceed => {}
                    ControlAction::ContinueTurn => continue,
                    ControlAction::Finish(outcome) => return Ok(outcome),
                }
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Completed,
                    observer,
                ));
            }

            if let Some(execution) = execution_context.as_ref() {
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::ToolBatchStarted {
                        batch: ToolBatchIntent {
                            turn_id: execution.turn_id.clone(),
                            step,
                            reasoning: response.reasoning.clone(),
                            text: response.text.clone(),
                            calls: response.tool_calls.clone(),
                        },
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
            }

            let current_executed_batch = execute_tool_batch(
                &self.tools,
                response.tool_calls,
                &mut self.session,
                observer,
                control,
                crate::tool_batch_executor::ToolBatchOptions {
                    max_output_bytes: self.config.max_tool_output_bytes,
                    context: tool_context.as_ref(),
                    turn_id: execution_context.as_ref().map(|context| &context.turn_id),
                    step,
                    journal: &mut journal,
                },
            )
            .map_err(HarnessError::ExecutionJournal)?;

            if let Some(execution) = execution_context.as_ref() {
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::ToolBatchSettled {
                        turn_id: execution.turn_id.clone(),
                        step,
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
                crate::execution::append_if_present(
                    &mut journal,
                    ExecutionJournalEntry::Checkpoint {
                        checkpoint: ExecutionCheckpoint {
                            turn_id: execution.turn_id.clone(),
                            input: execution.input.clone(),
                            messages: self.session.messages().to_vec(),
                            next_model_step: step.saturating_add(1),
                            final_text: final_text.clone(),
                            phase: ExecutionPhase::ModelRequest,
                        },
                    },
                )
                .map_err(HarnessError::ExecutionJournal)?;
            }

            if last_tool_batch.as_ref() == Some(&current_executed_batch) {
                consecutive_duplicate_tool_batches =
                    consecutive_duplicate_tool_batches.saturating_add(1);
            } else {
                consecutive_duplicate_tool_batches = 0;
                last_tool_batch = Some(current_executed_batch);
            }

            if consecutive_duplicate_tool_batches >= 2 {
                let _ = self.append_context(LOOP_WARNING_TEXT);
            }

            match self.control_action(&mut final_text, step, control, steering_mode, observer)? {
                ControlAction::Proceed => {}
                ControlAction::ContinueTurn => continue,
                ControlAction::Finish(outcome) => return Ok(outcome),
            }

            if let Err(limit) = self.ensure_context_limit(&tool_specs) {
                return Err(fail_limit(limit, observer));
            }
        }
    }

    fn control_action<O: Observer + Send>(
        &mut self,
        final_text: &mut String,
        step: usize,
        control: &RunControl,
        steering_mode: SteeringMode,
        observer: &mut O,
    ) -> Result<ControlAction, HarnessError<M::Error>> {
        if control.take_cancel_requested() {
            return Ok(ControlAction::Finish(finish(
                std::mem::take(final_text),
                self.session.messages().to_vec(),
                step,
                StopReason::Cancelled,
                observer,
            )));
        }
        if steering_mode == SteeringMode::ContinueSameTurn
            && let Some(input) = control.take_steer_input()
        {
            if let Err(limit) = self.append_user_input(input.text) {
                return Err(fail_limit(limit, observer));
            }
            final_text.clear();
            return Ok(ControlAction::ContinueTurn);
        }
        if control.is_steer_requested() {
            return Ok(ControlAction::Finish(finish(
                std::mem::take(final_text),
                self.session.messages().to_vec(),
                step,
                StopReason::Steered,
                observer,
            )));
        }
        Ok(ControlAction::Proceed)
    }

    fn ensure_context_limit(
        &self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
    ) -> Result<(), LimitExceeded> {
        let actual = self.context_bytes(&self.config.system_prompt, tool_specs);
        if actual <= self.config.max_context_bytes {
            Ok(())
        } else {
            Err(LimitExceeded {
                kind: LimitKind::ContextBytes,
                limit: self.config.max_context_bytes,
                actual,
            })
        }
    }

    fn append_user_input(&mut self, text: String) -> Result<(), LimitExceeded> {
        if text.len() > self.config.max_user_input_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::UserInputBytes,
                limit: self.config.max_user_input_bytes,
                actual: text.len(),
            });
        }
        self.session.push(Message::User { text });
        Ok(())
    }

    fn context_bytes(
        &self,
        system_prompt: &str,
        tool_specs: &[mini_agent_protocol::ToolSpec],
    ) -> usize {
        self.session.context_bytes(system_prompt, tool_specs)
    }

    async fn prepare_context<O: Observer + Send>(
        &mut self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
    ) -> Result<(), HarnessError<M::Error>> {
        let actual = self.context_bytes(&self.config.system_prompt, tool_specs);
        let compact_at = self.config.max_context_bytes / 2;
        let should_compact = self.config.context_limit_behavior == ContextLimitBehavior::Compact
            && self.session.messages().len() > 1
            && actual >= compact_at;
        if should_compact {
            let _ = self.compact_context(tool_specs, observer).await?;
        }
        self.ensure_context_limit(tool_specs)
            .map_err(|limit| fail_limit(limit, observer))
    }

    async fn compact_context<O: Observer + Send>(
        &mut self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
    ) -> Result<ForkCompactionMethod, HarnessError<M::Error>> {
        let compact_at = self.config.max_context_bytes / 2;
        self.compact_context_to(tool_specs, observer, compact_at, false)
            .await
    }

    async fn compact_context_after_window_error<O: Observer + Send>(
        &mut self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
    ) -> Result<bool, HarnessError<M::Error>> {
        let before_bytes = self.context_bytes(&self.config.system_prompt, tool_specs);
        let target_bytes = before_bytes.saturating_mul(9) / 10;
        let method = self
            .compact_context_to(tool_specs, observer, target_bytes, true)
            .await?;
        let after_bytes = self.context_bytes(&self.config.system_prompt, tool_specs);
        Ok(method != ForkCompactionMethod::Exact && after_bytes < before_bytes)
    }

    async fn compact_context_to<O: Observer + Send>(
        &mut self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
        compact_at: usize,
        summary_must_reach_target: bool,
    ) -> Result<ForkCompactionMethod, HarnessError<M::Error>> {
        let before_bytes = self.context_bytes(&self.config.system_prompt, tool_specs);
        let (mut prefix, contexts, tail) = split_compaction_parts(self.session.messages());
        if prefix.is_empty() {
            return Ok(ForkCompactionMethod::Exact);
        }
        let compaction_prompt = bounded_compaction_prompt(self.config.max_user_input_bytes);
        observer.observe(&Event::ContextCompactionStarted { before_bytes });
        trim_prefix_to_fit(
            &mut prefix,
            &compaction_prompt,
            &self.config.system_prompt,
            &[],
            self.config.max_context_bytes,
        );
        if prefix.is_empty() {
            let compacted =
                assemble_compacted(None, contexts, tail, self.config.max_user_input_bytes);
            self.finish_compacted(compacted, before_bytes, None, tool_specs, observer)?;
            return Ok(ForkCompactionMethod::Mechanical);
        }
        let mut compaction_messages = prefix.clone();
        compaction_messages.push(Message::User {
            text: compaction_prompt,
        });
        let response = self
            .model
            .respond(
                ModelRequest {
                    system_prompt: &self.config.system_prompt,
                    messages: &compaction_messages,
                    tools: &[],
                    allowed_tools: None,
                    max_response_bytes: self.config.max_model_response_bytes,
                    model_selection: self.model_selection.as_ref(),
                    reasoning_selection: self.reasoning_selection.as_ref(),
                    reasoning_effort: self.reasoning_effort.as_deref(),
                },
                &mut SilentModelEvents,
            )
            .await
            .ok();
        let mut compacted = None;
        if let Some(response) = response.as_ref()
            && model_response_bytes(response) <= self.config.max_model_response_bytes
        {
            let summary = response.text.trim();
            if response.tool_calls.is_empty() && !summary.is_empty() {
                let candidate = assemble_compacted(
                    Some(summary),
                    contexts.clone(),
                    tail.clone(),
                    self.config.max_user_input_bytes,
                );
                let after_bytes =
                    context_bytes_for(&self.config.system_prompt, &candidate, tool_specs);
                let reaches_target = !summary_must_reach_target || after_bytes < compact_at;
                if after_bytes < before_bytes
                    && after_bytes <= self.config.max_context_bytes
                    && reaches_target
                {
                    compacted = Some(candidate);
                }
            }
        }
        let method = if compacted.is_some() {
            ForkCompactionMethod::ModelSummary
        } else {
            ForkCompactionMethod::Mechanical
        };
        let compacted = compacted.unwrap_or_else(|| {
            mechanical_compact(
                prefix,
                contexts,
                tail,
                compact_at,
                &self.config.system_prompt,
                tool_specs,
                self.config.max_user_input_bytes,
            )
        });
        let usage = response.and_then(|response| response.usage);
        self.finish_compacted(compacted, before_bytes, usage, tool_specs, observer)?;
        Ok(method)
    }

    fn finish_compacted<O: Observer + Send>(
        &mut self,
        compacted: Vec<Message>,
        before_bytes: usize,
        usage: Option<mini_agent_protocol::ModelUsage>,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
    ) -> Result<(), HarnessError<M::Error>> {
        let after_bytes = context_bytes_for(&self.config.system_prompt, &compacted, tool_specs);
        if after_bytes >= before_bytes {
            return Err(fail_compaction(
                format!("summary did not reduce context: {before_bytes} -> {after_bytes} bytes"),
                observer,
            ));
        }
        if after_bytes > self.config.max_context_bytes {
            return Err(fail_limit(
                LimitExceeded {
                    kind: LimitKind::ContextBytes,
                    limit: self.config.max_context_bytes,
                    actual: after_bytes,
                },
                observer,
            ));
        }
        self.session.replace_messages(compacted);
        observer.observe(&Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            usage,
        });
        Ok(())
    }
}

const LOOP_WARNING_TEXT: &str = "[Loop warning: identical tool calls and outputs were repeated without progress. Please adjust arguments or try an alternate strategy.]";

fn finish<O: Observer>(
    final_text: String,
    messages: Vec<Message>,
    steps: usize,
    stop_reason: StopReason,
    observer: &mut O,
) -> RunOutcome {
    observer.observe(&Event::RunFinished { stop_reason, steps });
    RunOutcome {
        final_text,
        messages,
        steps,
        stop_reason,
    }
}

fn fail_limit<E, O: Observer>(limit: LimitExceeded, observer: &mut O) -> HarnessError<E> {
    observer.observe(&Event::RunFailed {
        reason: mini_agent_protocol::RunFailure::LimitExceeded(limit),
    });
    HarnessError::Limit(limit)
}

fn fail_compaction<E, O: Observer>(reason: String, observer: &mut O) -> HarnessError<E> {
    observer.observe(&Event::RunFailed {
        reason: mini_agent_protocol::RunFailure::Compaction,
    });
    HarnessError::Compaction(reason)
}

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;
