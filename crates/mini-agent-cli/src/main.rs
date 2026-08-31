mod args;
mod ask;
mod repl;

use std::process::ExitCode;

use args::Command;
use args::HelpTopic;
use args::help_text;
use args::parse_args;
use mini_agent_app_server::SessionRequest;
use mini_agent_app_server::frontend::ApprovalMode;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;

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
        Command::Ask => {
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
                invocation.trace_path,
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
        None,
    )
    .await
}
