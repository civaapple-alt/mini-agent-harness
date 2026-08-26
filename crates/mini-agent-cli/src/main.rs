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
mod sandbox;
mod security;
mod session;
mod skills;
mod trace;
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

use config::RuntimeConfig;
use observer::RunObserver;
use openai::OpenAiModel;
use sandbox::SandboxKind;
use security::SecurityPreset;
use session::SessionRequest;
use workspace::ApprovalController;
use workspace::ApprovalMode;
use workspace::workspace_tools_with_read_roots;
use world::WorldState;

const HELP: &str = "mini-agent\n\nUSAGE:\n    mini-agent [--ephemeral] [--security-preset PRESET] [--sandbox KIND] [--trace PATH]\n    mini-agent resume SESSION_ID [--trace PATH]\n    mini-agent fork SESSION_ID [--trace PATH]\n    mini-agent sessions\n    mini-agent mentor insight SESSION_ID [--json] [--trace PATH]\n    mini-agent mentor verify SESSION_ID [--json] [--trace PATH] [--] <CRITERIA>\n    mini-agent ask [--auto] [--json] [--security-preset PRESET] [--sandbox KIND] [--trace PATH] [--] [PROMPT]\n    mini-agent auto [--ephemeral] [--security-preset PRESET] [--sandbox KIND] [--trace PATH] [--] [PROMPT]\n    mini-agent demo [--trace PATH] [--] <PROMPT>\n    mini-agent trace replay PATH [--json]\n    mini-agent trace summary PATH [--json]\n    mini-agent status [--json]\n    mini-agent doctor [--json]\n    mini-agent help [COMMAND]\n    mini-agent --version\n\nInteractive sessions run tools without per-step approval and persist settled checkpoints under ~/.mini-agent/sessions by default. Use `--ephemeral` for temporary in-memory sessions. Use `/auto off` to prompt for writes, shell, and MCP. `ask` is one script turn; add `--auto` when stdin is not a TTY. `auto` is the unattended copilot loop (unlimited steps unless MINI_AGENT_MAX_STEPS is set, compact).\n\nRun `mini-agent help COMMAND` or `mini-agent COMMAND --help` for details.\nUse `--` before a prompt that starts with `-`.\n\nENVIRONMENT:\n    OPENAI_API_KEY           Required by primary commands unless mentor overrides it\n    OPENAI_MODEL             Required by primary model commands\n    OPENAI_BASE_URL          Optional; defaults to https://api.openai.com/v1\n    MINI_AGENT_MAX_STEPS     Copilot/auto step cap; 0 means unlimited (default 0)\n    MENTOR_OPENAI_MODEL      Enables mentor commands with a dedicated model\n    MENTOR_OPENAI_API_KEY    Optional mentor credential override\n    MENTOR_OPENAI_BASE_URL   Optional mentor endpoint override";
const INTERACTIVE_HELP: &str = "mini-agent interactive\n\nUSAGE:\n    mini-agent [--ephemeral] [--trace PATH]\n\nStarts the interactive REPL. Tools run without per-step approval; shell is unsandboxed. Settled checkpoints are saved under ~/.mini-agent/sessions by default; use `--ephemeral` for temporary in-memory sessions. `/auto off` restores prompts.";
const RESUME_HELP: &str = "mini-agent resume\n\nUSAGE:\n    mini-agent resume SESSION_ID [--trace PATH]\n\nResumes the latest settled checkpoint of a durable session for this workspace.";
const FORK_HELP: &str = "mini-agent fork\n\nUSAGE:\n    mini-agent fork SESSION_ID [--trace PATH]\n\nForks a new independent session from the latest settled checkpoint of an existing session.";
const SESSIONS_HELP: &str = "mini-agent sessions\n\nUSAGE:\n    mini-agent sessions\n\nLists bounded durable sessions for the current workspace under ~/.mini-agent/sessions.";
const MENTOR_HELP: &str = "mini-agent mentor\n\nUSAGE:\n    mini-agent mentor insight SESSION_ID [--json] [--trace PATH]\n    mini-agent mentor verify SESSION_ID [--json] [--trace PATH] [--] <CRITERIA>\n\nRuns a tool-free independent model against the latest settled checkpoint. The result is appended as a derived item and never enters the primary conversation history.\n\nCONFIGURATION:\n    MENTOR_OPENAI_MODEL      Required dedicated mentor model\n    MENTOR_OPENAI_API_KEY    Optional; falls back to OPENAI_API_KEY\n    MENTOR_OPENAI_BASE_URL   Optional; falls back to OPENAI_BASE_URL";
const ASK_HELP: &str = "mini-agent ask\n\nUSAGE:\n    mini-agent ask [--auto] [--json] [--trace PATH] [--] [PROMPT]\n\nRuns one script-facing turn (8 steps, no compaction). If PROMPT is omitted, reads at most 32 KiB from stdin.\nOn a TTY, tools run without per-step approval. When stdin is not a TTY, sensitive tools fail closed unless `--auto`.\nProgress is written to stderr and the final result to stdout.\n\nOPTIONS:\n    --auto        Permit sensitive tools without a TTY\n    --json        Emit a machine-readable final result\n    --trace PATH  Write JSONL observation events";
const RUN_HELP: &str = "mini-agent run\n\nUSAGE:\n    mini-agent run [--auto] [--json] [--trace PATH] [--] <PROMPT>\n\nAlias of `ask`. Prefer `ask` in scripts and docs.";
const AUTO_HELP: &str = "mini-agent auto\n\nUSAGE:\n    mini-agent auto [--ephemeral] [--trace PATH] [--] [PROMPT]\n\nUnattended copilot: no per-step approval, unlimited model steps (MINI_AGENT_MAX_STEPS, 0 = unlimited), and context compaction that keeps recent tool work.\nWith a prompt, runs one copilot turn. Without a prompt, starts the REPL in copilot mode.";
const DEMO_HELP: &str = "mini-agent demo\n\nUSAGE:\n    mini-agent demo [--trace PATH] [--] <PROMPT>\n\nRuns the deterministic local demo without provider credentials.";
const TRACE_HELP: &str = "mini-agent trace\n\nUSAGE:\n    mini-agent trace replay PATH [--json]\n    mini-agent trace summary PATH [--json]\n\nReplays and analyzes deterministic JSONL observation traces offline without contacting model providers.";
const STATUS_HELP: &str = "mini-agent status\n\nUSAGE:\n    mini-agent status [--json]\n\nPrints effective non-secret startup configuration.";
const DOCTOR_HELP: &str = "mini-agent doctor\n\nUSAGE:\n    mini-agent doctor [--json]\n\nChecks local configuration without contacting the model provider.";
const VERSION_HELP: &str =
    "mini-agent version\n\nUSAGE:\n    mini-agent version\n    mini-agent --version";
const AUTO_MAX_STEPS: usize = 0;

pub(crate) fn version_line() -> String {
    format!("mini-agent {} ({})", env!("CARGO_PKG_VERSION"), git_sha())
}

pub(crate) fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("unknown")
}

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
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else {
                SessionRequest::New
            };
            repl::run(invocation.trace, ApprovalMode::Automatic, false, request).await
        }
        Command::Demo => run_demo(invocation.prompt, invocation.trace).await,
        Command::Run => {
            ask::run(
                invocation.prompt,
                invocation.trace,
                invocation.json,
                invocation.automatic,
                invocation.security_preset,
            )
            .await
        }
        Command::Ask => {
            ask::run(
                invocation.prompt,
                invocation.trace,
                invocation.json,
                invocation.automatic,
                invocation.security_preset,
            )
            .await
        }
        Command::Auto if invocation.prompt.is_empty() => {
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else {
                SessionRequest::New
            };
            repl::run(invocation.trace, ApprovalMode::Automatic, true, request).await
        }
        Command::Auto => run_auto(invocation.prompt, invocation.trace).await,
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
            )
            .await
        }
        Command::Fork => {
            repl::run(
                invocation.trace,
                ApprovalMode::Automatic,
                false,
                SessionRequest::Fork(invocation.prompt),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Interactive,
    Demo,
    Run,
    Ask,
    Auto,
    Resume,
    Fork,
    Sessions,
    Mentor,
    TraceReplay,
    TraceSummary,
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
    #[allow(dead_code)]
    persist: bool,
    ephemeral: bool,
    security_preset: SecurityPreset,
    #[allow(dead_code)]
    sandbox_kind: SandboxKind,
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
    Fork,
    Sessions,
    Mentor,
    Demo,
    Trace,
    Status,
    Doctor,
    Version,
}

fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let command = match args.peek().map(String::as_str) {
        None | Some("--trace" | "--persist" | "--ephemeral" | "--no-persist") => {
            Command::Interactive
        }
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
        Some("trace") => {
            args.next();
            match args.next().as_deref() {
                Some("replay") => Command::TraceReplay,
                Some("summary") => Command::TraceSummary,
                Some("--help" | "-h") => return Ok(help_invocation(HelpTopic::Trace)),
                Some(other) => return Err(format!("unknown trace subcommand: {other}")),
                None => return Ok(help_invocation(HelpTopic::Trace)),
            }
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
        Some("fork") => {
            args.next();
            Command::Fork
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
            ephemeral: false,
            security_preset: SecurityPreset::Default,
            sandbox_kind: SandboxKind::Native,
            help_topic: HelpTopic::Root,
        });
    }

    let mut args = remaining.into_iter();
    let mut prompt = Vec::new();
    let mut trace = None;
    let mut json = false;
    let mut automatic = false;
    let mut persist = false;
    let mut ephemeral = false;
    let mut security_preset = SecurityPreset::Default;
    let mut sandbox_kind = SandboxKind::Native;
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
        } else if options && (argument == "--ephemeral" || argument == "--no-persist") {
            if ephemeral {
                return Err(format!("{argument} may be provided only once"));
            }
            ephemeral = true;
        } else if options && argument == "--security-preset" {
            let value = args
                .next()
                .ok_or_else(|| "--security-preset requires a preset name".to_string())?;
            security_preset = SecurityPreset::parse(&value)?;
        } else if options && argument == "--sandbox" {
            let value = args
                .next()
                .ok_or_else(|| "--sandbox requires a sandbox kind".to_string())?;
            sandbox_kind = SandboxKind::parse(&value)?;
        } else if options && argument.starts_with('-') {
            return Err(format!("unknown option: {argument}"));
        } else {
            prompt.push(argument);
        }
    }
    if matches!(command, Command::Interactive) && !prompt.is_empty() {
        return Err("interactive mode does not accept a prompt; use `ask`".to_string());
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
    if command == Command::Fork && prompt.len() != 1 {
        return Err("fork requires exactly one SESSION_ID".to_string());
    }
    if matches!(command, Command::TraceReplay | Command::TraceSummary) && prompt.len() != 1 {
        return Err("trace subcommand requires exactly one PATH".to_string());
    }
    if json
        && !matches!(
            command,
            Command::Ask
                | Command::Run
                | Command::Mentor
                | Command::Status
                | Command::Doctor
                | Command::TraceReplay
                | Command::TraceSummary
        )
    {
        return Err(
            "--json is supported only by ask, mentor, status, doctor, and trace".to_string(),
        );
    }
    if automatic && !matches!(command, Command::Ask | Command::Run) {
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
    if trace.is_some() && matches!(command, Command::TraceReplay | Command::TraceSummary) {
        return Err("--trace is not supported by trace subcommands".to_string());
    }
    if persist && ephemeral {
        return Err("--persist and --ephemeral cannot be combined".to_string());
    }
    if persist
        && !(command == Command::Interactive || command == Command::Auto && prompt.is_empty())
    {
        return Err("--persist is supported only by interactive sessions".to_string());
    }
    if ephemeral
        && !(command == Command::Interactive || command == Command::Auto && prompt.is_empty())
    {
        return Err("--ephemeral is supported only by interactive sessions".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        trace,
        json,
        automatic,
        persist,
        ephemeral,
        security_preset,
        sandbox_kind,
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
        ephemeral: false,
        security_preset: SecurityPreset::Default,
        sandbox_kind: SandboxKind::Native,
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
        "fork" => Ok(HelpTopic::Fork),
        "sessions" => Ok(HelpTopic::Sessions),
        "mentor" => Ok(HelpTopic::Mentor),
        "demo" => Ok(HelpTopic::Demo),
        "trace" => Ok(HelpTopic::Trace),
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
        Command::Fork => HelpTopic::Fork,
        Command::Sessions => HelpTopic::Sessions,
        Command::Mentor => HelpTopic::Mentor,
        Command::Demo => HelpTopic::Demo,
        Command::TraceReplay | Command::TraceSummary => HelpTopic::Trace,
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
        HelpTopic::Fork => FORK_HELP,
        HelpTopic::Sessions => SESSIONS_HELP,
        HelpTopic::Mentor => MENTOR_HELP,
        HelpTopic::Demo => DEMO_HELP,
        HelpTopic::Trace => TRACE_HELP,
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

async fn run_demo(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    let model = DemoModel { turn: 0 };
    let tools = ToolRegistry::new(vec![Box::new(Uppercase)]);
    let mut harness = Harness::new(model, tools, HarnessConfig::default());
    match run_with_observer(&mut harness, prompt, trace).await {
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

async fn run_auto(prompt: String, trace: Option<PathBuf>) -> ExitCode {
    print_auto_warning();
    let approval = ApprovalController::new(ApprovalMode::Automatic);
    let runtime = match RuntimeConfig::load() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let mut harness = match openai_harness(
        approval,
        harness_config_auto(true, runtime.copilot_max_steps()),
    ) {
        Ok(harness) => harness,
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

fn openai_harness(
    approval: ApprovalController,
    config: HarnessConfig,
) -> Result<Harness<OpenAiModel>, String> {
    Ok(prepare_openai_harness(&RuntimeConfig::load()?, approval, config)?.harness)
}

struct HarnessBuild {
    harness: Harness<OpenAiModel>,
    stable_system_prompt: String,
    world: WorldState,
    enabled_mcp_servers: Vec<String>,
    mcp_tool_count: usize,
    retry_mcp_servers: Vec<skills::McpServerConfig>,
}

fn prepare_openai_harness(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
) -> Result<HarnessBuild, String> {
    let provider = runtime_config.provider_settings()?;
    let copilot = config.context_limit_behavior == ContextLimitBehavior::Compact;
    let model = match OpenAiModel::new(provider.api_key, provider.model, provider.base_url) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
    let workspace = runtime_config.workspace();
    let project_instructions = project_context::load_agents_md(&workspace)?;
    if let Some(warning) = project_instructions.truncation_warning() {
        eprintln!("warning: {warning}");
    }
    config.system_prompt = project_instructions.augment(&config.system_prompt);
    let skill_discovery = skills::discover(&workspace);
    for diagnostic in skill_discovery.diagnostics() {
        eprintln!("warning: {diagnostic}");
    }
    config.system_prompt = skill_discovery.augment_system_prompt(&config.system_prompt)?;
    let mut tools = match workspace_tools_with_read_roots(
        workspace.clone(),
        approval.clone(),
        skill_discovery.extra_read_roots().to_vec(),
    ) {
        Ok(tools) => tools,
        Err(error) => return Err(error.to_string()),
    };
    let configured_mcp_servers = skill_discovery.mcp_servers().to_vec();
    let approval_mode = approval.mode();
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
    let world = WorldState::detect(&workspace, approval_mode, copilot);
    let world_context = world.model_context()?;
    let mut harness = Harness::new(model, ToolRegistry::new(tools), config);
    harness
        .append_context(world_context)
        .map_err(|error| error.to_string())?;
    Ok(HarnessBuild {
        harness,
        stable_system_prompt,
        world,
        enabled_mcp_servers,
        mcp_tool_count,
        retry_mcp_servers,
    })
}

pub(crate) fn harness_config(copilot: bool) -> HarnessConfig {
    harness_config_auto(copilot, AUTO_MAX_STEPS)
}

pub(crate) fn harness_config_auto(copilot: bool, auto_max_steps: usize) -> HarnessConfig {
    if copilot {
        HarnessConfig {
            max_steps: auto_max_steps,
            context_limit_behavior: ContextLimitBehavior::Compact,
            ..HarnessConfig::default()
        }
    } else {
        HarnessConfig::default()
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
    fn auto_harness_uses_unlimited_steps_by_default() {
        let config = harness_config(true);
        assert_eq!(config.max_steps, 0);
        assert_eq!(config.context_limit_behavior, ContextLimitBehavior::Compact);
        let capped = harness_config_auto(true, 40);
        assert_eq!(capped.max_steps, 40);
        assert_eq!(harness_config(false).max_steps, 8);
    }

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
        let ephemeral = parse_args(vec!["--ephemeral".to_string()]).unwrap();
        let no_persist = parse_args(vec!["--no-persist".to_string()]).unwrap();
        let resume = parse_args(vec!["resume".to_string(), "s-123".to_string()]).unwrap();
        let sessions = parse_args(vec!["sessions".to_string()]).unwrap();

        assert_eq!(persistent.command, Command::Interactive);
        assert!(persistent.persist);
        assert!(!persistent.ephemeral);
        assert_eq!(ephemeral.command, Command::Interactive);
        assert!(ephemeral.ephemeral);
        assert_eq!(no_persist.command, Command::Interactive);
        assert!(no_persist.ephemeral);
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

    #[test]
    fn parses_trace_commands() {
        let replay = parse_args(vec![
            "trace".to_string(),
            "replay".to_string(),
            "trace.jsonl".to_string(),
        ])
        .unwrap();
        assert_eq!(replay.command, Command::TraceReplay);
        assert_eq!(replay.prompt, "trace.jsonl");

        let summary = parse_args(vec![
            "trace".to_string(),
            "summary".to_string(),
            "trace.jsonl".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(summary.command, Command::TraceSummary);
        assert_eq!(summary.prompt, "trace.jsonl");
        assert!(summary.json);

        assert_eq!(
            parse_args(vec!["trace".to_string(), "unknown".to_string()]).unwrap_err(),
            "unknown trace subcommand: unknown"
        );
    }

    #[test]
    fn parses_fork_command() {
        let invocation = parse_args(vec!["fork".to_string(), "s-12345678".to_string()]).unwrap();
        assert_eq!(invocation.command, Command::Fork);
        assert_eq!(invocation.prompt, "s-12345678");

        assert_eq!(
            parse_args(vec!["fork".to_string()]).unwrap_err(),
            "fork requires exactly one SESSION_ID"
        );
    }

    #[test]
    fn parses_security_preset_and_sandbox_options() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--security-preset".to_string(),
            "turbomode".to_string(),
            "--sandbox".to_string(),
            "native".to_string(),
            "list files".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.security_preset, SecurityPreset::Turbomode);
        assert_eq!(invocation.sandbox_kind, SandboxKind::Native);
        assert_eq!(invocation.prompt, "list files");
    }
}
