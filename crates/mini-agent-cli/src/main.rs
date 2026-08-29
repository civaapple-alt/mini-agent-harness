mod args;
mod ask;
mod repl;

use serde_json::json;
use std::env;
use std::process::ExitCode;

use args::Command;
use args::HelpTopic;
use args::help_text;
use args::parse_args;
use mini_agent_app_server::SessionRequest;
use mini_agent_app_server::frontend::ApprovalMode;
use mini_agent_app_server::frontend::RuntimeConfig;
use mini_agent_app_server::frontend::RuntimeProfile;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;
use mini_agent_app_server::frontend::load_workspace_profile;

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
            let request = invocation
                .session_id
                .map_or(SessionRequest::New, SessionRequest::Resume);
            repl::run(
                ApprovalMode::Automatic,
                false,
                invocation.no_tools,
                request,
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
            )
            .await
        }
        Command::Demo => run_demo(invocation.prompt).await,
        Command::Run | Command::Ask => {
            let request = match invocation.session_id {
                Some(id) => SessionRequest::Named(id),
                None => SessionRequest::New,
            };
            ask::run(
                invocation.prompt,
                invocation.json,
                invocation.automatic,
                invocation.no_tools,
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
                request,
                invocation.max_steps,
            )
            .await
        }
        Command::Auto if invocation.prompt.is_empty() => {
            let request = if let Some(session_id) = invocation.session_id {
                SessionRequest::Resume(session_id)
            } else {
                SessionRequest::New
            };
            repl::run(
                ApprovalMode::Automatic,
                true,
                invocation.no_tools,
                request,
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
            )
            .await
        }
        Command::Auto => {
            let request = if let Some(session_id) = invocation.session_id {
                SessionRequest::Resume(session_id)
            } else {
                SessionRequest::New
            };
            run_auto(
                invocation.prompt,
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
                request,
                invocation.no_tools,
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
                ApprovalMode::Automatic,
                false,
                invocation.no_tools,
                SessionRequest::Resume(invocation.prompt),
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
            )
            .await
        }
        Command::Fork => {
            repl::run(
                ApprovalMode::Automatic,
                false,
                invocation.no_tools,
                SessionRequest::Fork(invocation.prompt),
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
            )
            .await
        }
        Command::Sessions => run_sessions(),
        Command::Mentor => {
            mini_agent_app_server::mentor::run(invocation.prompt, invocation.json).await
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
    let profile =
        match load_workspace_profile(&config.workspace(), RuntimeProfile::interactive_default()) {
            Ok(profile) => profile,
            Err(error) => {
                eprintln!("error: {error}");
                if json {
                    println!("{}", json!({"error": error}));
                }
                return ExitCode::from(2);
            }
        };
    let manifest = profile.manifest();
    if json {
        let mut status = config.status_json();
        status["capabilities"] =
            serde_json::to_value(&manifest).expect("capability manifest must be serializable");
        println!(
            "{}",
            serde_json::to_string_pretty(&status).expect("status must be serializable")
        );
    } else {
        for line in config.status_lines() {
            println!("{line}");
        }
        println!(
            "capabilities: profile={} enabled={}",
            manifest.profile,
            manifest.enabled.join(",")
        );
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
    match mini_agent_app_server::local::list_sessions(&workspace) {
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

async fn run_demo(prompt: String) -> ExitCode {
    match mini_agent_app_server::demo::run(prompt).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_auto(
    prompt: String,
    preset: SecurityPreset,
    security_preset_explicit: bool,
    sandbox: SandboxKind,
    sandbox_kind_explicit: bool,
    web_search_override: Option<bool>,
    session_request: SessionRequest,
    no_tools: bool,
) -> ExitCode {
    crate::ask::run(
        prompt,
        false,
        true,
        no_tools,
        preset,
        security_preset_explicit,
        sandbox,
        sandbox_kind_explicit,
        web_search_override,
        session_request,
        None,
    )
    .await
}
