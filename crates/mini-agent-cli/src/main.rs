mod ask;
mod config;
mod env_file;
mod marketplaces;
mod mcp;
mod mentor;
mod observer;
mod openai;
mod processes;
mod project_context;
mod repl;
mod result_store;
mod session;
mod skills;
mod workspace;
mod world;

use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
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

use config::RuntimeConfig;
use observer::RunObserver;
use openai::OpenAiModel;
use session::SessionRequest;
use workspace::ApprovalController;
use workspace::ApprovalMode;
use workspace::workspace_tools;
use world::WorldState;

const HELP: &str = "mini-agent\n\nUSAGE:\n    mini-agent [--persist] [--trace PATH]\n    mini-agent resume SESSION_ID [--trace PATH]\n    mini-agent sessions\n    mini-agent mentor insight SESSION_ID [--json] [--trace PATH]\n    mini-agent mentor verify SESSION_ID [--json] [--trace PATH] [--] <CRITERIA>\n    mini-agent ask [--auto] [--json] [--trace PATH] [--] [PROMPT]\n    mini-agent run [--trace PATH] [--] <PROMPT>\n    mini-agent auto [--persist] [--trace PATH] [--] [PROMPT]\n    mini-agent demo [--trace PATH] [--] <PROMPT>\n    mini-agent status [--json]\n    mini-agent doctor [--json]\n    mini-agent help [COMMAND]\n    mini-agent --version\n\nRun `mini-agent help COMMAND` or `mini-agent COMMAND --help` for details.\nUse `--` before a prompt that starts with `-`.\n\nENVIRONMENT:\n    OPENAI_API_KEY           Required by primary commands unless mentor overrides it\n    OPENAI_MODEL             Required by primary model commands\n    OPENAI_BASE_URL          Optional; defaults to https://api.openai.com/v1\n    MENTOR_OPENAI_MODEL      Enables mentor commands with a dedicated model\n    MENTOR_OPENAI_API_KEY    Optional mentor credential override\n    MENTOR_OPENAI_BASE_URL   Optional mentor endpoint override";
const INTERACTIVE_HELP: &str = "mini-agent interactive\n\nUSAGE:\n    mini-agent [--persist] [--trace PATH]\n\nStarts the approval-gated interactive REPL. --persist creates a resumable project session.";
const RESUME_HELP: &str = "mini-agent resume\n\nUSAGE:\n    mini-agent resume SESSION_ID [--trace PATH]\n\nResumes the latest settled checkpoint of a durable project session.";
const SESSIONS_HELP: &str = "mini-agent sessions\n\nUSAGE:\n    mini-agent sessions\n\nLists bounded durable sessions in the current project.";
const MENTOR_HELP: &str = "mini-agent mentor\n\nUSAGE:\n    mini-agent mentor insight SESSION_ID [--json] [--trace PATH]\n    mini-agent mentor verify SESSION_ID [--json] [--trace PATH] [--] <CRITERIA>\n\nRuns a tool-free independent model against the latest settled checkpoint. The result is appended as a derived item and never enters the primary conversation history.\n\nCONFIGURATION:\n    MENTOR_OPENAI_MODEL      Required dedicated mentor model\n    MENTOR_OPENAI_API_KEY    Optional; falls back to OPENAI_API_KEY\n    MENTOR_OPENAI_BASE_URL   Optional; falls back to OPENAI_BASE_URL";
const ASK_HELP: &str = "mini-agent ask\n\nUSAGE:\n    mini-agent ask [--auto] [--json] [--trace PATH] [--] [PROMPT]\n\nRuns one script-facing turn. If PROMPT is omitted, reads at most 32 KiB from stdin.\nProgress is written to stderr and the final result to stdout.\n\nOPTIONS:\n    --auto        Run tools without approval\n    --json        Emit a machine-readable final result\n    --trace PATH  Write JSONL observation events";
const RUN_HELP: &str = "mini-agent run\n\nUSAGE:\n    mini-agent run [--trace PATH] [--] <PROMPT>\n\nRuns one approval-gated model turn.";
const AUTO_HELP: &str = "mini-agent auto\n\nUSAGE:\n    mini-agent auto [--trace PATH] [--] [PROMPT]\n\nRuns an automatic turn, or starts the REPL in automatic mode when PROMPT is omitted.";
const DEMO_HELP: &str = "mini-agent demo\n\nUSAGE:\n    mini-agent demo [--trace PATH] [--] <PROMPT>\n\nRuns the deterministic local demo without provider credentials.";
const STATUS_HELP: &str = "mini-agent status\n\nUSAGE:\n    mini-agent status [--json]\n\nPrints effective non-secret startup configuration.";
const DOCTOR_HELP: &str = "mini-agent doctor\n\nUSAGE:\n    mini-agent doctor [--json]\n\nChecks local configuration without contacting the model provider.";
const VERSION_HELP: &str =
    "mini-agent version\n\nUSAGE:\n    mini-agent version\n    mini-agent --version";
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
        Command::Interactive => {
            let request = if invocation.persist {
                SessionRequest::New
            } else {
                SessionRequest::Disabled
            };
            run_interactive(invocation.trace, ApprovalMode::Interactive, request).await
        }
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
            let request = if invocation.persist {
                SessionRequest::New
            } else {
                SessionRequest::Disabled
            };
            run_interactive(invocation.trace, ApprovalMode::Automatic, request).await
        }
        Command::Auto => run_auto(invocation.prompt, invocation.trace).await,
        Command::Help => {
            println!("{}", help_text(invocation.help_topic));
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("mini-agent {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Status => run_status(invocation.json),
        Command::Doctor => run_doctor(invocation.json),
        Command::Resume => {
            run_interactive(
                invocation.trace,
                ApprovalMode::Interactive,
                SessionRequest::Resume(invocation.prompt),
            )
            .await
        }
        Command::Sessions => run_sessions(),
        Command::Mentor => mentor::run(invocation.prompt, invocation.trace, invocation.json).await,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Interactive,
    Demo,
    Run,
    Ask,
    Auto,
    Resume,
    Sessions,
    Mentor,
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
    persist: bool,
    help_topic: HelpTopic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelpTopic {
    Root,
    Interactive,
    Ask,
    Run,
    Auto,
    Resume,
    Sessions,
    Mentor,
    Demo,
    Status,
    Doctor,
    Version,
}

fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let command = match args.peek().map(String::as_str) {
        None | Some("--trace" | "--persist") => Command::Interactive,
        Some("help") => {
            args.next();
            let topic = match args.next() {
                Some(name) => help_topic(&name)?,
                None => HelpTopic::Root,
            };
            if let Some(argument) = args.next() {
                return Err(format!("unexpected argument after help topic: {argument}"));
            }
            return Ok(help_invocation(topic));
        }
        Some("--help" | "-h") => {
            args.next();
            if let Some(argument) = args.next() {
                return Err(format!("unexpected argument after --help: {argument}"));
            }
            return Ok(help_invocation(HelpTopic::Root));
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
        Some("resume") => {
            args.next();
            Command::Resume
        }
        Some("sessions") => {
            args.next();
            Command::Sessions
        }
        Some("mentor") => {
            args.next();
            Command::Mentor
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
    let remaining = args.collect::<Vec<_>>();
    let delimiter = remaining.iter().position(|argument| argument == "--");
    if let Some(position) = remaining
        .iter()
        .position(|argument| argument == "--help" || argument == "-h")
        && delimiter.is_none_or(|delimiter| position < delimiter)
    {
        if remaining.len() != 1 {
            return Err("--help cannot be combined with other arguments".to_string());
        }
        return Ok(help_invocation(help_topic_for(command)));
    }
    if command == Command::Version {
        if let Some(argument) = remaining.first() {
            return Err(format!("version does not accept arguments: {argument}"));
        }
        return Ok(Invocation {
            command,
            prompt: String::new(),
            trace: None,
            json: false,
            automatic: false,
            persist: false,
            help_topic: HelpTopic::Root,
        });
    }

    let mut args = remaining.into_iter();
    let mut prompt = Vec::new();
    let mut trace = None;
    let mut json = false;
    let mut automatic = false;
    let mut persist = false;
    let mut options = true;
    while let Some(argument) = args.next() {
        if options && argument == "--" {
            options = false;
        } else if options && argument == "--trace" {
            if trace.is_some() {
                return Err("--trace may be provided only once".to_string());
            }
            trace = Some(PathBuf::from(
                args.next()
                    .ok_or_else(|| "--trace requires a path".to_string())?,
            ));
        } else if options && argument == "--json" {
            if json {
                return Err("--json may be provided only once".to_string());
            }
            json = true;
        } else if options && argument == "--auto" {
            if automatic {
                return Err("--auto may be provided only once".to_string());
            }
            automatic = true;
        } else if options && argument == "--persist" {
            if persist {
                return Err("--persist may be provided only once".to_string());
            }
            persist = true;
        } else if options && argument.starts_with('-') {
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
    if matches!(
        command,
        Command::Status | Command::Doctor | Command::Sessions
    ) && !prompt.is_empty()
    {
        return Err("this command does not accept positional arguments".to_string());
    }
    if command == Command::Resume && prompt.len() != 1 {
        return Err("resume requires exactly one SESSION_ID".to_string());
    }
    if json
        && !matches!(
            command,
            Command::Ask | Command::Mentor | Command::Status | Command::Doctor
        )
    {
        return Err("--json is supported only by ask, mentor, status, and doctor".to_string());
    }
    if automatic && command != Command::Ask {
        return Err("--auto is supported only by ask".to_string());
    }
    if trace.is_some()
        && matches!(
            command,
            Command::Status | Command::Doctor | Command::Sessions
        )
    {
        return Err("--trace is not supported by status, doctor, or sessions".to_string());
    }
    if persist
        && !(command == Command::Interactive || command == Command::Auto && prompt.is_empty())
    {
        return Err("--persist is supported only by interactive sessions".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        trace,
        json,
        automatic,
        persist,
        help_topic: HelpTopic::Root,
    })
}

fn help_invocation(help_topic: HelpTopic) -> Invocation {
    Invocation {
        command: Command::Help,
        prompt: String::new(),
        trace: None,
        json: false,
        automatic: false,
        persist: false,
        help_topic,
    }
}

fn help_topic(name: &str) -> Result<HelpTopic, String> {
    match name {
        "interactive" | "repl" => Ok(HelpTopic::Interactive),
        "ask" => Ok(HelpTopic::Ask),
        "run" => Ok(HelpTopic::Run),
        "auto" => Ok(HelpTopic::Auto),
        "resume" => Ok(HelpTopic::Resume),
        "sessions" => Ok(HelpTopic::Sessions),
        "mentor" => Ok(HelpTopic::Mentor),
        "demo" => Ok(HelpTopic::Demo),
        "status" => Ok(HelpTopic::Status),
        "doctor" => Ok(HelpTopic::Doctor),
        "version" => Ok(HelpTopic::Version),
        _ => Err(format!("unknown help topic: {name}")),
    }
}

fn help_topic_for(command: Command) -> HelpTopic {
    match command {
        Command::Interactive => HelpTopic::Interactive,
        Command::Ask => HelpTopic::Ask,
        Command::Run => HelpTopic::Run,
        Command::Auto => HelpTopic::Auto,
        Command::Resume => HelpTopic::Resume,
        Command::Sessions => HelpTopic::Sessions,
        Command::Mentor => HelpTopic::Mentor,
        Command::Demo => HelpTopic::Demo,
        Command::Status => HelpTopic::Status,
        Command::Doctor => HelpTopic::Doctor,
        Command::Version => HelpTopic::Version,
        Command::Help => HelpTopic::Root,
    }
}

fn help_text(topic: HelpTopic) -> &'static str {
    match topic {
        HelpTopic::Root => HELP,
        HelpTopic::Interactive => INTERACTIVE_HELP,
        HelpTopic::Ask => ASK_HELP,
        HelpTopic::Run => RUN_HELP,
        HelpTopic::Auto => AUTO_HELP,
        HelpTopic::Resume => RESUME_HELP,
        HelpTopic::Sessions => SESSIONS_HELP,
        HelpTopic::Mentor => MENTOR_HELP,
        HelpTopic::Demo => DEMO_HELP,
        HelpTopic::Status => STATUS_HELP,
        HelpTopic::Doctor => DOCTOR_HELP,
        HelpTopic::Version => VERSION_HELP,
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

async fn run_interactive(
    trace: Option<PathBuf>,
    initial_mode: ApprovalMode,
    session: SessionRequest,
) -> ExitCode {
    repl::run(trace, initial_mode, session).await
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

struct ReplHarnessBuild {
    harness: Harness<OpenAiModel>,
    stable_system_prompt: String,
    world: WorldState,
    enabled_mcp_servers: Vec<String>,
    mcp_tool_count: usize,
    retry_mcp_servers: Vec<skills::McpServerConfig>,
}

fn build_repl_harness(
    approval: ApprovalController,
    config: HarnessConfig,
) -> Result<ReplHarnessBuild, String> {
    let runtime_config = RuntimeConfig::load()?;
    build_openai_harness_with_retries(&runtime_config, approval, config)
}

fn build_openai_harness_with(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
) -> Result<Harness<OpenAiModel>, String> {
    build_openai_harness_with_retries(runtime_config, approval, config).map(|build| build.harness)
}

fn build_openai_harness_with_retries(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
) -> Result<ReplHarnessBuild, String> {
    let provider = runtime_config.provider_settings()?;
    let mode = approval.mode();
    let model = match OpenAiModel::new(provider.api_key, provider.model, provider.base_url) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
    let workspace = runtime_config.workspace();
    config.system_prompt =
        project_context::augment_system_prompt(&config.system_prompt, &workspace)?;
    let skill_discovery = skills::discover(&workspace);
    for diagnostic in skill_discovery.diagnostics() {
        eprintln!("warning: {diagnostic}");
    }
    config.system_prompt = skill_discovery.augment_system_prompt(&config.system_prompt)?;
    let mut tools = match workspace_tools(workspace.clone(), approval.clone()) {
        Ok(tools) => tools,
        Err(error) => return Err(error.to_string()),
    };
    let configured_mcp_servers = skill_discovery.mcp_servers().to_vec();
    let mcp::LoadResult {
        tools: mcp_tools,
        loaded_servers,
        diagnostics,
    } = mcp::load(&configured_mcp_servers, approval);
    for diagnostic in diagnostics {
        eprintln!("warning: {diagnostic}");
    }
    let enabled_mcp_servers = loaded_servers.iter().cloned().collect();
    let mcp_tool_count = mcp_tools.len();
    tools.extend(mcp_tools);
    let retry_mcp_servers = configured_mcp_servers
        .into_iter()
        .filter(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        })
        .collect();
    let stable_system_prompt = config.system_prompt.clone();
    let world = WorldState::detect(&workspace, mode);
    let world_context = world.model_context()?;
    let mut harness = Harness::new(model, ToolRegistry::new(tools), config);
    harness
        .append_context(world_context)
        .map_err(|error| error.to_string())?;
    Ok(ReplHarnessBuild {
        harness,
        stable_system_prompt,
        world,
        enabled_mcp_servers,
        mcp_tool_count,
        retry_mcp_servers,
    })
}

fn harness_config(mode: ApprovalMode) -> HarnessConfig {
    match mode {
        ApprovalMode::Interactive => HarnessConfig::default(),
        ApprovalMode::Automatic => HarnessConfig {
            max_steps: AUTO_MAX_STEPS,
            context_limit_behavior: ContextLimitBehavior::Compact,
            ..HarnessConfig::default()
        },
    }
}

fn print_auto_warning() {
    eprintln!(
        "warning: auto mode runs workspace writes, MCP servers, and unsandboxed shell commands without approval"
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
    fn parses_durable_session_commands() {
        let persistent = parse_args(vec!["--persist".to_string()]).unwrap();
        let resume = parse_args(vec!["resume".to_string(), "s-123".to_string()]).unwrap();
        let sessions = parse_args(vec!["sessions".to_string()]).unwrap();

        assert_eq!(persistent.command, Command::Interactive);
        assert!(persistent.persist);
        assert_eq!(resume.command, Command::Resume);
        assert_eq!(resume.prompt, "s-123");
        assert_eq!(sessions.command, Command::Sessions);
    }

    #[test]
    fn parses_mentor_commands_and_options() {
        let insight = parse_args(vec![
            "mentor".to_string(),
            "insight".to_string(),
            "s-123".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        let verify = parse_args(vec![
            "mentor".to_string(),
            "verify".to_string(),
            "s-123".to_string(),
            "--".to_string(),
            "tests pass".to_string(),
        ])
        .unwrap();

        assert_eq!(insight.command, Command::Mentor);
        assert_eq!(insight.prompt, "insight s-123");
        assert!(insight.json);
        assert_eq!(verify.command, Command::Mentor);
        assert_eq!(verify.prompt, "verify s-123 tests pass");
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
    fn parses_subcommand_help_forms() {
        let option = parse_args(vec!["ask".to_string(), "--help".to_string()]).unwrap();
        let command = parse_args(vec!["help".to_string(), "ask".to_string()]).unwrap();
        let version = parse_args(vec!["version".to_string(), "--help".to_string()]).unwrap();

        assert_eq!(option.command, Command::Help);
        assert_eq!(option.help_topic, HelpTopic::Ask);
        assert_eq!(command.command, Command::Help);
        assert_eq!(command.help_topic, HelpTopic::Ask);
        assert_eq!(version.command, Command::Help);
        assert_eq!(version.help_topic, HelpTopic::Version);
    }

    #[test]
    fn option_delimiter_allows_prompt_starting_with_dash() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--".to_string(),
            "--explain".to_string(),
            "this".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "--explain this");
    }

    #[test]
    fn rejects_options_unsupported_by_a_command() {
        assert_eq!(
            parse_args(vec![
                "status".to_string(),
                "--trace".to_string(),
                "events.jsonl".to_string(),
            ])
            .unwrap_err(),
            "--trace is not supported by status, doctor, or sessions"
        );
        assert_eq!(
            parse_args(vec!["--version".to_string(), "extra".to_string()]).unwrap_err(),
            "version does not accept arguments: extra"
        );
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
