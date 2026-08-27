mod args;
mod ask;
mod config;
mod env_file;
mod harness_builder;
mod marketplaces;
mod mcp;
mod mentor;
mod observer;
mod openai;
mod processes;
mod project_context;
mod repl;
mod result_store;
mod sandbox;
mod security;
mod session;
mod skills;
mod trace;
mod workspace;
mod world;

use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::RunOutcome;
use mini_agent_core::StopReason;
use mini_agent_core::Tool;
use mini_agent_core::ToolCall;
use mini_agent_core::ToolError;
use mini_agent_core::ToolRegistry;
use mini_agent_core::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::convert::Infallible;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use args::Command;
use args::HelpTopic;
use args::help_text;
use args::parse_args;
use config::RuntimeConfig;
pub(crate) use harness_builder::HarnessBuild;
pub(crate) use harness_builder::harness_config;
pub(crate) use harness_builder::harness_config_auto;
pub(crate) use harness_builder::prepare_openai_harness;
pub(crate) use harness_builder::print_auto_warning;
use observer::RunObserver;
use session::SessionRequest;
use workspace::ApprovalController;
use workspace::ApprovalMode;

pub(crate) fn version_line() -> String {
    format!("mini-agent {} ({})", env!("CARGO_PKG_VERSION"), git_sha())
}

pub(crate) fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("unknown")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let invocation = match parse_args(std::env::args().skip(1).collect()) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("error: {error}\n");
            println!("{}", help_text(HelpTopic::Root));
            return ExitCode::from(2);
        }
    };
    match invocation.command {
        Command::Interactive => {
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else {
                SessionRequest::New
            };
            repl::run(
                invocation.trace,
                ApprovalMode::Automatic,
                false,
                request,
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
            )
            .await
        }
        Command::Demo => run_demo(invocation.prompt, invocation.trace).await,
        Command::Run | Command::Ask => {
            ask::run(
                invocation.prompt,
                invocation.trace,
                invocation.json,
                invocation.automatic,
                invocation.security_preset,
                invocation.web_search,
            )
            .await
        }
        Command::Auto if invocation.prompt.is_empty() => {
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else {
                SessionRequest::New
            };
            repl::run(
                invocation.trace,
                ApprovalMode::Automatic,
                true,
                request,
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
            )
            .await
        }
        Command::Auto => run_auto(invocation.prompt, invocation.trace, invocation.web_search).await,
        Command::Help => {
            println!("{}", help_text(invocation.help_topic));
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        Command::Status => run_status(invocation.json),
        Command::Doctor => run_doctor(invocation.json),
        Command::Resume => {
            repl::run(
                invocation.trace,
                ApprovalMode::Automatic,
                false,
                SessionRequest::Resume(invocation.prompt),
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
            )
            .await
        }
        Command::Fork => {
            repl::run(
                invocation.trace,
                ApprovalMode::Automatic,
                false,
                SessionRequest::Fork(invocation.prompt),
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
            )
            .await
        }
        Command::Sessions => run_sessions(),
        Command::Mentor => mentor::run(invocation.prompt, invocation.trace, invocation.json).await,
        Command::TraceReplay => {
            trace::replay(std::path::Path::new(&invocation.prompt), invocation.json)
        }
        Command::TraceSummary => {
            trace::summary(std::path::Path::new(&invocation.prompt), invocation.json)
        }
    }
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

fn run_sessions() -> ExitCode {
    let workspace = match env::current_dir() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("error: cannot resolve current directory: {error}");
            return ExitCode::FAILURE;
        }
    };
    match session::list(&workspace) {
        Ok(sessions) if sessions.is_empty() => {
            println!("no durable sessions");
            ExitCode::SUCCESS
        }
        Ok(sessions) => {
            for session in sessions {
                println!("{} ({} bytes)", session.id, session.bytes);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_demo(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    let model = DemoModel { turn: 0 };
    let tools = ToolRegistry::new(vec![Box::new(Uppercase)]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    match run_with_observer(&mut harness, prompt, trace).await {
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

async fn run_auto(
    prompt: String,
    trace: Option<PathBuf>,
    web_search_override: Option<bool>,
) -> ExitCode {
    print_auto_warning();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    let mut runtime = match RuntimeConfig::load() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    if let Some(enabled) = web_search_override {
        runtime = runtime.with_web_search(enabled);
    }
    let mut harness = match prepare_openai_harness(
        &runtime,
        approval,
        harness_config_auto(true, runtime.copilot_max_steps()),
    ) {
        Ok(build) => build.harness,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    match run_with_observer(&mut harness, prompt, trace).await {
        Ok(outcome) if outcome.stop_reason == StopReason::StepLimit => {
            eprintln!(
                "error: auto mode stopped after {} model steps without completing",
                outcome.steps
            );
            ExitCode::FAILURE
        }
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

async fn run_with_observer<M: Model>(
    harness: &mut Harness<M>,
    prompt: String,
    trace: Option<PathBuf>,
) -> Result<RunOutcome, ExitCode> {
    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return Err(ExitCode::FAILURE);
        }
    };
    let result = harness.run(prompt, &mut observer).await;
    observer.finish();
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            eprintln!("error: {error}");
            Err(ExitCode::FAILURE)
        }
    }
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
                    Message::Context { .. } | Message::Assistant { .. } | Message::Tool { .. } => {
                        None
                    }
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
                Message::Context { .. } | Message::User { .. } | Message::Assistant { .. } => None,
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
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("text must be a string".to_string()))?;
        Ok(text.to_uppercase())
    }
}
