use mini_agent_app_server::SessionRequest;
use mini_agent_app_server::frontend::ApprovalController;
use mini_agent_app_server::frontend::ApprovalMode;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;
use mini_agent_app_server::frontend::TurnStatus;
use mini_agent_app_server::frontend::observer::RunObserver;
use mini_agent_app_server::frontend::observer::ScriptFormat;
use mini_agent_app_server::frontend::observer::print_final_answer;
use mini_agent_app_server::frontend::print_auto_warning;
use mini_agent_app_server::local::LocalRuntimeRequest;
use serde_json::json;
use std::io;
use std::io::IsTerminal;
use std::io::Read;
use std::process::ExitCode;

const MAX_STDIN_PROMPT_BYTES: usize = 32 * 1024;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    prompt: String,
    json_output: bool,
    automatic: bool,
    no_tools: bool,
    preset: SecurityPreset,
    security_preset_explicit: bool,
    sandbox: SandboxKind,
    sandbox_kind_explicit: bool,
    web_search_override: Option<bool>,
    session_request: SessionRequest,
    max_steps: Option<usize>,
) -> ExitCode {
    let prompt = match resolve_prompt(prompt) {
        Ok(prompt) => prompt,
        Err(error) => return preflight_error(json_output, &error),
    };
    let tty = io::stdin().is_terminal();
    let mode = if automatic || tty {
        print_auto_warning();
        ApprovalMode::Automatic
    } else {
        ApprovalMode::Interactive
    };
    let launch = match mini_agent_app_server::local::prepare(LocalRuntimeRequest {
        automatic,
        no_tools,
        security_preset: preset,
        security_preset_explicit,
        sandbox_kind: sandbox,
        sandbox_kind_explicit,
        web_search_override,
        session_request,
        max_steps,
    }) {
        Ok(launch) => launch,
        Err(error) => return preflight_error(json_output, &error),
    };
    let approval = ApprovalController::with_preset(mode, launch.security_preset());
    let mut runtime = match launch.start(approval).await {
        Ok(runtime) => runtime,
        Err(error) => return preflight_error(json_output, &error),
    };

    let mut observer = if automatic && !json_output {
        RunObserver::new()
    } else {
        let format = if json_output {
            ScriptFormat::Json
        } else {
            ScriptFormat::Text
        };
        RunObserver::for_script(format)
    };

    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let result = runtime.run_turn(prompt.clone(), &mut observer).await;
    let session_id = runtime
        .client_mut()
        .session_info()
        .await
        .ok()
        .flatten()
        .map(|session| session.session_id);

    match result {
        Ok(outcome) if !matches!(outcome.status, TurnStatus::StepLimit | TurnStatus::Failed) => {
            observer.finish();
            let _ = runtime.record_turn(started_at_ms, &prompt, &outcome);
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 0,
                        "model": runtime.model_name(),
                        "steps": outcome.steps,
                        "session_id": session_id,
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json(),
                        "capabilities": runtime.capability_manifest()
                    })
                );
            } else if !observer.assistant_displayed() {
                print_final_answer(outcome.final_text.as_deref().unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            observer.finish();
            let error = format!(
                "stopped after {} model steps without completing",
                outcome.steps
            );
            let mut outcome = outcome;
            outcome.error = Some(error.clone());
            let _ = runtime.record_turn(started_at_ms, &prompt, &outcome);
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 1,
                        "model": runtime.model_name(),
                        "steps": outcome.steps,
                        "session_id": session_id,
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json(),
                        "error": error,
                        "capabilities": runtime.capability_manifest()
                    })
                );
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            observer.finish();
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": "",
                        "exit_code": 1,
                        "model": runtime.model_name(),
                        "steps": 0,
                        "session_id": session_id,
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json(),
                        "error": error.to_string(),
                        "capabilities": runtime.capability_manifest()
                    })
                );
            }
            ExitCode::FAILURE
        }
    }
}

fn resolve_prompt(prompt: String) -> Result<String, String> {
    let prompt = if prompt.is_empty() {
        if io::stdin().is_terminal() {
            return Err("prompt is required as an argument or on stdin".to_string());
        }
        let mut prompt = String::new();
        io::stdin()
            .take(MAX_STDIN_PROMPT_BYTES as u64 + 1)
            .read_to_string(&mut prompt)
            .map_err(|error| format!("cannot read prompt from stdin: {error}"))?;
        if prompt.len() > MAX_STDIN_PROMPT_BYTES {
            return Err(format!(
                "stdin prompt exceeds {MAX_STDIN_PROMPT_BYTES} byte limit"
            ));
        }
        prompt
    } else {
        prompt
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        Err("prompt must not be empty".to_string())
    } else {
        Ok(prompt)
    }
}

fn preflight_error(json_output: bool, error: &str) -> ExitCode {
    eprintln!("error: {error}");
    if json_output {
        println!(
            "{}",
            json!({
                "output": "",
                "exit_code": 2,
                "model": "",
                "steps": 0,
                "usage": {
                    "requests": 0,
                    "input_tokens": 0,
                    "cached_input_tokens": 0,
                    "output_tokens": 0
                },
                "tool_calls": [],
                "error": error
            })
        );
    }
    ExitCode::from(2)
}
