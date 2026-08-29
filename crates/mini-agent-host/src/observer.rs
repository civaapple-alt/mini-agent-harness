use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::Observer;
use mini_agent_protocol::ToolCall;
use serde_json::Value;
use serde_json::json;
use std::io;
use std::io::IsTerminal;
use std::io::Write;

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

const MAX_TOOL_DETAIL_BYTES: usize = 512;

#[derive(Default)]
struct RunStats {
    requests: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    tool_calls: Vec<Value>,
}

impl Default for RunObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl RunObserver {
    pub fn new() -> Self {
        Self::with_terminal(
            OutputTarget::Stdout,
            AssistantDisplay::Stream {
                target: OutputTarget::Stdout,
                color: OutputTarget::Stdout.color_enabled(),
            },
        )
    }

    pub fn for_script(format: ScriptFormat) -> Self {
        let terminal = io::stdout().is_terminal();
        Self::with_terminal(
            OutputTarget::Stderr,
            script_assistant_display(format, terminal, OutputTarget::Stdout.color_enabled()),
        )
    }

    fn with_terminal(target: OutputTarget, assistant: AssistantDisplay) -> Self {
        Self {
            terminal: TerminalObserver {
                lane: StreamLane::None,
                lane_target: target,
                assistant_displayed: false,
                target,
                assistant,
                color: target.color_enabled(),
            },
            stats: RunStats::default(),
        }
    }

    pub fn finish(&mut self) {
        self.terminal.end_stream();
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
        self.observe_event(event);
    }
}

impl EventSink for RunObserver {
    fn emit(&mut self, event: EventEnvelope) {
        self.observe_event(&event.event);
    }
}

impl RunObserver {
    fn observe_event(&mut self, event: &Event) {
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
            Event::TurnStarted { .. }
            | Event::TurnFinished { .. }
            | Event::RunStarted { .. }
            | Event::ModelStarted { .. }
            | Event::AssistantReasoningDelta { .. }
            | Event::AssistantTextDelta { .. }
            | Event::ToolStarted { .. }
            | Event::ContextCompactionStarted { .. }
            | Event::ContextCompactionFinished { .. }
            | Event::RunFinished { .. }
            | Event::RunFailed { .. } => {}
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

fn format_tool_started(call: &ToolCall, color: bool) -> String {
    let tag = styled_tag("tool>", TagColor::Yellow, color);
    match tool_detail(call) {
        Some(detail) => format!("{tag} {} — {detail}", call.name),
        None => format!("{tag} {}", call.name),
    }
}

fn format_tool_finished(name: &str, content: &str, is_error: bool, color: bool) -> String {
    let (tag, tag_color) = if is_error {
        ("tool[error]>", TagColor::Red)
    } else {
        ("tool[ok]>", TagColor::Green)
    };
    let tag = styled_tag(tag, tag_color, color);
    if content.is_empty() {
        return tag;
    }
    if shows_full_tool_output(name) {
        format!("{tag} {content}")
    } else {
        format!(
            "{tag} {}",
            bounded_single_line(content, MAX_TOOL_DETAIL_BYTES)
        )
    }
}

fn shows_full_tool_output(name: &str) -> bool {
    matches!(name, "shell" | "process_read" | "read_tool_result")
}

fn arg_str<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

fn tool_detail(call: &ToolCall) -> Option<String> {
    match call.name.as_str() {
        "shell" | "process_start" => Some(bounded_single_line(
            arg_str(&call.arguments, "command")?,
            MAX_TOOL_DETAIL_BYTES,
        )),
        "read_file" | "edit_file" | "write_file" | "read_image" => Some(bounded_single_line(
            arg_str(&call.arguments, "path")?,
            MAX_TOOL_DETAIL_BYTES,
        )),
        "web_fetch" => Some(bounded_single_line(
            arg_str(&call.arguments, "url")?,
            MAX_TOOL_DETAIL_BYTES,
        )),
        "process_read" | "process_write" | "process_stop" => Some(bounded_single_line(
            arg_str(&call.arguments, "process_id")?,
            MAX_TOOL_DETAIL_BYTES,
        )),
        "read_tool_result" => Some(bounded_single_line(
            arg_str(&call.arguments, "handle")?,
            MAX_TOOL_DETAIL_BYTES,
        )),
        _ => None,
    }
}

fn bounded_single_line(value: &str, max_bytes: usize) -> String {
    let mut output = String::with_capacity(value.len().min(max_bytes));
    for character in value.chars() {
        let escaped = match character {
            '\r' => "\\r".to_string(),
            '\n' => "\\n".to_string(),
            '\t' => "\\t".to_string(),
            character if character.is_control() => format!("\\u{{{:x}}}", character as u32),
            character => character.to_string(),
        };
        if output.len().saturating_add(escaped.len()) > max_bytes.saturating_sub('…'.len_utf8()) {
            output.push('…');
            break;
        }
        output.push_str(&escaped);
    }
    output
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
                self.target.line(&format_tool_started(call, self.color));
            }
            Event::ToolFinished {
                name,
                content,
                is_error,
                ..
            } => {
                self.end_stream();
                self.target
                    .line(&format_tool_finished(name, content, *is_error, self.color));
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
            Event::TurnStarted { .. }
            | Event::TurnFinished { .. }
            | Event::RunStarted { .. }
            | Event::ModelResponded { .. }
            | Event::RunFailed { .. } => {}
        }
    }
}

#[cfg(test)]
#[path = "observer_tests.rs"]
mod tests;
