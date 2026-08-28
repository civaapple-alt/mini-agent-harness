mod args;
mod ask;
mod repl;

use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use args::Command;
use args::HelpTopic;
use args::help_text;
use args::parse_args;
use host::ApprovalMode;
use host::RuntimeConfig;
use host::SandboxKind;
use host::SecurityPreset;
use host::SessionRequest;
use mini_agent_host as host;

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
            eprintln!("{}", help_text(HelpTopic::Root));
            return ExitCode::from(2);
        }
    };
    match invocation.command {
        Command::Interactive => {
            let request = if invocation.persist && !invocation.ephemeral {
                SessionRequest::New
            } else {
                SessionRequest::Disabled
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
            let request = match invocation.session_id {
                Some(id) => SessionRequest::Named(id),
                None if invocation.persist && !invocation.ephemeral => SessionRequest::New,
                None => SessionRequest::Disabled,
            };
            ask::run(
                invocation.prompt,
                invocation.trace,
                invocation.json,
                invocation.automatic,
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
                request,
                invocation.max_steps,
            )
            .await
        }
        Command::Auto if invocation.prompt.is_empty() => {
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else if let Some(session_id) = invocation.session_id {
                SessionRequest::Resume(session_id)
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
        Command::Auto => {
            let request = if invocation.ephemeral {
                SessionRequest::Disabled
            } else if let Some(session_id) = invocation.session_id {
                SessionRequest::Resume(session_id)
            } else {
                SessionRequest::New
            };
            run_auto(
                invocation.prompt,
                invocation.trace,
                invocation.security_preset,
                invocation.sandbox_kind,
                invocation.web_search,
                request,
            )
            .await
        }
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
        Command::Mentor => {
            mini_agent_app_server::mentor::run(invocation.prompt, invocation.trace, invocation.json)
                .await
        }
        Command::TraceReplay => {
            host::trace::replay(std::path::Path::new(&invocation.prompt), invocation.json)
        }
        Command::TraceSummary => {
            host::trace::summary(std::path::Path::new(&invocation.prompt), invocation.json)
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
    match host::session::list(&workspace) {
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
    match mini_agent_app_server::demo::run(prompt, trace).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_auto(
    prompt: String,
    trace: Option<PathBuf>,
    preset: SecurityPreset,
    sandbox: SandboxKind,
    web_search_override: Option<bool>,
    session_request: SessionRequest,
) -> ExitCode {
    crate::ask::run(
        prompt,
        trace,
        false,
        true,
        preset,
        sandbox,
        web_search_override,
        session_request,
        None,
    )
    .await
}
