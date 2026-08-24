mod ask;
mod config;
mod env_file;
mod observer;
mod openai;
mod processes;
mod project_context;
mod repl;
mod result_store;
mod workspace;

use mini_codex_core::ContextLimitBehavior;
use mini_codex_core::Harness;
use mini_codex_core::HarnessConfig;
use mini_codex_core::Message;
use mini_codex_core::Model;
use mini_codex_core::ModelEventSink;
use mini_codex_core::ModelRequest;
use mini_codex_core::ModelResponse;
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
use std::path::PathBuf;
use std::process::ExitCode;

use config::RuntimeConfig;
use observer::RunObserver;
use openai::OpenAiModel;
use workspace::ApprovalController;
use workspace::ApprovalMode;
use workspace::workspace_tools;

const HELP: &str = "mini-codex\n\nUSAGE:\n    mini-codex [--trace PATH]\n    mini-codex ask [--auto] [--json] [--trace PATH] [PROMPT]\n    mini-codex run [--trace PATH] <PROMPT>\n    mini-codex auto [--trace PATH] [PROMPT]\n    mini-codex demo [--trace PATH] <PROMPT>\n    mini-codex status [--json]\n    mini-codex doctor [--json]\n    mini-codex --version\n\n`ask` is the script-facing one-shot command and also accepts a prompt on stdin.\n`auto` without a prompt starts an interactive session in auto mode.\n\nENVIRONMENT:\n    OPENAI_API_KEY    Required except by demo\n    OPENAI_MODEL      Required except by demo\n    OPENAI_BASE_URL   Optional; defaults to https://api.openai.com/v1";
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
        Command::Ask => {
            ask::run(
                invocation.prompt,
                invocation.trace,
                invocation.json,
                invocation.automatic,
            )
            .await
        }
        Command::Auto if invocation.prompt.is_empty() => {
            run_interactive(invocation.trace, ApprovalMode::Automatic).await
        }
        Command::Auto => run_auto(invocation.prompt, invocation.trace).await,
        Command::Help => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("mini-codex {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Status => run_status(invocation.json),
        Command::Doctor => run_doctor(invocation.json),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Interactive,
    Demo,
    Run,
    Ask,
    Auto,
    Status,
    Doctor,
    Help,
    Version,
}

#[derive(Debug)]
struct Invocation {
    command: Command,
    prompt: String,
    trace: Option<PathBuf>,
    json: bool,
    automatic: bool,
}

fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let command = match args.peek().map(String::as_str) {
        None | Some("--trace") => Command::Interactive,
        Some("help" | "--help" | "-h") => {
            args.next();
            Command::Help
        }
        Some("version" | "--version" | "-V") => {
            args.next();
            Command::Version
        }
        Some("demo") => {
            args.next();
            Command::Demo
        }
        Some("run") => {
            args.next();
            Command::Run
        }
        Some("ask") => {
            args.next();
            Command::Ask
        }
        Some("auto") => {
            args.next();
            Command::Auto
        }
        Some("status") => {
            args.next();
            Command::Status
        }
        Some("doctor") => {
            args.next();
            Command::Doctor
        }
        Some(other) => return Err(format!("unknown command: {other}")),
    };
    if matches!(command, Command::Help | Command::Version) {
        return Ok(Invocation {
            command,
            prompt: String::new(),
            trace: None,
            json: false,
            automatic: false,
        });
    }

    let mut prompt = Vec::new();
    let mut trace = None;
    let mut json = false;
    let mut automatic = false;
    while let Some(argument) = args.next() {
        if argument == "--trace" {
            if trace.is_some() {
                return Err("--trace may be provided only once".to_string());
            }
            trace = Some(PathBuf::from(
                args.next()
                    .ok_or_else(|| "--trace requires a path".to_string())?,
            ));
        } else if argument == "--json" {
            if json {
                return Err("--json may be provided only once".to_string());
            }
            json = true;
        } else if argument == "--auto" {
            if automatic {
                return Err("--auto may be provided only once".to_string());
            }
            automatic = true;
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
    if matches!(command, Command::Status | Command::Doctor) && !prompt.is_empty() {
        return Err("this command does not accept positional arguments".to_string());
    }
    if json && !matches!(command, Command::Ask | Command::Status | Command::Doctor) {
        return Err("--json is supported only by ask, status, and doctor".to_string());
    }
    if automatic && command != Command::Ask {
        return Err("--auto is supported only by ask".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        trace,
        json,
        automatic,
    })
}

fn run_status(json: bool) -> ExitCode {
    let config = match RuntimeConfig::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            if json {
                println!("{}", json!({"error": error}));
            }
            return ExitCode::from(2);
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&config.status_json())
                .expect("status must be serializable")
        );
    } else {
        for line in config.status_lines() {
            println!("{line}");
        }
    }
    ExitCode::SUCCESS
}

fn run_doctor(json: bool) -> ExitCode {
    let config = match RuntimeConfig::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            if json {
                println!("{}", json!({"ok": false, "checks": [], "error": error}));
            }
            return ExitCode::from(2);
        }
    };
    let report = config.doctor();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.json).expect("doctor report must be serializable")
        );
    } else {
        for line in report.lines {
            println!("{line}");
        }
    }
    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
    let runtime_config = RuntimeConfig::load()?;
    build_openai_harness_with(&runtime_config, approval, config)
}

fn build_openai_harness_with(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
) -> Result<Harness<OpenAiModel>, String> {
    let provider = runtime_config.provider_settings()?;
    let model = match OpenAiModel::new(provider.api_key, provider.model, provider.base_url) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
    let workspace = runtime_config.workspace();
    config.system_prompt =
        project_context::augment_system_prompt(&config.system_prompt, &workspace)?;
    let tools = match workspace_tools(workspace, approval) {
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
    fn parses_script_ask_options() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--auto".to_string(),
            "--json".to_string(),
            "inspect".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "inspect");
        assert!(invocation.automatic);
        assert!(invocation.json);
    }

    #[test]
    fn parses_version_command() {
        let invocation = parse_args(vec!["--version".to_string()]).unwrap();

        assert_eq!(invocation.command, Command::Version);
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
