mod env_file;
mod openai;
mod processes;
mod repl;
mod result_store;
mod workspace;

use mini_codex_core::ContextLimitBehavior;
use mini_codex_core::Event;
use mini_codex_core::Harness;
use mini_codex_core::HarnessConfig;
use mini_codex_core::Message;
use mini_codex_core::Model;
use mini_codex_core::ModelEventSink;
use mini_codex_core::ModelRequest;
use mini_codex_core::ModelResponse;
use mini_codex_core::Observer;
use mini_codex_core::StopReason;
use mini_codex_core::Tool;
use mini_codex_core::ToolCall;
use mini_codex_core::ToolError;
use mini_codex_core::ToolRegistry;
use mini_codex_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::convert::Infallible;
use std::env;
use std::fs::OpenOptions;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use env_file::Environment;
use openai::OpenAiModel;
use workspace::ApprovalController;
use workspace::ApprovalMode;
use workspace::workspace_tools;

const HELP: &str = "mini-codex\n\nUSAGE:\n    mini-codex [--trace PATH]\n    mini-codex run [--trace PATH] <PROMPT>\n    mini-codex auto [--trace PATH] [PROMPT]\n    mini-codex demo [--trace PATH] <PROMPT>\n\n`auto` without a prompt starts an interactive session in auto mode.\n\nENVIRONMENT:\n    OPENAI_API_KEY    Required except by demo\n    OPENAI_MODEL      Required except by demo\n    OPENAI_BASE_URL   Optional; defaults to https://api.openai.com/v1";
const AUTO_SYSTEM_PROMPT: &str = "You are an autonomous coding agent. Work continuously toward the user's goal. Inspect the workspace before editing, use tools as needed, keep changes scoped to the request, and run relevant checks. Do not stop at intermediate progress or ask for confirmation unless you are blocked by missing information or an unsafe action outside the workspace. When the work is complete, report the result plainly.";
const AUTO_MAX_STEPS: usize = 128;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let invocation = match parse_args(env::args().skip(1).collect()) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    match invocation.command {
        Command::Interactive => run_interactive(invocation.trace, ApprovalMode::Interactive).await,
        Command::Demo => run_demo(invocation.prompt, invocation.trace).await,
        Command::Run => run_openai(invocation.prompt, invocation.trace).await,
        Command::Auto if invocation.prompt.is_empty() => {
            run_interactive(invocation.trace, ApprovalMode::Automatic).await
        }
        Command::Auto => run_auto(invocation.prompt, invocation.trace).await,
        Command::Help => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Interactive,
    Demo,
    Run,
    Auto,
    Help,
}

#[derive(Debug)]
struct Invocation {
    command: Command,
    prompt: String,
    trace: Option<PathBuf>,
}

fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let command = match args.peek().map(String::as_str) {
        None | Some("--trace") => Command::Interactive,
        Some("help" | "--help" | "-h") => {
            args.next();
            Command::Help
        }
        Some("demo") => {
            args.next();
            Command::Demo
        }
        Some("run") => {
            args.next();
            Command::Run
        }
        Some("auto") => {
            args.next();
            Command::Auto
        }
        Some(other) => return Err(format!("unknown command: {other}")),
    };
    if matches!(command, Command::Help) {
        return Ok(Invocation {
            command,
            prompt: String::new(),
            trace: None,
        });
    }

    let mut prompt = Vec::new();
    let mut trace = None;
    while let Some(argument) = args.next() {
        if argument == "--trace" {
            if trace.is_some() {
                return Err("--trace may be provided only once".to_string());
            }
            trace = Some(PathBuf::from(
                args.next()
                    .ok_or_else(|| "--trace requires a path".to_string())?,
            ));
        } else if argument.starts_with('-') {
            return Err(format!("unknown option: {argument}"));
        } else {
            prompt.push(argument);
        }
    }
    if matches!(command, Command::Interactive) && !prompt.is_empty() {
        return Err("interactive mode does not accept a prompt; use `run`".to_string());
    }
    if matches!(command, Command::Demo | Command::Run) && prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        trace,
    })
}

async fn run_interactive(trace: Option<PathBuf>, initial_mode: ApprovalMode) -> ExitCode {
    repl::run(trace, initial_mode).await
}

async fn run_demo(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    let model = DemoModel { turn: 0 };
    let tools = ToolRegistry::new(vec![Box::new(Uppercase)]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());

    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return ExitCode::FAILURE;
        }
    };
    match harness.run(prompt, &mut observer).await {
        Ok(_) => {
            observer.finish();
            ExitCode::SUCCESS
        }
        Err(error) => {
            observer.finish();
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_openai(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    let approval = ApprovalController::new(ApprovalMode::Interactive);
    let mut harness = match build_openai_harness(approval, HarnessConfig::default()) {
        Ok(harness) => harness,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return ExitCode::FAILURE;
        }
    };

    match harness.run(prompt, &mut observer).await {
        Ok(_) => {
            observer.finish();
            ExitCode::SUCCESS
        }
        Err(error) => {
            observer.finish();
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_auto(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    print_auto_warning();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    let mut harness = match build_openai_harness(approval, harness_config(ApprovalMode::Automatic))
    {
        Ok(harness) => harness,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return ExitCode::FAILURE;
        }
    };

    match harness.run(prompt, &mut observer).await {
        Ok(outcome) => {
            observer.finish();
            if outcome.stop_reason == StopReason::StepLimit {
                eprintln!(
                    "error: auto mode stopped after {} model steps without completing",
                    outcome.steps
                );
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            observer.finish();
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_openai_harness(
    approval: ApprovalController,
    config: HarnessConfig,
) -> Result<Harness<OpenAiModel>, String> {
    let environment = Environment::load(".env")?;
    let api_key = environment.required("OPENAI_API_KEY")?;
    let model = environment.required("OPENAI_MODEL")?;
    let base_url = environment
        .get("OPENAI_BASE_URL")
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let model = match OpenAiModel::new(api_key, model, base_url) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
    let root = match env::current_dir() {
        Ok(root) => root,
        Err(error) => return Err(format!("cannot resolve current directory: {error}")),
    };
    let tools = match workspace_tools(root, approval) {
        Ok(tools) => ToolRegistry::new(tools),
        Err(error) => return Err(error.to_string()),
    };
    Ok(Harness::new(model, tools, config))
}

fn harness_config(mode: ApprovalMode) -> HarnessConfig {
    match mode {
        ApprovalMode::Interactive => HarnessConfig::default(),
        ApprovalMode::Automatic => HarnessConfig {
            system_prompt: AUTO_SYSTEM_PROMPT.to_string(),
            max_steps: AUTO_MAX_STEPS,
            context_limit_behavior: ContextLimitBehavior::Compact,
            ..HarnessConfig::default()
        },
    }
}

fn print_auto_warning() {
    eprintln!(
        "warning: auto mode runs workspace writes and unsandboxed shell commands without approval"
    );
}

struct DemoModel {
    turn: usize,
}

impl Model for DemoModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.turn += 1;
        if self.turn == 1 {
            let prompt = request
                .messages
                .iter()
                .find_map(|message| match message {
                    Message::User { text } => Some(text.as_str()),
                    Message::Assistant { .. } | Message::Tool { .. } => None,
                })
                .unwrap_or_default();
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "I will run one tool.".to_string(),
                tool_calls: vec![ToolCall {
                    id: "demo-call".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({ "text": prompt }),
                }],
                usage: None,
            });
        }

        let result = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Tool { content, .. } => Some(content.as_str()),
                Message::User { .. } | Message::Assistant { .. } => None,
            })
            .unwrap_or("no tool result");
        Ok(ModelResponse {
            reasoning: String::new(),
            text: format!("The tool returned: {result}"),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

struct Uppercase;

impl Tool for Uppercase {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "uppercase".to_string(),
            description: "Convert text to uppercase".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        arguments
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_uppercase)
            .ok_or_else(|| ToolError("text must be a string".to_string()))
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum StreamLane {
    #[default]
    None,
    Reasoning,
    Text,
}

#[derive(Default)]
struct TerminalObserver {
    lane: StreamLane,
    text_streamed: bool,
}

struct RunObserver {
    terminal: TerminalObserver,
    trace: Option<BufWriter<std::fs::File>>,
    trace_error: Option<String>,
}

impl RunObserver {
    fn new(trace: Option<PathBuf>) -> io::Result<Self> {
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
            terminal: TerminalObserver::default(),
            trace,
            trace_error: None,
        })
    }

    fn finish(&mut self) {
        self.terminal.end_stream();
        if let Some(error) = self.trace_error.take() {
            eprintln!("warning: trace stopped: {error}");
        }
    }
}

impl Observer for RunObserver {
    fn observe(&mut self, event: &Event) {
        self.terminal.observe(event);
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
            println!();
            self.lane = StreamLane::None;
        }
    }

    fn write_delta(&mut self, lane: StreamLane, label: &str, delta: &str) {
        if self.lane != lane {
            self.end_stream();
            print!("{label}> ");
            self.lane = lane;
        }
        print!("{delta}");
        let _ = io::stdout().flush();
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
                println!("assistant> {text}");
            }
            Event::ToolStarted { call } => {
                self.end_stream();
                println!("tool> {}", call.name);
            }
            Event::ToolFinished {
                content, is_error, ..
            } => {
                let status = if *is_error { "error" } else { "ok" };
                println!("tool[{status}]> {content}");
            }
            Event::ContextCompactionStarted { before_bytes } => {
                self.end_stream();
                println!("context> compacting {before_bytes} bytes");
            }
            Event::ContextCompactionFinished {
                before_bytes,
                after_bytes,
                ..
            } => {
                println!("context> compacted {before_bytes} -> {after_bytes} bytes");
            }
            Event::RunFinished { .. } => self.end_stream(),
            Event::RunStarted { .. } | Event::ModelResponded { .. } | Event::RunFailed { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_interactive_mode() {
        let invocation = parse_args(Vec::new()).unwrap();

        assert_eq!(invocation.command, Command::Interactive);
        assert_eq!(invocation.prompt, "");
        assert_eq!(invocation.trace, None);
    }

    #[test]
    fn accepts_interactive_trace() {
        let invocation =
            parse_args(vec!["--trace".to_string(), "events.jsonl".to_string()]).unwrap();

        assert_eq!(invocation.command, Command::Interactive);
        assert_eq!(invocation.trace, Some(PathBuf::from("events.jsonl")));
    }

    #[test]
    fn joins_one_shot_prompt() {
        let invocation = parse_args(vec![
            "run".to_string(),
            "inspect".to_string(),
            "this".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Run);
        assert_eq!(invocation.prompt, "inspect this");
    }

    #[test]
    fn one_shot_mode_requires_prompt() {
        assert_eq!(
            parse_args(vec!["run".to_string()]).unwrap_err(),
            "prompt is required"
        );
    }

    #[test]
    fn parses_auto_mode() {
        let invocation = parse_args(vec![
            "auto".to_string(),
            "finish".to_string(),
            "the task".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Auto);
        assert_eq!(invocation.prompt, "finish the task");
    }

    #[test]
    fn parses_auto_mode_without_prompt() {
        let invocation = parse_args(vec!["auto".to_string()]).unwrap();

        assert_eq!(invocation.command, Command::Auto);
        assert_eq!(invocation.prompt, "");
    }
}
