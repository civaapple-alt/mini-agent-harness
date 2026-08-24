use mini_codex_core::Event;
use mini_codex_core::Observer;
use serde_json::Value;
use serde_json::json;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::IsTerminal;
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
    lane_target: OutputTarget,
    assistant_displayed: bool,
    target: OutputTarget,
    assistant: AssistantDisplay,
    color: bool,
}

pub struct RunObserver {
    terminal: TerminalObserver,
    trace: Option<BufWriter<std::fs::File>>,
    trace_error: Option<String>,
    stats: RunStats,
}

#[derive(Clone, Copy, Default)]
enum OutputTarget {
    #[default]
    Stdout,
    Stderr,
}

#[derive(Clone, Copy)]
enum AssistantDisplay {
    Hidden,
    Stream { target: OutputTarget, color: bool },
}

#[derive(Clone, Copy)]
pub enum ScriptFormat {
    Text,
    Json,
}

#[derive(Clone, Copy)]
enum TagColor {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
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
        Self::with_terminal(
            trace,
            OutputTarget::Stdout,
            AssistantDisplay::Stream {
                target: OutputTarget::Stdout,
                color: OutputTarget::Stdout.color_enabled(),
            },
        )
    }

    pub fn for_script(trace: Option<PathBuf>, format: ScriptFormat) -> io::Result<Self> {
        let terminal = io::stdout().is_terminal();
        Self::with_terminal(
            trace,
            OutputTarget::Stderr,
            script_assistant_display(format, terminal, OutputTarget::Stdout.color_enabled()),
        )
    }

    fn with_terminal(
        trace: Option<PathBuf>,
        target: OutputTarget,
        assistant: AssistantDisplay,
    ) -> io::Result<Self> {
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
                lane_target: target,
                assistant_displayed: false,
                target,
                assistant,
                color: target.color_enabled(),
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

    pub fn assistant_displayed(&self) -> bool {
        self.terminal.assistant_displayed
    }
}

pub fn print_final_answer(text: &str) {
    let terminal = io::stdout().is_terminal();
    println!(
        "{}",
        format_final_answer(
            text,
            terminal,
            terminal && OutputTarget::Stdout.color_enabled()
        )
    );
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
            self.lane_target.line("");
            self.lane = StreamLane::None;
        }
    }

    fn write_delta(
        &mut self,
        lane: StreamLane,
        target: OutputTarget,
        tag: &str,
        color: TagColor,
        color_enabled: bool,
        delta: &str,
    ) {
        if self.lane != lane {
            self.end_stream();
            target.write(&styled_tag(tag, color, color_enabled));
            target.write(" ");
            self.lane = lane;
            self.lane_target = target;
        }
        target.write(delta);
        target.flush();
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

    fn color_enabled(self) -> bool {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        match self {
            Self::Stdout => io::stdout().is_terminal(),
            Self::Stderr => io::stderr().is_terminal(),
        }
    }
}

fn styled_tag(tag: &str, color: TagColor, enabled: bool) -> String {
    if !enabled {
        return tag.to_string();
    }
    let code = match color {
        TagColor::Red => 31,
        TagColor::Green => 32,
        TagColor::Yellow => 33,
        TagColor::Blue => 34,
        TagColor::Magenta => 35,
        TagColor::Cyan => 36,
    };
    format!("\u{1b}[{code}m{tag}\u{1b}[0m")
}

fn format_final_answer(text: &str, terminal: bool, color: bool) -> String {
    if terminal {
        format!("{} {text}", styled_tag("assistant>", TagColor::Blue, color))
    } else {
        text.to_string()
    }
}

fn script_assistant_display(format: ScriptFormat, terminal: bool, color: bool) -> AssistantDisplay {
    match (format, terminal) {
        (ScriptFormat::Text, true) => AssistantDisplay::Stream {
            target: OutputTarget::Stdout,
            color,
        },
        (ScriptFormat::Text, false) | (ScriptFormat::Json, _) => AssistantDisplay::Hidden,
    }
}

impl Observer for TerminalObserver {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::ModelStarted { .. } => {
                self.end_stream();
                self.assistant_displayed = false;
            }
            Event::AssistantReasoningDelta { delta } => {
                self.write_delta(
                    StreamLane::Reasoning,
                    self.target,
                    "thinking>",
                    TagColor::Magenta,
                    self.color,
                    delta,
                );
            }
            Event::AssistantTextDelta { delta } => {
                if let AssistantDisplay::Stream { target, color } = self.assistant {
                    self.assistant_displayed = true;
                    self.write_delta(
                        StreamLane::Text,
                        target,
                        "assistant>",
                        TagColor::Blue,
                        color,
                        delta,
                    );
                }
            }
            Event::ModelResponded { text, .. } if !text.is_empty() && !self.assistant_displayed => {
                if let AssistantDisplay::Stream { target, color } = self.assistant {
                    self.end_stream();
                    target.line(&format!(
                        "{} {text}",
                        styled_tag("assistant>", TagColor::Blue, color)
                    ));
                    self.assistant_displayed = true;
                }
            }
            Event::ToolStarted { call } => {
                self.end_stream();
                self.target.line(&format!(
                    "{} {}",
                    styled_tag("tool>", TagColor::Yellow, self.color),
                    call.name
                ));
            }
            Event::ToolFinished {
                content, is_error, ..
            } => {
                let (tag, color) = if *is_error {
                    ("tool[error]>", TagColor::Red)
                } else {
                    ("tool[ok]>", TagColor::Green)
                };
                self.target
                    .line(&format!("{} {content}", styled_tag(tag, color, self.color)));
            }
            Event::ContextCompactionStarted { before_bytes } => {
                self.end_stream();
                self.target.line(&format!(
                    "{} compacting {before_bytes} bytes",
                    styled_tag("context>", TagColor::Cyan, self.color)
                ));
            }
            Event::ContextCompactionFinished {
                before_bytes,
                after_bytes,
                ..
            } => {
                self.target.line(&format!(
                    "{} compacted {before_bytes} -> {after_bytes} bytes",
                    styled_tag("context>", TagColor::Cyan, self.color)
                ));
            }
            Event::RunFinished { .. } => self.end_stream(),
            Event::RunStarted { .. } | Event::ModelResponded { .. } | Event::RunFailed { .. } => {}
        }
    }
}

#[cfg(test)]
#[path = "observer_tests.rs"]
mod tests;
