use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;

pub const HELP: &str = "mini-agent — bounded native coding-agent CLI

USAGE:
    mini-agent                         Start the interactive REPL
    mini-agent run [PROMPT]            Run one bounded turn
    mini-agent <COMMAND> --help        Detailed command help

QUICK START:
    mini-agent run \"summarize repo\"  Run one provider-backed turn
    mini-agent                         Continue a conversation interactively

COMMANDS:
    run                                 One-shot/script-friendly turn
    resume, fork                        Durable session management

COMMON OPTIONS:
    --security-preset PRESET            default | full-machine
    --sandbox KIND                      native | docker
    --web-search / --no-web-search      Built-in web search toggle
    --no-tools                          Model-only runtime; disable all tools and extensions
    --auto-approve, -y                  Allow sensitive tools for a non-interactive run
    --json                              Machine-readable output for run

CONFIG:
    OPENAI_API_KEY, OPENAI_MODEL, OPENAI_BASE_URL
    Goal Runtime limits are configured separately by the App Server.

PROJECT:
    GitHub:  https://github.com/civaapple-alt/mini-agent-harness
    Creator: civaapple-alt
    License: MIT

Use `mini-agent help COMMAND` or `mini-agent COMMAND --help` for details.";

pub const REPL_HELP: &str = "mini-agent repl

USAGE:
    mini-agent [--session-id SESSION_ID] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--no-tools]

Starts the interactive REPL. Core keeps an internal bounded loop guard; it is
not a task setting. Durable sessions persist settled checkpoints under
~/.mini-agent/sessions.
Use `/steer <message>` to redirect a running turn at a safe checkpoint.
Plan and Goal workflows, session inspection, and project controls are exposed
through App Server clients such as Studio and the SDK.

OPTIONS:
    --session-id SESSION_ID     Resume this durable session instead of opening a new one
    --security-preset PRESET    Security policy preset: default, full-machine [default: default]
    --sandbox KIND              Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search      Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --no-tools                  Disable all Builtin and extension tools
";

pub const RUN_HELP: &str = "mini-agent run

USAGE:
    mini-agent run [--session-id SESSION_ID] [--auto-approve|-y] [--json] [--trace-jsonl PATH] [--no-tools] [--security-preset PRESET] [--sandbox KIND] [--web-search|--no-web-search] [--] [PROMPT]

Runs one provider-backed turn. If PROMPT is omitted, reads at most 32 KiB
from stdin. Core keeps an internal bounded loop guard; the guard is reported
as runtime protection and is not a user task setting.
On a TTY, tools run with the local automatic approval adapter. When stdin is
not a TTY, sensitive tools fail closed unless --auto-approve (or -y).
Progress is written to stderr and the final result to stdout.

OPTIONS:
    --session-id SESSION_ID      Resume this durable session instead of opening a new one
    --auto-approve, -y           Permit sensitive tools non-interactively (alias: --yes)
    --security-preset PRESET     Security policy preset: default, full-machine [default: default]
    --sandbox KIND               Execution sandbox: native (JobObject/process groups), docker [default: native]
    --web-search, --search       Enable built-in Responses web_search [default: enabled]
    --no-web-search, --no-search Disable built-in Responses web_search
    --no-tools                   Disable all host tools and extension loading
    --json                       Emit a machine-readable final result
    --trace-jsonl PATH           Write a bounded redacted trace; PATH must not exist
";

pub const RESUME_HELP: &str = "mini-agent resume

USAGE:
    mini-agent resume SESSION_ID

Resumes the latest settled checkpoint of a durable session for this workspace.";

pub const FORK_HELP: &str = "mini-agent fork

USAGE:
    mini-agent fork SESSION_ID

Forks a new independent session from the latest settled checkpoint of an existing session.";

pub const VERSION_HELP: &str = "mini-agent version

USAGE:
    mini-agent version
    mini-agent --version";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Repl,
    Run,
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
    pub auto_approve: bool,
    pub no_tools: bool,
    pub session_id: Option<String>,
    pub security_preset: SecurityPreset,
    pub security_preset_explicit: bool,
    pub sandbox_kind: SandboxKind,
    pub sandbox_kind_explicit: bool,
    pub web_search: Option<bool>,
    pub trace_path: Option<std::path::PathBuf>,
    pub help_topic: HelpTopic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTopic {
    Root,
    Repl,
    Run,
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
        Some("run") => {
            args.next();
            Command::Run
        }
        Some("resume") => {
            args.next();
            Command::Resume
        }
        Some("fork") => {
            args.next();
            Command::Fork
        }
        None => Command::Repl,
        Some(other) if other.starts_with('-') => Command::Repl,
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
            auto_approve: false,
            no_tools: false,
            session_id: None,
            security_preset: SecurityPreset::Default,
            security_preset_explicit: false,
            sandbox_kind: SandboxKind::Native,
            sandbox_kind_explicit: false,
            web_search: None,
            trace_path: None,
            help_topic: HelpTopic::Root,
        });
    }

    let mut args = remaining.into_iter();
    let mut prompt = Vec::new();
    let mut json = false;
    let mut auto_approve = false;
    let mut no_tools = false;
    let mut session_id = None;
    let mut security_preset = SecurityPreset::Default;
    let mut security_preset_explicit = false;
    let mut sandbox_kind = SandboxKind::Native;
    let mut sandbox_kind_explicit = false;
    let mut web_search = None;
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
            && (argument == "--auto-approve" || argument == "-y" || argument == "--yes")
        {
            if auto_approve {
                return Err(format!("{argument} may be provided only once"));
            }
            auto_approve = true;
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
    if command == Command::Repl && !prompt.is_empty() {
        return Err("the REPL does not accept a prompt; use `run`".to_string());
    }
    if command == Command::Resume && prompt.len() != 1 {
        return Err("resume requires exactly one SESSION_ID".to_string());
    }
    if command == Command::Fork && prompt.len() != 1 {
        return Err("fork requires exactly one SESSION_ID".to_string());
    }
    if json && command != Command::Run {
        return Err("--json is supported only by run".to_string());
    }
    if auto_approve && command != Command::Run {
        return Err("--auto-approve is supported only by run".to_string());
    }
    if session_id.is_some() && !matches!(command, Command::Run | Command::Repl) {
        return Err("--session-id is supported only by run and the REPL".to_string());
    }
    if trace_path.is_some() && command != Command::Run {
        return Err("--trace-jsonl is supported only by run".to_string());
    }
    if no_tools && !matches!(command, Command::Run | Command::Repl) {
        return Err("--no-tools is supported only by run and the REPL".to_string());
    }
    Ok(Invocation {
        command,
        prompt: prompt.join(" "),
        json,
        auto_approve,
        no_tools,
        session_id,
        security_preset,
        security_preset_explicit,
        sandbox_kind,
        sandbox_kind_explicit,
        web_search,
        trace_path,
        help_topic: HelpTopic::Root,
    })
}

fn help_invocation(help_topic: HelpTopic) -> Invocation {
    Invocation {
        command: Command::Help,
        prompt: String::new(),
        json: false,
        auto_approve: false,
        no_tools: false,
        session_id: None,
        security_preset: SecurityPreset::Default,
        security_preset_explicit: false,
        sandbox_kind: SandboxKind::Native,
        sandbox_kind_explicit: false,
        web_search: None,
        trace_path: None,
        help_topic,
    }
}

fn help_topic(name: &str) -> Result<HelpTopic, String> {
    match name {
        "repl" => Ok(HelpTopic::Repl),
        "run" => Ok(HelpTopic::Run),
        "resume" => Ok(HelpTopic::Resume),
        "fork" => Ok(HelpTopic::Fork),
        "version" => Ok(HelpTopic::Version),
        _ => Err(format!("unknown help topic: {name}")),
    }
}

fn help_topic_for(command: Command) -> HelpTopic {
    match command {
        Command::Repl => HelpTopic::Repl,
        Command::Run => HelpTopic::Run,
        Command::Resume => HelpTopic::Resume,
        Command::Fork => HelpTopic::Fork,
        Command::Version | Command::Help => HelpTopic::Root,
    }
}

pub fn help_text(topic: HelpTopic) -> &'static str {
    match topic {
        HelpTopic::Root => HELP,
        HelpTopic::Repl => REPL_HELP,
        HelpTopic::Run => RUN_HELP,
        HelpTopic::Resume => RESUME_HELP,
        HelpTopic::Fork => FORK_HELP,
        HelpTopic::Version => VERSION_HELP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_repl() {
        let invocation = parse_args(Vec::new()).unwrap();
        assert_eq!(invocation.command, Command::Repl);
        assert_eq!(invocation.prompt, "");
        assert!(!invocation.json);
        assert!(!invocation.auto_approve);
    }

    #[test]
    fn parses_one_shot_run() {
        let invocation = parse_args(vec![
            "run".to_string(),
            "explain".to_string(),
            "the".to_string(),
            "code".to_string(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::Run);
        assert_eq!(invocation.prompt, "explain the code");
    }

    #[test]
    fn run_accepts_script_options_but_not_a_step_budget() {
        let invocation = parse_args(vec![
            "run".to_string(),
            "--json".to_string(),
            "--auto-approve".to_string(),
            "--trace-jsonl".to_string(),
            "trace.jsonl".to_string(),
            "go".to_string(),
        ])
        .unwrap();
        assert!(invocation.json);
        assert!(invocation.auto_approve);
        assert_eq!(invocation.trace_path, Some("trace.jsonl".into()));
        assert_eq!(
            parse_args(vec![
                "run".to_string(),
                "--max-steps".to_string(),
                "50".to_string(),
            ])
            .unwrap_err(),
            "unknown option: --max-steps"
        );
    }

    #[test]
    fn rejects_removed_commands_and_aliases() {
        assert_eq!(
            parse_args(vec!["ask".to_string()]).unwrap_err(),
            "unknown command: ask"
        );
        assert_eq!(
            parse_args(vec!["auto".to_string()]).unwrap_err(),
            "unknown command: auto"
        );
        assert_eq!(
            parse_args(vec!["run".to_string(), "--auto".to_string()]).unwrap_err(),
            "unknown option: --auto"
        );
    }

    #[test]
    fn rejects_removed_turbomode_alias() {
        assert_eq!(
            parse_args(vec![
                "run".to_string(),
                "--security-preset".to_string(),
                "turbomode".to_string(),
            ])
            .unwrap_err(),
            "unknown security preset: turbomode"
        );
    }

    #[test]
    fn parses_durable_commands_and_help() {
        let resume = parse_args(vec!["resume".to_string(), "s-12345678".to_string()]).unwrap();
        assert_eq!(resume.command, Command::Resume);
        assert_eq!(resume.prompt, "s-12345678");
        let help = parse_args(vec!["help".to_string(), "run".to_string()]).unwrap();
        assert_eq!(help.command, Command::Help);
        assert_eq!(help.help_topic, HelpTopic::Run);
    }

    #[test]
    fn parses_security_and_web_options() {
        let invocation = parse_args(vec![
            "run".to_string(),
            "--security-preset".to_string(),
            "full-machine".to_string(),
            "--sandbox".to_string(),
            "native".to_string(),
            "--no-web-search".to_string(),
            "--no-tools".to_string(),
            "list files".to_string(),
        ])
        .unwrap();
        assert_eq!(invocation.security_preset, SecurityPreset::FullMachine);
        assert!(invocation.security_preset_explicit);
        assert!(invocation.sandbox_kind_explicit);
        assert_eq!(invocation.web_search, Some(false));
        assert!(invocation.no_tools);
    }
}
