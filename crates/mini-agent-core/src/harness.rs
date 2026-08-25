use crate::Event;
use crate::Message;
use crate::Model;
use crate::ModelEvent;
use crate::ModelEventSink;
use crate::ModelRequest;
use crate::ModelResponse;
use crate::Observer;
use crate::ToolRegistry;
use serde::Deserialize;
use serde::Serialize;
use std::error::Error;
use std::fmt;

const TRUNCATION_MARKER: &str = "\n[truncated]";
const COMPACTION_PREFIX: &str = "[Compacted conversation context]";
const COMPACTION_PROMPT: &str = "Summarize the conversation state for another coding agent that must continue the work. Preserve the user's active goal, constraints, decisions, files changed, commands and tests already run, failures, unresolved work, and the exact next actions. Be concise but do not omit information needed to continue. Output only the summary and do not call tools.";
const COMPACT_TAIL_GROUPS: usize = 2;
const COMPACT_TAIL_MAX_BYTES: usize = 128 * 1024;

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
            max_model_response_bytes: 384 * 1024,
            max_tool_calls_per_step: 8,
            max_tool_output_bytes: 16 * 1024,
            max_context_bytes: 1024 * 1024,
            context_limit_behavior: ContextLimitBehavior::Reject,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    ContextItemBytes,
    UserInputBytes,
    ModelResponseBytes,
    ToolCallsPerStep,
    ContextBytes,
}

impl fmt::Display for LimitKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ContextItemBytes => "context item bytes",
            Self::UserInputBytes => "user input bytes",
            Self::ModelResponseBytes => "model response bytes",
            Self::ToolCallsPerStep => "tool calls per step",
            Self::ContextBytes => "context bytes",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct LimitExceeded {
    pub kind: LimitKind,
    pub limit: usize,
    pub actual: usize,
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} limit exceeded: {} > {}",
            self.kind, self.actual, self.limit
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Completed,
    StepLimit,
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
}

impl<E: fmt::Display> fmt::Display for HarnessError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => write!(formatter, "model request failed: {error}"),
            Self::Compaction(error) => write!(formatter, "context compaction failed: {error}"),
            Self::Limit(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error + 'static> Error for HarnessError<E> {}

pub struct Harness<M> {
    model: M,
    tools: ToolRegistry,
    config: HarnessConfig,
    messages: Vec<Message>,
}

impl<M: Model> Harness<M> {
    pub fn new(model: M, tools: ToolRegistry, config: HarnessConfig) -> Self {
        Self {
            model,
            tools,
            config,
            messages: Vec::new(),
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn clear_history(&mut self) {
        self.messages.clear();
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
        self.messages.push(Message::Context { text });
        Ok(())
    }

    pub fn restore_history(&mut self, messages: Vec<Message>) -> Result<(), LimitExceeded> {
        if let Some(actual) = messages.iter().find_map(|message| match message {
            Message::Context { text } if text.len() > self.config.max_context_item_bytes => {
                Some(text.len())
            }
            Message::Context { .. }
            | Message::User { .. }
            | Message::Assistant { .. }
            | Message::Tool { .. } => None,
        }) {
            return Err(LimitExceeded {
                kind: LimitKind::ContextItemBytes,
                limit: self.config.max_context_item_bytes,
                actual,
            });
        }
        let tool_specs = self.tools.specs();
        let actual = context_bytes_for(&self.config.system_prompt, &messages, &tool_specs);
        if actual > self.config.max_context_bytes {
            return Err(LimitExceeded {
                kind: LimitKind::ContextBytes,
                limit: self.config.max_context_bytes,
                actual,
            });
        }
        self.messages = messages;
        Ok(())
    }

    pub fn replace_config(&mut self, config: HarnessConfig) {
        self.config = config;
    }

    pub fn extend_tools(&mut self, tools: Vec<Box<dyn crate::Tool>>) {
        self.tools.extend(tools);
    }

    pub async fn run<O: Observer + Send>(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut O,
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

        let previous_message_count = self.messages.len();
        self.messages.push(Message::User { text: prompt });
        let tool_specs = self.tools.specs();
        if let Err(error) = self.prepare_context(&tool_specs, observer).await {
            if self.config.context_limit_behavior == ContextLimitBehavior::Reject {
                self.messages.truncate(previous_message_count);
            }
            return Err(error);
        }
        let mut final_text = String::new();
        let mut step = 0usize;

        loop {
            step = step.saturating_add(1);
            if self.config.max_steps > 0 && step > self.config.max_steps {
                return Ok(finish(
                    final_text,
                    self.messages.clone(),
                    self.config.max_steps,
                    StopReason::StepLimit,
                    observer,
                ));
            }
            self.prepare_context(&tool_specs, observer).await?;
            observer.observe(&Event::ModelStarted { step });
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
                        messages: &self.messages,
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
                        reason: crate::RunFailure::Model,
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
            self.messages.push(Message::Assistant {
                reasoning: response.reasoning,
                text: response.text,
                tool_calls: response.tool_calls.clone(),
            });

            if response.tool_calls.is_empty() {
                return Ok(finish(
                    final_text,
                    self.messages.clone(),
                    step,
                    StopReason::Completed,
                    observer,
                ));
            }

            for call in response.tool_calls {
                observer.observe(&Event::ToolStarted { call: call.clone() });
                let result = self.tools.execute(&call.name, &call.arguments);
                let (content, is_error) = match result {
                    Ok(content) => (content, false),
                    Err(error) => (error.to_string(), true),
                };
                let truncated = content.len() > self.config.max_tool_output_bytes;
                let content = truncate_utf8(content, self.config.max_tool_output_bytes);

                observer.observe(&Event::ToolFinished {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: content.clone(),
                    is_error,
                    truncated,
                });
                self.messages.push(Message::Tool {
                    call_id: call.id,
                    name: call.name,
                    content,
                    is_error,
                });
            }
            if let Err(limit) = self.ensure_context_limit(&tool_specs) {
                return Err(fail_limit(limit, observer));
            }
        }
    }

    fn ensure_context_limit(&self, tool_specs: &[crate::ToolSpec]) -> Result<(), LimitExceeded> {
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

    fn context_bytes(&self, system_prompt: &str, tool_specs: &[crate::ToolSpec]) -> usize {
        context_bytes_for(system_prompt, &self.messages, tool_specs)
    }

    async fn prepare_context<O: Observer + Send>(
        &mut self,
        tool_specs: &[crate::ToolSpec],
        observer: &mut O,
    ) -> Result<(), HarnessError<M::Error>> {
        let actual = self.context_bytes(&self.config.system_prompt, tool_specs);
        let compact_at = self.config.max_context_bytes / 2;
        let should_compact = self.config.context_limit_behavior == ContextLimitBehavior::Compact
            && self.messages.len() > 1
            && actual >= compact_at;
        if should_compact {
            self.compact_context(tool_specs, observer).await?;
        }
        self.ensure_context_limit(tool_specs)
            .map_err(|limit| fail_limit(limit, observer))
    }

    async fn compact_context<O: Observer + Send>(
        &mut self,
        tool_specs: &[crate::ToolSpec],
        observer: &mut O,
    ) -> Result<(), HarnessError<M::Error>> {
        let before_bytes = self.context_bytes(&self.config.system_prompt, tool_specs);
        let compact_at = self.config.max_context_bytes / 2;
        let (mut prefix, context, tail) = split_compaction_parts(&self.messages);
        if prefix.is_empty() {
            return Ok(());
        }
        observer.observe(&Event::ContextCompactionStarted { before_bytes });
        trim_prefix_to_fit(
            &mut prefix,
            COMPACTION_PROMPT,
            &self.config.system_prompt,
            tool_specs,
            self.config.max_context_bytes,
        );
        if prefix.is_empty() {
            let compacted = assemble_compacted(None, context, tail);
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
                    tools: tool_specs,
                    max_response_bytes: self.config.max_model_response_bytes,
                },
                &mut SilentModelEvents,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                observer.observe(&Event::RunFailed {
                    reason: crate::RunFailure::Model,
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
            let candidate = assemble_compacted(Some(summary), context.clone(), tail.clone());
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
        usage: Option<crate::ModelUsage>,
        tool_specs: &[crate::ToolSpec],
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
        self.messages = compacted;
        observer.observe(&Event::ContextCompactionFinished {
            before_bytes,
            after_bytes,
            usage,
        });
        Ok(())
    }
}

fn split_compaction_parts(messages: &[Message]) -> (Vec<Message>, Option<Message>, Vec<Message>) {
    let (without_context, context) = take_latest_context(messages);
    let (prefix, tail) = split_prefix_tail(&without_context);
    (prefix, context, tail)
}

fn take_latest_context(messages: &[Message]) -> (Vec<Message>, Option<Message>) {
    let Some(index) = messages
        .iter()
        .rposition(|message| matches!(message, Message::Context { .. }))
    else {
        return (messages.to_vec(), None);
    };
    let context = messages[index].clone();
    let mut rest = Vec::with_capacity(messages.len().saturating_sub(1));
    rest.extend_from_slice(&messages[..index]);
    rest.extend_from_slice(&messages[index + 1..]);
    (rest, Some(context))
}

fn split_prefix_tail(messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
    let starts = assistant_starts(messages);
    if starts.is_empty() {
        return (messages.to_vec(), Vec::new());
    }
    let group_count = starts.len().min(COMPACT_TAIL_GROUPS);
    let mut tail_start = starts[starts.len() - group_count];
    let mut tail = messages[tail_start..].to_vec();
    while assistant_starts(&tail).len() > 1 && serialized_len(&tail) > COMPACT_TAIL_MAX_BYTES {
        let inner = assistant_starts(&tail);
        tail_start += inner[1];
        tail = messages[tail_start..].to_vec();
    }
    (messages[..tail_start].to_vec(), tail)
}

fn assistant_starts(messages: &[Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            matches!(message, Message::Assistant { .. }).then_some(index)
        })
        .collect()
}

fn serialized_len(messages: &[Message]) -> usize {
    serde_json::to_vec(messages)
        .expect("messages must serialize")
        .len()
}

fn trim_prefix_to_fit(
    prefix: &mut Vec<Message>,
    prompt: &str,
    system_prompt: &str,
    tool_specs: &[crate::ToolSpec],
    max_bytes: usize,
) {
    while !prefix.is_empty() {
        let mut request = prefix.clone();
        request.push(Message::User {
            text: prompt.to_string(),
        });
        if context_bytes_for(system_prompt, &request, tool_specs) <= max_bytes {
            return;
        }
        prefix.remove(0);
    }
}

fn assemble_compacted(
    summary: Option<&str>,
    context: Option<Message>,
    tail: Vec<Message>,
) -> Vec<Message> {
    let mut compacted = Vec::new();
    if let Some(summary) = summary {
        compacted.push(Message::User {
            text: format!("{COMPACTION_PREFIX}\n{summary}"),
        });
    }
    if let Some(context) = context {
        compacted.push(context);
    }
    compacted.extend(tail);
    compacted
}

fn mechanical_compact(
    mut prefix: Vec<Message>,
    context: Option<Message>,
    tail: Vec<Message>,
    compact_at: usize,
    system_prompt: &str,
    tool_specs: &[crate::ToolSpec],
) -> Vec<Message> {
    loop {
        let compacted = assemble_compacted(None, context.clone(), tail.clone());
        let mut candidate = prefix.clone();
        candidate.extend(compacted.iter().cloned());
        if prefix.is_empty()
            || context_bytes_for(system_prompt, &candidate, tool_specs) < compact_at
        {
            return candidate;
        }
        prefix.remove(0);
    }
}

fn context_bytes_for(
    system_prompt: &str,
    messages: &[Message],
    tool_specs: &[crate::ToolSpec],
) -> usize {
    system_prompt.len()
        + serde_json::to_vec(messages)
            .expect("messages must serialize")
            .len()
        + serde_json::to_vec(tool_specs)
            .expect("tool specs must serialize")
            .len()
}

fn model_response_bytes(response: &ModelResponse) -> usize {
    response.reasoning.len()
        + response.text.len()
        + serde_json::to_vec(&response.tool_calls)
            .expect("tool calls must serialize")
            .len()
}

struct SilentModelEvents;

impl ModelEventSink for SilentModelEvents {
    fn emit(&mut self, _event: ModelEvent) {}
}

struct ModelEventForwarder<'a, O> {
    observer: &'a mut O,
    emitted_bytes: usize,
    max_bytes: usize,
}

impl<O: Observer> ModelEventSink for ModelEventForwarder<'_, O> {
    fn emit(&mut self, event: ModelEvent) {
        match event {
            ModelEvent::ReasoningDelta(delta) => {
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer
                        .observe(&Event::AssistantReasoningDelta { delta });
                }
            }
            ModelEvent::TextDelta(delta) => {
                self.emitted_bytes = self.emitted_bytes.saturating_add(delta.len());
                if self.emitted_bytes <= self.max_bytes {
                    self.observer.observe(&Event::AssistantTextDelta { delta });
                }
            }
        }
    }
}

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
        reason: crate::RunFailure::LimitExceeded(limit),
    });
    HarnessError::Limit(limit)
}

fn fail_compaction<E, O: Observer>(reason: String, observer: &mut O) -> HarnessError<E> {
    observer.observe(&Event::RunFailed {
        reason: crate::RunFailure::Compaction,
    });
    HarnessError::Compaction(reason)
}

fn truncate_utf8(mut content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }

    if max_bytes <= TRUNCATION_MARKER.len() {
        let end = floor_char_boundary(&content, max_bytes);
        content.truncate(end);
        return content;
    }

    let retained_bytes = max_bytes - TRUNCATION_MARKER.len();
    let head_bytes = retained_bytes.div_ceil(2);
    let tail_bytes = retained_bytes - head_bytes;
    let head_end = floor_char_boundary(&content, head_bytes);
    let tail_start = ceil_char_boundary(&content, content.len() - tail_bytes);
    let mut output = String::with_capacity(max_bytes);
    output.push_str(&content[..head_end]);
    output.push_str(TRUNCATION_MARKER);
    output.push_str(&content[tail_start..]);
    output
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
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

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tool_output_experiment.rs"]
mod tool_output_experiment;
