use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;

pub const HELP: &str = "mini-agent — bounded native coding-agent CLI

USAGE:
    mini-agent                         Interactive session
    mini-agent auto [PROMPT]           Autonomous session (REPL when omitted)
    mini-agent ask [PROMPT]             One-shot/script-friendly turn
    mini-agent <COMMAND> --help         Detailed command help

QUICK START:
    mini-agent ask \"summarize repo\"    Run one provider-backed turn
    mini-agent auto                     Start the interactive copilot

COMMANDS:
    resume, fork                        Durable session management

COMMON OPTIONS:
    --security-preset PRESET            default | turbomode | full-machine
    --sandbox KIND                      native | docker
    --web-search / --no-web-search      Built-in web search toggle
    --no-tools                          Model-only runtime; disable all tools and extensions
    --auto-approve, -y                  Allow sensitive tools in non-TTY ask/run
    --json                              Machine-readable output

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
    mini-agent [--session-id SESSION_ID] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--no-tools]

Starts the interactive REPL. Tools run without per-step approval; shell is protected by the sandbox.
Interactive, one-shot ask, and auto sessions persist settled checkpoints under ~/.mini-agent/sessions.
Use `/auto` to enter copilot mode; `/auto off` restores per-action prompts.
Plan and Goal workflows are exposed through App Server clients such as Studio and the SDK; this REPL stays focused on core turn execution.

OPTIONS:
    --session-id SESSION_ID     Resume this durable session instead of opening a new one
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --no-tools                   Disable all Builtin and extension tools
";

pub const RESUME_HELP: &str = "mini-agent resume

USAGE:
    mini-agent resume SESSION_ID

Resumes the latest settled checkpoint of a durable session for this workspace.";

pub const FORK_HELP: &str = "mini-agent fork

USAGE:
    mini-agent fork SESSION_ID

Forks a new independent session from the latest settled checkpoint of an existing session.";

pub const ASK_HELP: &str = "mini-agent ask

USAGE:
    mini-agent ask [--session-id SESSION_ID] [--auto-approve|-y] [--max-steps N] [--json] [--trace-jsonl PATH] [--no-tools] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--] [PROMPT]

Runs one script-facing turn (8 steps by default, no compaction). If PROMPT is omitted, reads at most 32 KiB from stdin.
On a TTY, tools run without per-step approval. When stdin is not a TTY, sensitive tools fail closed unless `--auto-approve` (or `-y`).
Progress is written to stderr and the final result to stdout.

OPTIONS:
    --session-id SESSION_ID      Resume this durable session instead of opening a new one
    --auto-approve, -y           Permit sensitive tools non-interactively (aliases: --yes, --auto)
    --max-steps N                Cap model steps for this turn (default: 8; 0 means unlimited)
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --no-tools                   Disable all host tools and extension loading
    --json                       Emit a machine-readable final result
    --trace-jsonl PATH            Write a bounded redacted trace; PATH must not exist
";

pub const AUTO_HELP: &str = "mini-agent auto

USAGE:
    mini-agent auto [--session-id SESSION_ID] [--security-preset PRESET] [--sandbox KIND] [--no-tools] [--web-search|--no-web-search] [--] [PROMPT]

Unattended copilot: runs continuous model/tool cycles without per-step approval, unlimited steps (unless capped by MINI_AGENT_MAX_STEPS), and automatic context compaction that preserves recent tool work.
With a prompt, runs one autonomous copilot turn to completion.
Without a prompt, starts the interactive REPL in copilot mode.

OPTIONS:
    --session-id SESSION_ID      Resume this durable session instead of opening a new one
    --security-preset PRESET     Security policy preset: default, turbomode, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --no-tools                   Disable all host tools and extension loading
";

pub const VERSION_HELP: &str = "mini-agent version

USAGE:
    mini-agent version
    mini-agent --version";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Interactive,
    Ask,
    Auto,
    Resume,
    Fork,
    Help,
    Version,
}

#[derive(Debug)]
pub struct Invocation {
    pub command: Command,
    pub prompt: String,
    pub json: bool,
    pub automatic: bool,
    pub no_tools: bool,
    pub session_id: Option<String>,
    pub security_preset: SecurityPreset,
    pub security_preset_explicit: bool,
    pub sandbox_kind: SandboxKind,
    pub sandbox_kind_explicit: bool,
    pub web_search: Option<bool>,
    pub max_steps: Option<usize>,
    pub trace_path: Option<std::path::PathBuf>,
    pub help_topic: HelpTopic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTopic {
    Root,
    Interactive,
    Ask,
    Auto,
    Resume,
    Fork,
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
            json: false,
            automatic: false,
            no_tools: false,
            session_id: None,
            security_preset: SecurityPreset::Default,
            security_preset_explicit: false,
            sandbox_kind: SandboxKind::Native,
            sandbox_kind_explicit: false,
            web_search: None,
            max_steps: None,
            trace_path: None,
            help_topic: HelpTopic::Root,
        });
    }

    let mut args = remaining.into_iter();
    let mut prompt = Vec::new();
    let mut json = false;
    let mut automatic = false;
    let mut no_tools = false;
    let mut session_id = None;
    let mut security_preset = SecurityPreset::Default;
    let mut security_preset_explicit = false;
    let mut sandbox_kind = SandboxKind::Native;
    let mut sandbox_kind_explicit = false;
    let mut web_search = None;
    let mut max_steps = None;
    let mut trace_path = None;
    let mut options = true;
    while let Some(argument) = args.next() {
        if options && argument == "--" {
            options = false;
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
        } else if options && argument == "--no-tools" {
            if no_tools {
                return Err("--no-tools may be provided only once".to_string());
            }
            no_tools = true;
        } else if options && (argument == "--session" || argument == "--session-id") {
            if session_id.is_some() {
                return Err(format!("{argument} may be provided only once"));
            }
            session_id = Some(
                args.next()
                    .ok_or_else(|| format!("{argument} requires a session ID"))?,
            );
        } else if options && argument == "--security-preset" {
            let value = args
                .next()
                .ok_or_else(|| "--security-preset requires a preset name".to_string())?;
            security_preset = SecurityPreset::parse(&value)?;
            security_preset_explicit = true;
        } else if options && argument == "--sandbox" {
            let value = args
                .next()
                .ok_or_else(|| "--sandbox requires a sandbox kind".to_string())?;
            sandbox_kind = SandboxKind::parse(&value)?;
            sandbox_kind_explicit = true;
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
        } else if options && argument == "--trace-jsonl" {
            if trace_path.is_some() {
                return Err("--trace-jsonl may be provided only once".to_string());
            }
            trace_path =
                Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                    "--trace-jsonl requires a file path".to_string()
                })?));
        } else if options && argument.starts_with('-') {
            return Err(format!("unknown option: {argument}"));
        } else {
            prompt.push(argument);
        }
    }
    if matches!(command, Command::Interactive) && !prompt.is_empty() {
        return Err("interactive mode does not accept a prompt; use `ask`".to_string());
    }
    if command == Command::Resume && prompt.len() != 1 {
        return Err("resume requires exactly one SESSION_ID".to_string());
    }
    if command == Command::Fork && prompt.len() != 1 {
        return Err("fork requires exactly one SESSION_ID".to_string());
    }
    if json && !matches!(command, Command::Ask) {
        return Err("--json is supported only by ask".to_string());
    }
    if automatic && !matches!(command, Command::Ask) {
        return Err("--auto-approve is supported only by ask".to_string());
    }
    if session_id.is_some()
        && !matches!(command, Command::Ask | Command::Interactive | Command::Auto)
    {
        return Err("--session-id is supported only by ask, auto, and interactive".to_string());
    }
    if max_steps.is_some() && !matches!(command, Command::Ask) {
        return Err("--max-steps is supported only by ask".to_string());
    }
    if trace_path.is_some() && !matches!(command, Command::Ask) {
        return Err("--trace-jsonl is supported only by ask".to_string());
    }
    if no_tools && !matches!(command, Command::Interactive | Command::Ask | Command::Auto) {
        return Err("--no-tools is supported only by interactive, ask, and auto".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        json,
        automatic,
        no_tools,
        session_id,
        security_preset,
        security_preset_explicit,
        sandbox_kind,
        sandbox_kind_explicit,
        web_search,
        max_steps,
        trace_path,
        help_topic: HelpTopic::Root,
    })
}

fn help_invocation(help_topic: HelpTopic) -> Invocation {
    Invocation {
        command: Command::Help,
        prompt: String::new(),
        json: false,
        automatic: false,
        no_tools: false,
        session_id: None,
        security_preset: SecurityPreset::Default,
        security_preset_explicit: false,
        sandbox_kind: SandboxKind::Native,
        sandbox_kind_explicit: false,
        web_search: None,
        max_steps: None,
        trace_path: None,
        help_topic,
    }
}

fn help_topic(name: &str) -> Result<HelpTopic, String> {
    match name {
        "interactive" | "repl" => Ok(HelpTopic::Interactive),
        "ask" => Ok(HelpTopic::Ask),
        "auto" => Ok(HelpTopic::Auto),
        "resume" => Ok(HelpTopic::Resume),
        "fork" => Ok(HelpTopic::Fork),
        "version" => Ok(HelpTopic::Version),
        _ => Err(format!("unknown help topic: {name}")),
    }
}

fn help_topic_for(command: Command) -> HelpTopic {
    match command {
        Command::Interactive => HelpTopic::Interactive,
        Command::Ask => HelpTopic::Ask,
        Command::Auto => HelpTopic::Auto,
        Command::Resume => HelpTopic::Resume,
        Command::Fork => HelpTopic::Fork,
        Command::Version => HelpTopic::Version,
        Command::Help => HelpTopic::Root,
    }
}

pub fn help_text(topic: HelpTopic) -> &'static str {
    match topic {
        HelpTopic::Root => HELP,
        HelpTopic::Interactive => INTERACTIVE_HELP,
        HelpTopic::Ask => ASK_HELP,
        HelpTopic::Auto => AUTO_HELP,
        HelpTopic::Resume => RESUME_HELP,
        HelpTopic::Fork => FORK_HELP,
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
        assert!(!invocation.json);
        assert!(!invocation.automatic);
        assert_eq!(invocation.security_preset, SecurityPreset::Default);
        assert!(!invocation.security_preset_explicit);
        assert_eq!(invocation.sandbox_kind, SandboxKind::Native);
        assert!(!invocation.sandbox_kind_explicit);
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
        assert!(!invocation.json);
        assert!(!invocation.automatic);
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
        assert!(!invocation.json);
        assert!(!invocation.automatic);
    }

    #[test]
    fn parses_durable_session_commands() {
        let resume = parse_args(vec!["resume".to_string(), "s-12345678".to_string()]).unwrap();
        assert_eq!(resume.command, Command::Resume);
        assert_eq!(resume.prompt, "s-12345678");

        let auto_repl = parse_args(vec!["auto".to_string()]).unwrap();
        assert_eq!(auto_repl.command, Command::Auto);
        assert_eq!(
            parse_args(vec!["auto".to_string(), "--ephemeral".to_string()]).unwrap_err(),
            "unknown option: --ephemeral"
        );

        assert_eq!(
            parse_args(vec!["resume".to_string()]).unwrap_err(),
            "resume requires exactly one SESSION_ID"
        );
    }

    #[test]
    fn parses_script_ask_options() {
        let invocation = parse_args(vec![
            "ask".to_string(),
            "--json".to_string(),
            "--auto-approve".to_string(),
            "explain".to_string(),
            "the".to_string(),
            "code".to_string(),
        ])
        .unwrap();

        assert_eq!(invocation.command, Command::Ask);
        assert_eq!(invocation.prompt, "explain the code");
        assert!(invocation.json);
        assert!(invocation.automatic);
        assert!(!invocation.no_tools);
        assert_eq!(invocation.max_steps, None);

        let restricted = parse_args(vec![
            "ask".to_string(),
            "--no-tools".to_string(),
            "explain".to_string(),
        ])
        .unwrap();
        assert!(restricted.no_tools);

        let stepped = parse_args(vec![
            "ask".to_string(),
            "--max-steps".to_string(),
            "50".to_string(),
            "go".to_string(),
        ])
        .unwrap();
        assert_eq!(stepped.max_steps, Some(50));

        let traced = parse_args(vec![
            "ask".to_string(),
            "--trace-jsonl".to_string(),
            "trace.jsonl".to_string(),
            "go".to_string(),
        ])
        .unwrap();
        assert_eq!(
            traced.trace_path,
            Some(std::path::PathBuf::from("trace.jsonl"))
        );
        assert_eq!(
            parse_args(vec![
                "auto".to_string(),
                "--trace-jsonl".to_string(),
                "trace.jsonl".to_string(),
            ])
            .unwrap_err(),
            "--trace-jsonl is supported only by ask"
        );
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
        assert!(ask_inv.security_preset_explicit);
        assert_eq!(ask_inv.sandbox_kind, SandboxKind::Native);
        assert!(ask_inv.sandbox_kind_explicit);
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
        assert!(interactive_inv.security_preset_explicit);
        assert_eq!(interactive_inv.sandbox_kind, SandboxKind::Native);
        assert!(interactive_inv.sandbox_kind_explicit);
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
            parse_args(vec!["--json".to_string()]).unwrap_err(),
            "--json is supported only by ask"
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
