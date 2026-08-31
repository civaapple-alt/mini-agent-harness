use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use serde::Deserialize;
use serde::Serialize;
use std::io;
use std::io::Write;

const MAX_TRACE_ID_BYTES: usize = 128;
const MAX_TRACE_RECORD_BYTES: usize = 8 * 1024;

/// A redacted, bounded record for comparing one local harness execution.
///
/// Event payloads are never written. Their byte count and stable digest are
/// retained so a report can prove that the event stream changed without
/// copying prompts, tool arguments, or tool results into the trace artifact.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct TraceRecord {
    pub trace_id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub round_index: usize,
    pub event: String,
    pub input_bytes: Option<usize>,
    pub input_hash: Option<String>,
    pub tool_manifest_hash: Option<String>,
    pub output_bytes: Option<usize>,
    pub payload_bytes: usize,
    pub payload_hash: String,
}

/// Writes the local App Server event stream as bounded JSONL diagnostics.
///
/// This is an internal local-client utility, not a replacement wire protocol.
/// The writer receives only event envelopes, so each record is append-only and
/// does not retain raw model or tool content.
pub struct JsonlTrace<W> {
    writer: W,
    trace_id: String,
    input_bytes: Option<usize>,
    input_hash: Option<String>,
    tool_manifest_hash: Option<String>,
    round_index: usize,
    error: Option<String>,
}

impl<W: Write> JsonlTrace<W> {
    pub fn new(trace_id: impl Into<String>, writer: W) -> io::Result<Self> {
        let trace_id = trace_id.into();
        if trace_id.is_empty() || trace_id.len() > MAX_TRACE_ID_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "trace id must be between 1 and 128 bytes",
            ));
        }
        Ok(Self {
            writer,
            trace_id,
            input_bytes: None,
            input_hash: None,
            tool_manifest_hash: None,
            round_index: 0,
            error: None,
        })
    }

    pub fn finish(mut self) -> io::Result<W> {
        if let Some(error) = self.error {
            return Err(io::Error::other(error));
        }
        self.writer.flush()?;
        Ok(self.writer)
    }

    fn record(&mut self, envelope: &EventEnvelope) {
        if self.error.is_some() {
            return;
        }
        if matches!(&envelope.event, Event::TurnStarted { .. }) {
            self.input_bytes = None;
            self.input_hash = None;
            self.tool_manifest_hash = None;
            self.round_index = 0;
        }
        if let Event::ModelStarted {
            input_bytes,
            input_hash,
            tool_manifest_hash,
            ..
        } = &envelope.event
        {
            self.round_index = self.round_index.saturating_add(1);
            self.input_bytes = Some(*input_bytes);
            self.input_hash = Some(input_hash.clone());
            self.tool_manifest_hash = Some(tool_manifest_hash.clone());
        }
        let payload = serde_json::to_vec(&envelope.event).expect("event must serialize");
        let record = TraceRecord {
            trace_id: self.trace_id.clone(),
            thread_id: envelope.thread_id.as_str().to_string(),
            turn_id: envelope.turn_id.as_ref().map(|id| id.as_str().to_string()),
            sequence: envelope.sequence,
            round_index: self.round_index,
            event: event_name(&envelope.event).to_string(),
            input_bytes: self.input_bytes,
            input_hash: self.input_hash.clone(),
            tool_manifest_hash: self.tool_manifest_hash.clone(),
            output_bytes: output_bytes(&envelope.event),
            payload_bytes: payload.len(),
            payload_hash: mini_agent_protocol::stable_digest(&payload),
        };
        let mut line = serde_json::to_vec(&record).expect("trace record must serialize");
        line.push(b'\n');
        if line.len() > MAX_TRACE_RECORD_BYTES {
            self.error = Some("trace record exceeded 8 KiB".to_string());
        } else if let Err(error) = self.writer.write_all(&line) {
            self.error = Some(error.to_string());
        }
    }
}

impl<W: Write> EventSink for JsonlTrace<W> {
    fn emit(&mut self, event: EventEnvelope) {
        self.record(&event);
    }
}

fn event_name(event: &Event) -> &'static str {
    match event {
        Event::TurnStarted { .. } => "turn_started",
        Event::RunStarted { .. } => "run_started",
        Event::ModelStarted { .. } => "model_started",
        Event::AssistantReasoningDelta { .. } => "assistant_reasoning_delta",
        Event::AssistantTextDelta { .. } => "assistant_text_delta",
        Event::ModelResponded { .. } => "model_responded",
        Event::ToolStarted { .. } => "tool_started",
        Event::ToolFinished { .. } => "tool_finished",
        Event::ContextCompactionStarted { .. } => "context_compaction_started",
        Event::ContextCompactionFinished { .. } => "context_compaction_finished",
        Event::RunFinished { .. } => "run_finished",
        Event::TurnFinished { .. } => "turn_finished",
        Event::RunFailed { .. } => "run_failed",
    }
}

fn output_bytes(event: &Event) -> Option<usize> {
    match event {
        Event::AssistantReasoningDelta { delta } | Event::AssistantTextDelta { delta } => {
            Some(delta.len())
        }
        Event::ModelResponded {
            reasoning,
            text,
            tool_calls,
            ..
        } => Some(
            reasoning.len()
                + text.len()
                + serde_json::to_vec(tool_calls)
                    .expect("tool calls must serialize")
                    .len(),
        ),
        Event::ToolFinished { content, .. } => Some(content.len()),
        Event::ToolStarted { .. }
        | Event::TurnStarted { .. }
        | Event::RunStarted { .. }
        | Event::ModelStarted { .. }
        | Event::ContextCompactionStarted { .. }
        | Event::ContextCompactionFinished { .. }
        | Event::RunFinished { .. }
        | Event::TurnFinished { .. }
        | Event::RunFailed { .. } => None,
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
