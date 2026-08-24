use mini_codex_core::Event;
use mini_codex_core::Observer;
use serde_json::Value;
use serde_json::json;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum StreamLane {
    #[default]
    None,
    Reasoning,
    Text,
}

struct TerminalObserver {
    lane: StreamLane,
    text_streamed: bool,
    target: OutputTarget,
}

pub struct RunObserver {
    terminal: TerminalObserver,
    trace: Option<BufWriter<std::fs::File>>,
    trace_error: Option<String>,
    stats: RunStats,
}

#[derive(Clone, Copy, Default)]
pub enum OutputTarget {
    #[default]
    Stdout,
    Stderr,
}

#[derive(Default)]
struct RunStats {
    requests: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    tool_calls: Vec<Value>,
}

impl RunObserver {
    pub fn new(trace: Option<PathBuf>) -> io::Result<Self> {
        Self::with_target(trace, OutputTarget::Stdout)
    }

    pub fn with_target(trace: Option<PathBuf>, target: OutputTarget) -> io::Result<Self> {
        let trace = trace
            .map(|path| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map(BufWriter::new)
            })
            .transpose()?;
        Ok(Self {
            terminal: TerminalObserver {
                lane: StreamLane::None,
                text_streamed: false,
                target,
            },
            trace,
            trace_error: None,
            stats: RunStats::default(),
        })
    }

    pub fn finish(&mut self) {
        self.terminal.end_stream();
        if let Some(error) = self.trace_error.take() {
            eprintln!("warning: trace stopped: {error}");
        }
    }

    pub fn stats_json(&self) -> Value {
        json!({
            "requests": self.stats.requests,
            "input_tokens": self.stats.input_tokens,
            "cached_input_tokens": self.stats.cached_input_tokens,
            "output_tokens": self.stats.output_tokens
        })
    }

    pub fn tool_calls_json(&self) -> &[Value] {
        &self.stats.tool_calls
    }
}

impl Observer for RunObserver {
    fn observe(&mut self, event: &Event) {
        self.terminal.observe(event);
        match event {
            Event::ModelResponded { usage, .. } => {
                self.stats.requests = self.stats.requests.saturating_add(1);
                if let Some(usage) = usage {
                    self.stats.input_tokens =
                        self.stats.input_tokens.saturating_add(usage.input_tokens);
                    self.stats.cached_input_tokens = self
                        .stats
                        .cached_input_tokens
                        .saturating_add(usage.cached_input_tokens);
                    self.stats.output_tokens =
                        self.stats.output_tokens.saturating_add(usage.output_tokens);
                }
            }
            Event::ToolFinished { name, is_error, .. } => {
                self.stats.tool_calls.push(json!({
                    "name": name,
                    "status": if *is_error { "error" } else { "success" }
                }));
            }
            Event::RunStarted { .. }
            | Event::ModelStarted { .. }
            | Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::ToolStarted { .. }
            | Event::ContextCompactionStarted { .. }
            | Event::ContextCompactionFinished { .. }
            | Event::RunFinished { .. }
            | Event::RunFailed { .. } => {}
        }
        if self.trace_error.is_some() {
            return;
        }
        if let Some(trace) = &mut self.trace
            && let Err(error) = serde_json::to_writer(&mut *trace, event)
                .and_then(|()| writeln!(trace).map_err(serde_json::Error::io))
                .and_then(|()| trace.flush().map_err(serde_json::Error::io))
        {
            self.trace_error = Some(error.to_string());
        }
    }
}

impl TerminalObserver {
    fn end_stream(&mut self) {
        if self.lane != StreamLane::None {
            self.target.line("");
            self.lane = StreamLane::None;
        }
    }

    fn write_delta(&mut self, lane: StreamLane, label: &str, delta: &str) {
        if self.lane != lane {
            self.end_stream();
            self.target.write(&format!("{label}> "));
            self.lane = lane;
        }
        self.target.write(delta);
        self.target.flush();
    }
}

impl OutputTarget {
    fn write(self, text: &str) {
        match self {
            Self::Stdout => print!("{text}"),
            Self::Stderr => eprint!("{text}"),
        }
    }

    fn line(self, text: &str) {
        match self {
            Self::Stdout => println!("{text}"),
            Self::Stderr => eprintln!("{text}"),
        }
    }

    fn flush(self) {
        let _ = match self {
            Self::Stdout => io::stdout().flush(),
            Self::Stderr => io::stderr().flush(),
        };
    }
}

impl Observer for TerminalObserver {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::ModelStarted { .. } => {
                self.end_stream();
                self.text_streamed = false;
            }
            Event::AssistantReasoningDelta { delta } => {
                self.write_delta(StreamLane::Reasoning, "thinking", delta);
            }
            Event::AssistantTextDelta { delta } => {
                self.text_streamed = true;
                self.write_delta(StreamLane::Text, "assistant", delta);
            }
            Event::ModelResponded { text, .. } if !text.is_empty() && !self.text_streamed => {
                self.end_stream();
                self.target.line(&format!("assistant> {text}"));
            }
            Event::ToolStarted { call } => {
                self.end_stream();
                self.target.line(&format!("tool> {}", call.name));
            }
            Event::ToolFinished {
                content, is_error, ..
            } => {
                let status = if *is_error { "error" } else { "ok" };
                self.target.line(&format!("tool[{status}]> {content}"));
            }
            Event::ContextCompactionStarted { before_bytes } => {
                self.end_stream();
                self.target
                    .line(&format!("context> compacting {before_bytes} bytes"));
            }
            Event::ContextCompactionFinished {
                before_bytes,
                after_bytes,
                ..
            } => {
                self.target.line(&format!(
                    "context> compacted {before_bytes} -> {after_bytes} bytes"
                ));
            }
            Event::RunFinished { .. } => self.end_stream(),
            Event::RunStarted { .. } | Event::ModelResponded { .. } | Event::RunFailed { .. } => {}
        }
    }
}
