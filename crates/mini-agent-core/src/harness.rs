use crate::ToolRegistry;
use mini_agent_protocol::Event;
use mini_agent_protocol::LimitExceeded;
use mini_agent_protocol::LimitKind;
use mini_agent_protocol::Message;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::Observer;
use mini_agent_protocol::StopReason;
use mini_agent_protocol::ToolExecutionContext;
use std::error::Error;
use std::fmt;

use crate::SessionState;
use crate::context::context_bytes_for;
use crate::context::model_input_digest;
use crate::context::tool_manifest_digest;
use crate::context_controller::COMPACTION_PROMPT;
use crate::context_controller::assemble_compacted;
use crate::context_controller::mechanical_compact;
use crate::context_controller::split_compaction_parts;
use crate::context_controller::trim_prefix_to_fit;
use crate::run_control::RunControl;
use crate::run_control::SteeringMode;
use crate::tool_batch_executor::execute_tool_batch;
use crate::turn_engine::ModelEventForwarder;
use crate::turn_engine::SilentModelEvents;
use crate::turn_engine::model_response_bytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextLimitBehavior {
    Reject,
    Compact,
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
            system_prompt:
                "You are a coding agent. Use tools when needed and report the result plainly."
                    .to_string(),
            max_steps: 8,
            max_context_item_bytes: 8 * 1024,
            max_user_input_bytes: 32 * 1024,
            max_model_response_bytes: 64 * 1024,
            max_tool_calls_per_step: 8,
            max_tool_output_bytes: 16 * 1024,
            max_context_bytes: 1024 * 1024,
            context_limit_behavior: ContextLimitBehavior::Reject,
        }
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
}

impl<E: fmt::Display> fmt::Display for HarnessError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "model request failed: {error}"),
            Self::Compaction(error) => write!(formatter, "context compaction failed: {error}"),
            Self::Limit(error) => error.fmt(formatter),
            Self::Thread(error) => write!(formatter, "thread operation failed: {error}"),
        }
    }
}

impl<E: Error + 'static> Error for HarnessError<E> {}

pub struct Harness<M> {
    model: M,
    tools: ToolRegistry,
    config: HarnessConfig,
    session: SessionState,
}

impl<M: Model> Harness<M> {
    pub fn new(model: M, tools: ToolRegistry, config: HarnessConfig) -> Self {
        Self {
            model,
            tools,
            config,
            session: SessionState::new(),
        }
    }

    pub fn system_prompt(&self) -> &str {
        &self.config.system_prompt
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

    pub fn restore_history(&mut self, messages: Vec<Message>) -> Result<(), LimitExceeded> {
        self.restore_session(SessionState::from_messages(messages))
    }

    pub fn restore_session(&mut self, session: SessionState) -> Result<(), LimitExceeded> {
        let messages = session.messages();
        for message in messages {
            match message {
                Message::Context { text } if text.len() > self.config.max_context_item_bytes => {
                    return Err(LimitExceeded {
                        kind: LimitKind::ContextItemBytes,
                        limit: self.config.max_context_item_bytes,
                        actual: text.len(),
                    });
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
        Ok(())
    }

    pub fn replace_config(&mut self, config: HarnessConfig) {
        self.config = config;
    }

    pub fn extend_tools(&mut self, tools: Vec<Box<dyn mini_agent_protocol::Tool>>) {
        self.tools.extend(tools);
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
        self.run_with_control_mode_and_tool_context(prompt, observer, control, steering_mode, None)
            .await
    }

    pub(crate) async fn run_with_control_mode_and_tool_context<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
        control: &RunControl,
        steering_mode: SteeringMode,
        tool_context: Option<ToolExecutionContext>,
    ) -> Result<RunOutcome, HarnessError<M::Error>> {
        let prompt = prompt.into();
        if prompt.len() > self.config.max_user_input_bytes {
            return Err(fail_limit(
                LimitExceeded {
                    kind: LimitKind::UserInputBytes,
                    limit: self.config.max_user_input_bytes,
                    actual: prompt.len(),
                },
                observer,
            ));
        }
        observer.observe(&Event::RunStarted {
            prompt: prompt.clone(),
        });

        let previous_message_count = self.session.messages().len();
        self.session.push(Message::User { text: prompt });
        let tool_specs = self.tools.specs();
        if let Err(error) = self.prepare_context(&tool_specs, observer).await {
            if self.config.context_limit_behavior == ContextLimitBehavior::Reject {
                self.session.truncate_messages(previous_message_count);
            }
            return Err(error);
        }
        let mut final_text = String::new();
        let mut step = 0usize;
        let mut consecutive_duplicate_tool_batches = 0usize;
        let mut last_tool_batch: Option<Vec<(String, serde_json::Value, String)>> = None;

        loop {
            if control.take_cancel_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Cancelled,
                    observer,
                ));
            }
            if steering_mode == SteeringMode::ContinueSameTurn
                && let Some(input) = control.take_steer_input()
            {
                if let Err(limit) = self.append_user_input(input.text) {
                    return Err(fail_limit(limit, observer));
                }
                final_text.clear();
                continue;
            }
            if control.is_steer_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Steered,
                    observer,
                ));
            }
            step = step.saturating_add(1);
            if step > self.config.max_steps {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step.saturating_sub(1),
                    StopReason::StepLimit,
                    observer,
                ));
            }
            self.prepare_context(&tool_specs, observer).await?;
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
            let mut model_events = ModelEventForwarder {
                observer,
                emitted_bytes: 0,
                max_bytes: self.config.max_model_response_bytes,
            };
            let response = match self
                .model
                .respond(
                    ModelRequest {
                        system_prompt: &self.config.system_prompt,
                        messages: self.session.messages(),
                        tools: &tool_specs,
                        max_response_bytes: self.config.max_model_response_bytes,
                    },
                    &mut model_events,
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    observer.observe(&Event::RunFailed {
                        reason: mini_agent_protocol::RunFailure::Model,
                    });
                    return Err(HarnessError::Model(error));
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
            });
            final_text = response.text.clone();
            self.session.push(Message::Assistant {
                reasoning: response.reasoning,
                text: response.text,
                tool_calls: response.tool_calls.clone(),
            });

            if control.take_cancel_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Cancelled,
                    observer,
                ));
            }

            if steering_mode == SteeringMode::ContinueSameTurn
                && let Some(input) = control.take_steer_input()
            {
                if let Err(limit) = self.append_user_input(input.text) {
                    return Err(fail_limit(limit, observer));
                }
                final_text.clear();
                continue;
            }
            if control.is_steer_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Steered,
                    observer,
                ));
            }

            if response.tool_calls.is_empty() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Completed,
                    observer,
                ));
            }

            let current_executed_batch = execute_tool_batch(
                &self.tools,
                response.tool_calls,
                self.config.max_tool_output_bytes,
                &mut self.session,
                observer,
                tool_context.as_ref(),
            );

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

            if control.take_cancel_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Cancelled,
                    observer,
                ));
            }

            if steering_mode == SteeringMode::ContinueSameTurn
                && let Some(input) = control.take_steer_input()
            {
                if let Err(limit) = self.append_user_input(input.text) {
                    return Err(fail_limit(limit, observer));
                }
                final_text.clear();
                continue;
            }
            if control.is_steer_requested() {
                return Ok(finish(
                    final_text,
                    self.session.messages().to_vec(),
                    step,
                    StopReason::Steered,
                    observer,
                ));
            }

            if let Err(limit) = self.ensure_context_limit(&tool_specs) {
                return Err(fail_limit(limit, observer));
            }
        }
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
            self.compact_context(tool_specs, observer).await?;
        }
        self.ensure_context_limit(tool_specs)
            .map_err(|limit| fail_limit(limit, observer))
    }

    async fn compact_context<O: Observer + Send>(
        &mut self,
        tool_specs: &[mini_agent_protocol::ToolSpec],
        observer: &mut O,
    ) -> Result<(), HarnessError<M::Error>> {
        let before_bytes = self.context_bytes(&self.config.system_prompt, tool_specs);
        let compact_at = self.config.max_context_bytes / 2;
        let (mut prefix, context, tail) = split_compaction_parts(self.session.messages());
        if prefix.is_empty() {
            return Ok(());
        }
        observer.observe(&Event::ContextCompactionStarted { before_bytes });
        trim_prefix_to_fit(
            &mut prefix,
            COMPACTION_PROMPT,
            &self.config.system_prompt,
            &[],
            self.config.max_context_bytes,
        );
        if prefix.is_empty() {
            let compacted =
                assemble_compacted(None, context, tail, self.config.max_user_input_bytes);
            return self.finish_compacted(compacted, before_bytes, None, tool_specs, observer);
        }
        let mut compaction_messages = prefix.clone();
        compaction_messages.push(Message::User {
            text: COMPACTION_PROMPT.to_string(),
        });
        let response = match self
            .model
            .respond(
                ModelRequest {
                    system_prompt: &self.config.system_prompt,
                    messages: &compaction_messages,
                    tools: &[],
                    max_response_bytes: self.config.max_model_response_bytes,
                },
                &mut SilentModelEvents,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                observer.observe(&Event::RunFailed {
                    reason: mini_agent_protocol::RunFailure::Model,
                });
                return Err(HarnessError::Model(error));
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
        let summary = response.text.trim();
        let mut compacted = None;
        if response.tool_calls.is_empty() && !summary.is_empty() {
            let candidate = assemble_compacted(
                Some(summary),
                context.clone(),
                tail.clone(),
                self.config.max_user_input_bytes,
            );
            let after_bytes = context_bytes_for(&self.config.system_prompt, &candidate, tool_specs);
            if after_bytes < before_bytes && after_bytes <= self.config.max_context_bytes {
                compacted = Some(candidate);
            }
        }
        let compacted = compacted.unwrap_or_else(|| {
            mechanical_compact(
                prefix,
                context,
                tail,
                compact_at,
                &self.config.system_prompt,
                tool_specs,
                self.config.max_user_input_bytes,
            )
        });
        self.finish_compacted(
            compacted,
            before_bytes,
            response.usage,
            tool_specs,
            observer,
        )
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
