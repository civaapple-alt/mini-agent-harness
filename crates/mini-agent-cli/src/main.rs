mod args;
mod observer;
mod repl;
#[path = "ask.rs"]
mod run;

use std::process::ExitCode;

use args::Command;
use args::HelpTopic;
use args::help_text;
use args::parse_args;
use mini_agent_app_server::SessionRequest;

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
        Command::Repl => {
            let request = invocation
                .session_id
                .map_or(SessionRequest::New, SessionRequest::Resume);
            repl::run(
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
        Command::Run => {
            let request = match invocation.session_id {
                Some(id) => SessionRequest::Named(id),
                None => SessionRequest::New,
            };
            run::run(
                invocation.prompt,
                invocation.json,
                invocation.auto_approve,
                invocation.no_tools,
                invocation.security_preset,
                invocation.security_preset_explicit,
                invocation.sandbox_kind,
                invocation.sandbox_kind_explicit,
                invocation.web_search,
                request,
                invocation.trace_path,
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
