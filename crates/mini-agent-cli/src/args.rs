use std::path::PathBuf;

use mini_agent_host::sandbox::SandboxKind;
use mini_agent_host::security::SecurityPreset;

pub const HELP: &str = "mini-agent — bounded native coding-agent CLI

USAGE:
    mini-agent                         Interactive session
    mini-agent auto [PROMPT]           Autonomous session (REPL when omitted)
    mini-agent ask [PROMPT]             One-shot/script-friendly turn
    mini-agent <COMMAND> --help         Detailed command help

QUICK START:
    mini-agent doctor                  Check local setup (no provider call)
    mini-agent demo \"make this loud\"  Run the offline demo (no credentials)
    mini-agent ask \"summarize repo\"    Run one provider-backed turn
    mini-agent auto                     Start the interactive copilot

COMMANDS:
    resume, fork, sessions              Durable session management
    status, doctor                      Configuration and prerequisite checks
    mentor, trace                       Review and replay settled runs
    demo                                Deterministic offline run

COMMON OPTIONS:
    --security-preset PRESET            default | turbomode | full-machine
    --sandbox KIND                      native | docker
    --web-search / --no-web-search      Built-in web search toggle
    --persist / --ephemeral             Save or discard session checkpoints
    --auto-approve, -y                  Allow sensitive tools in non-TTY ask
    --json                              Machine-readable output
    --trace PATH                        JSONL observation trace

CONFIG:
    OPENAI_API_KEY, OPENAI_MODEL, OPENAI_BASE_URL
    More provider and extension settings: `mini-agent help ask` and docs.

PROJECT:
    GitHub:  https://github.com/civaapple-alt/mini-agent-harness
    Creator: civaapple-alt
    License: MIT

Use `mini-agent help COMMAND` or `mini-agent COMMAND --help` for details.";

pub const INTERACTIVE_HELP: &str = "mini-agent interactive

USAGE:
    mini-agent [--ephemeral] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--trace PATH]

Starts the interactive REPL. Tools run without per-step approval; shell is protected by the sandbox.
Interactive and one-shot ask sessions are in-memory by default; use `--persist` to save settled checkpoints under ~/.mini-agent/sessions. Auto sessions persist by default; use `--ephemeral` for temporary in-memory sessions.
Use `/auto` to enter copilot mode; `/auto off` restores per-action prompts.
Use `/plan` or `/plan <prompt>` to enter Plan Mode (locks codebase mutations, drafts the session living plan); `/plan off` exits.
Use `/goal <objective>` to start Autonomous Goal Mode and immediately execute the first milestone.

OPTIONS:
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --ephemeral, --no-persist    Run in-memory without persisting session to disk
    --trace PATH                 Write JSONL observation events to file";

pub const RESUME_HELP: &str = "mini-agent resume

USAGE:
    mini-agent resume SESSION_ID [--trace PATH]

Resumes the latest settled checkpoint of a durable session for this workspace.";

pub const FORK_HELP: &str = "mini-agent fork

USAGE:
    mini-agent fork SESSION_ID [--trace PATH]

Forks a new independent session from the latest settled checkpoint of an existing session.";

pub const SESSIONS_HELP: &str = "mini-agent sessions

USAGE:
    mini-agent sessions

Lists bounded durable sessions for the current workspace under ~/.mini-agent/sessions.";

pub const MENTOR_HELP: &str = "mini-agent mentor

USAGE:
    mini-agent mentor insight SESSION_ID [--json] [--trace PATH]
    mini-agent mentor verify SESSION_ID [--json] [--trace PATH] [--] <CRITERIA>

Runs a tool-free independent model against the latest settled checkpoint. The result is appended as a derived item and never enters the primary conversation history.

CONFIGURATION:
    MENTOR_OPENAI_MODEL      Required dedicated mentor model
    MENTOR_OPENAI_API_KEY    Optional; falls back to OPENAI_API_KEY
    MENTOR_OPENAI_BASE_URL   Optional; falls back to OPENAI_BASE_URL";

pub const ASK_HELP: &str = "mini-agent ask

USAGE:
    mini-agent ask [--auto-approve|-y] [--max-steps N] [--json] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--trace PATH] [--] [PROMPT]

Runs one script-facing turn (8 steps by default, no compaction). If PROMPT is omitted, reads at most 32 KiB from stdin.
On a TTY, tools run without per-step approval. When stdin is not a TTY, sensitive tools fail closed unless `--auto-approve` (or `-y`).
Progress is written to stderr and the final result to stdout.

OPTIONS:
    --auto-approve, -y           Permit sensitive tools non-interactively (auto-approve)
    --max-steps N                Cap model steps for this turn (default: 8; 0 means unlimited)
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --json                       Emit a machine-readable final result
    --trace PATH                 Write JSONL observation events";

pub const RUN_HELP: &str = "mini-agent run

USAGE:
    mini-agent run [--auto-approve|-y] [--json] [--trace PATH] [--] <PROMPT>

Alias of `ask`. Prefer `ask` in scripts and docs.";

pub const AUTO_HELP: &str = "mini-agent auto

USAGE:
    mini-agent auto [--ephemeral] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--trace PATH] [--] [PROMPT]

Unattended copilot: runs continuous model/tool cycles without per-step approval, unlimited steps (unless capped by MINI_AGENT_MAX_STEPS), and automatic context compaction that preserves recent tool work.
With a prompt, runs one autonomous copilot turn to completion.
Without a prompt, starts the interactive REPL in copilot mode.

OPTIONS:
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --ephemeral, --no-persist    Run in-memory without persisting session to disk
    --trace PATH                 Write JSONL observation events";

pub const DEMO_HELP: &str = "mini-agent demo

USAGE:
    mini-agent demo [--trace PATH] [--] <PROMPT>

Runs the deterministic local demo without provider credentials.";

pub const TRACE_HELP: &str = "mini-agent trace

USAGE:
    mini-agent trace replay PATH [--json]
    mini-agent trace summary PATH [--json]

Replays and analyzes deterministic JSONL observation traces offline without contacting model providers.";

pub const STATUS_HELP: &str = "mini-agent status

USAGE:
    mini-agent status [--json]

Prints effective non-secret startup configuration, active preset, sandbox, and world state.";

pub const DOCTOR_HELP: &str = "mini-agent doctor

USAGE:
    mini-agent doctor [--json]

Checks local configuration and environment prerequisites without contacting the model provider.";

pub const VERSION_HELP: &str = "mini-agent version

USAGE:
    mini-agent version
    mini-agent --version";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
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
pub struct Invocation {
    pub command: Command,
    pub prompt: String,
    pub trace: Option<PathBuf>,
    pub json: bool,
    pub automatic: bool,
    #[allow(dead_code)]
    pub persist: bool,
    pub ephemeral: bool,
    pub session_id: Option<String>,
    pub security_preset: SecurityPreset,
    pub sandbox_kind: SandboxKind,
    pub web_search: Option<bool>,
    pub max_steps: Option<usize>,
    pub help_topic: HelpTopic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTopic {
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

pub fn parse_args(args: Vec<String>) -> Result<Invocation, String> {
    let mut args = args.into_iter().peekable();
    let command = match args.peek().map(String::as_str) {
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
        None => Command::Interactive,
        Some(other) if other.starts_with('-') => Command::Interactive,
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
            session_id: None,
            security_preset: SecurityPreset::Default,
            sandbox_kind: SandboxKind::Native,
            web_search: None,
            max_steps: None,
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
    let mut session_id = None;
    let mut security_preset = SecurityPreset::Default;
    let mut sandbox_kind = SandboxKind::Native;
    let mut web_search = None;
    let mut max_steps = None;
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
        } else if options
            && (argument == "--auto-approve"
                || argument == "-y"
                || argument == "--yes"
                || argument == "--auto")
        {
            if automatic {
                return Err(format!("{argument} may be provided only once"));
            }
            automatic = true;
        } else if options && (argument == "--session" || argument == "--session-id") {
            if session_id.is_some() {
                return Err(format!("{argument} may be provided only once"));
            }
            session_id = Some(
                args.next()
                    .ok_or_else(|| format!("{argument} requires a session ID"))?,
            );
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
        } else if options && (argument == "--web-search" || argument == "--search") {
            if web_search.is_some() {
                return Err(format!("{argument} may be provided only once"));
            }
            web_search = Some(true);
        } else if options && (argument == "--no-web-search" || argument == "--no-search") {
            if web_search.is_some() {
                return Err(format!("{argument} may be provided only once"));
            }
            web_search = Some(false);
        } else if options && argument == "--max-steps" {
            if max_steps.is_some() {
                return Err("--max-steps may be provided only once".to_string());
            }
            let value = args
                .next()
                .ok_or_else(|| "--max-steps requires a number".to_string())?;
            max_steps = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "--max-steps requires a non-negative integer".to_string())?,
            );
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
    if command == Command::Mentor {
        match prompt.first().map(String::as_str) {
            Some("insight") => {
                if prompt.len() != 2 {
                    return Err("mentor insight requires exactly one SESSION_ID".to_string());
                }
            }
            Some("verify") => {
                if prompt.len() < 3 {
                    return Err("mentor verify requires criteria text".to_string());
                }
            }
            Some(other) => return Err(format!("unknown mentor subcommand: {other}")),
            None => return Ok(help_invocation(HelpTopic::Mentor)),
        }
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
        return Err("--auto-approve is supported only by ask".to_string());
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
        && !(command == Command::Interactive
            || command == Command::Ask
            || command == Command::Run
            || command == Command::Auto)
    {
        return Err(
            "--persist is supported only by interactive, auto, and ask sessions".to_string(),
        );
    }
    if ephemeral
        && !(command == Command::Interactive
            || command == Command::Ask
            || command == Command::Run
            || command == Command::Auto)
    {
        return Err(
            "--ephemeral is supported only by interactive, auto, and ask sessions".to_string(),
        );
    }
    if session_id.is_some()
        && !matches!(
            command,
            Command::Ask | Command::Run | Command::Interactive | Command::Auto
        )
    {
        return Err("--session-id is supported only by ask, auto, and interactive".to_string());
    }
    if max_steps.is_some() && !matches!(command, Command::Ask | Command::Run) {
        return Err("--max-steps is supported only by ask".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        trace,
        json,
        automatic,
        persist,
        ephemeral,
        session_id,
        security_preset,
        sandbox_kind,
        web_search,
        max_steps,
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
        session_id: None,
        security_preset: SecurityPreset::Default,
        sandbox_kind: SandboxKind::Native,
        web_search: None,
        max_steps: None,
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

pub fn help_text(topic: HelpTopic) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_interactive_mode() {
        let invocation = parse_args(Vec::new()).unwrap();

        assert_eq!(invocation.command, Command::Interactive);
        assert_eq!(invocation.prompt, "");
        assert_eq!(invocation.trace, None);
        assert!(!invocation.json);
        assert!(!invocation.automatic);
        assert_eq!(invocation.security_preset, SecurityPreset::Default);
        assert_eq!(invocation.sandbox_kind, SandboxKind::Native);
        assert_eq!(invocation.web_search, None);
    }

    #[test]
    fn joins_one_shot_prompt() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "explain".to_string(),
            "the".to_string(),
            "code".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "explain the code");
        assert_eq!(invocation.trace, None);
        assert!(!invocation.json);
        assert!(!invocation.automatic);
    }

    #[test]
    fn one_shot_mode_requires_prompt() {
        assert_eq!(
            parse_args(vec!["demo".to_string()]).unwrap_err(),
            "prompt is required"
        );
    }

    #[test]
    fn option_delimiter_allows_prompt_starting_with_dash() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--".to_string(),
            "-v".to_string(),
            "--json".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "-v --json");
        assert_eq!(invocation.trace, None);
        assert!(!invocation.json);
        assert!(!invocation.automatic);
    }

    #[test]
    fn accepts_interactive_trace() {
        let invocation =
            parse_args(vec!["--trace".to_string(), "trace.jsonl".to_string()]).unwrap();

        assert_eq!(invocation.command, Command::Interactive);
        assert_eq!(invocation.prompt, "");
        assert_eq!(invocation.trace, Some(PathBuf::from("trace.jsonl")));
        assert!(!invocation.json);
        assert!(!invocation.automatic);
    }

    #[test]
    fn parses_durable_session_commands() {
        let sessions = parse_args(vec!["sessions".to_string()]).unwrap();
        assert_eq!(sessions.command, Command::Sessions);

        let resume = parse_args(vec![
            "resume".to_string(),
            "s-12345678".to_string(),
            "--trace".to_string(),
            "resume.jsonl".to_string(),
        ])
        .unwrap();
        assert_eq!(resume.command, Command::Resume);
        assert_eq!(resume.prompt, "s-12345678");
        assert_eq!(resume.trace, Some(PathBuf::from("resume.jsonl")));

        let auto_repl = parse_args(vec!["auto".to_string()]).unwrap();
        assert_eq!(auto_repl.command, Command::Auto);
        assert!(!auto_repl.persist);
        assert!(!auto_repl.ephemeral);

        let auto_ephemeral =
            parse_args(vec!["auto".to_string(), "--ephemeral".to_string()]).unwrap();
        assert_eq!(auto_ephemeral.command, Command::Auto);
        assert!(auto_ephemeral.ephemeral);

        assert_eq!(
            parse_args(vec!["resume".to_string()]).unwrap_err(),
            "resume requires exactly one SESSION_ID"
        );
        assert_eq!(
            parse_args(vec!["sessions".to_string(), "extra".to_string()]).unwrap_err(),
            "this command does not accept positional arguments"
        );
    }

    #[test]
    fn parses_mentor_commands_and_options() {
        let insight = parse_args(vec![
            "mentor".to_string(),
            "insight".to_string(),
            "s-12345678".to_string(),
            "--json".to_string(),
            "--trace".to_string(),
            "mentor.jsonl".to_string(),
        ])
        .unwrap();
        assert_eq!(insight.command, Command::Mentor);
        assert_eq!(insight.prompt, "insight s-12345678");
        assert!(insight.json);
        assert_eq!(insight.trace, Some(PathBuf::from("mentor.jsonl")));

        let verify = parse_args(vec![
            "mentor".to_string(),
            "verify".to_string(),
            "s-12345678".to_string(),
            "--json".to_string(),
            "--".to_string(),
            "- leading criteria".to_string(),
        ])
        .unwrap();
        assert_eq!(verify.command, Command::Mentor);
        assert_eq!(verify.prompt, "verify s-12345678 - leading criteria");
        assert!(verify.json);

        assert_eq!(
            parse_args(vec!["mentor".to_string(), "unknown".to_string()]).unwrap_err(),
            "unknown mentor subcommand: unknown"
        );
        assert_eq!(
            parse_args(vec!["mentor".to_string(), "insight".to_string()]).unwrap_err(),
            "mentor insight requires exactly one SESSION_ID"
        );
        assert_eq!(
            parse_args(vec![
                "mentor".to_string(),
                "verify".to_string(),
                "s-12345678".to_string()
            ])
            .unwrap_err(),
            "mentor verify requires criteria text"
        );
    }

    #[test]
    fn parses_script_ask_options() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--trace".to_string(),
            "trace.jsonl".to_string(),
            "--json".to_string(),
            "--auto-approve".to_string(),
            "explain".to_string(),
            "the".to_string(),
            "code".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "explain the code");
        assert_eq!(invocation.trace, Some(PathBuf::from("trace.jsonl")));
        assert!(invocation.json);
        assert!(invocation.automatic);
        assert_eq!(invocation.max_steps, None);

        let stepped = parse_args(vec![
            "ask".to_string(),
            "--max-steps".to_string(),
            "50".to_string(),
            "go".to_string(),
        ])
        .unwrap();
        assert_eq!(stepped.max_steps, Some(50));
    }

    #[test]
    fn parses_subcommand_help_forms() {
        let root = parse_args(vec!["--help".to_string()]).unwrap();
        assert_eq!(root.command, Command::Help);
        assert_eq!(root.help_topic, HelpTopic::Root);

        let ask_positional = parse_args(vec!["help".to_string(), "ask".to_string()]).unwrap();
        assert_eq!(ask_positional.command, Command::Help);
        assert_eq!(ask_positional.help_topic, HelpTopic::Ask);

        let ask_flag = parse_args(vec!["ask".to_string(), "--help".to_string()]).unwrap();
        assert_eq!(ask_flag.command, Command::Help);
        assert_eq!(ask_flag.help_topic, HelpTopic::Ask);

        let trace_flag = parse_args(vec!["trace".to_string(), "--help".to_string()]).unwrap();
        assert_eq!(trace_flag.command, Command::Help);
        assert_eq!(trace_flag.help_topic, HelpTopic::Trace);
    }

    #[test]
    fn root_help_stays_actionable_and_identifies_the_project() {
        assert!(HELP.lines().count() <= 60);
        assert!(HELP.contains("QUICK START:"));
        assert!(HELP.contains("https://github.com/civaapple-alt/mini-agent-harness"));
        assert!(HELP.contains("Creator: civaapple-alt"));
        assert!(HELP.contains("License: MIT"));
    }

    #[test]
    fn parses_version_command() {
        let invocation = parse_args(vec!["version".to_string()]).unwrap();
        assert_eq!(invocation.command, Command::Version);
        assert_eq!(invocation.prompt, "");

        let flag = parse_args(vec!["--version".to_string()]).unwrap();
        assert_eq!(flag.command, Command::Version);
        assert_eq!(flag.prompt, "");
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
        let ask_inv = parse_args(vec![
            "ask".to_string(),
            "--security-preset".to_string(),
            "turbomode".to_string(),
            "--sandbox".to_string(),
            "native".to_string(),
            "list files".to_string(),
        ])
        .unwrap();

        assert_eq!(ask_inv.command, Command::Ask);
        assert_eq!(ask_inv.security_preset, SecurityPreset::Turbomode);
        assert_eq!(ask_inv.sandbox_kind, SandboxKind::Native);
        assert_eq!(ask_inv.prompt, "list files");

        let interactive_inv = parse_args(vec![
            "--security-preset".to_string(),
            "turbomode".to_string(),
            "--sandbox".to_string(),
            "native".to_string(),
        ])
        .unwrap();

        assert_eq!(interactive_inv.command, Command::Interactive);
        assert_eq!(interactive_inv.security_preset, SecurityPreset::Turbomode);
        assert_eq!(interactive_inv.sandbox_kind, SandboxKind::Native);
    }

    #[test]
    fn parses_web_search_options() {
        let default_inv = parse_args(vec!["ask".to_string(), "hello".to_string()]).unwrap();
        assert_eq!(default_inv.web_search, None);

        let enabled_inv = parse_args(vec![
            "ask".to_string(),
            "--web-search".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        assert_eq!(enabled_inv.web_search, Some(true));

        let disabled_inv = parse_args(vec![
            "ask".to_string(),
            "--no-web-search".to_string(),
            "hello".to_string(),
        ])
        .unwrap();
        assert_eq!(disabled_inv.web_search, Some(false));
    }

    #[test]
    fn rejects_options_unsupported_by_a_command() {
        assert_eq!(
            parse_args(vec![
                "status".to_string(),
                "--trace".to_string(),
                "trace.jsonl".to_string()
            ])
            .unwrap_err(),
            "--trace is not supported by status, doctor, or sessions"
        );
        assert_eq!(
            parse_args(vec![
                "doctor".to_string(),
                "--trace".to_string(),
                "trace.jsonl".to_string()
            ])
            .unwrap_err(),
            "--trace is not supported by status, doctor, or sessions"
        );
        assert_eq!(
            parse_args(vec![
                "demo".to_string(),
                "--json".to_string(),
                "prompt".to_string()
            ])
            .unwrap_err(),
            "--json is supported only by ask, mentor, status, doctor, and trace"
        );
        let auto_inv = parse_args(vec![
            "ask".to_string(),
            "--auto".to_string(),
            "prompt".to_string(),
        ])
        .unwrap();
        assert!(auto_inv.automatic);
        assert_eq!(auto_inv.prompt, "prompt");
    }
}
