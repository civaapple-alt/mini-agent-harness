use crate::config::RuntimeConfig;
use crate::harness_config;
use crate::observer::RunObserver;
use crate::observer::ScriptFormat;
use crate::observer::print_final_answer;
use crate::prepare_openai_harness;
use crate::print_auto_warning;
use crate::sandbox::SandboxKind;
use crate::security::SecurityPreset;
use crate::session::SessionRequest;
use crate::session::SessionStore;
use crate::session::TurnCommit;
use crate::session::TurnStatus;
use crate::workspace::ApprovalController;
use crate::workspace::ApprovalMode;
use mini_agent_core::StopReason;
use serde_json::json;
use std::io;
use std::io::IsTerminal;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

const MAX_STDIN_PROMPT_BYTES: usize = 32 * 1024;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    prompt: String,
    trace: Option<PathBuf>,
    json_output: bool,
    automatic: bool,
    preset: SecurityPreset,
    sandbox: SandboxKind,
    web_search_override: Option<bool>,
    session_request: SessionRequest,
) -> ExitCode {
    let prompt = match resolve_prompt(prompt) {
        Ok(prompt) => prompt,
        Err(error) => return preflight_error(json_output, &error),
    };
    let mut runtime_config = match RuntimeConfig::load() {
        Ok(config) => config,
        Err(error) => return preflight_error(json_output, &error),
    };
    if let Some(enabled) = web_search_override {
        runtime_config = runtime_config.with_web_search(enabled);
    }
    let model_name = runtime_config.model().unwrap_or_default().to_string();
    let tty = io::stdin().is_terminal();
    let mode = if automatic || tty {
        print_auto_warning();
        ApprovalMode::Automatic
    } else {
        ApprovalMode::Interactive
    };
    let approval = ApprovalController::with_preset(mode, preset);
    let mut harness =
        match prepare_openai_harness(&runtime_config, approval, harness_config(false), sandbox) {
            Ok(build) => build.harness,
            Err(error) => return preflight_error(json_output, &error),
        };

    let mut opened_session = match session_request {
        SessionRequest::Disabled => None,
        other => match SessionStore::open(&runtime_config.workspace(), other) {
            Ok(opened) => {
                if opened.resumed {
                    let _ = harness.restore_history(opened.messages.clone());
                }
                Some(opened)
            }
            Err(error) => {
                return preflight_error(json_output, &format!("cannot open session: {error}"));
            }
        },
    };

    let format = if json_output {
        ScriptFormat::Json
    } else {
        ScriptFormat::Text
    };
    let mut observer = match RunObserver::for_script(trace, format) {
        Ok(observer) => observer,
        Err(error) => {
            return preflight_error(json_output, &format!("cannot create trace: {error}"));
        }
    };

    let started_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let result = harness.run(prompt.clone(), &mut observer).await;

    match result {
        Ok(outcome) if outcome.stop_reason != StopReason::StepLimit => {
            observer.finish();
            if let Some(ref mut session) = opened_session {
                let _ = session.store.record_turn(TurnCommit {
                    started_at_ms,
                    prompt: &prompt,
                    status: TurnStatus::Completed,
                    steps: outcome.steps,
                    error: None,
                    messages: harness.messages(),
                    checkpoint: harness.messages(),
                });
            }
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 0,
                        "model": model_name,
                        "steps": outcome.steps,
                        "session_id": opened_session.as_ref().map(|s| s.store.session_id()),
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json()
                    })
                );
            } else if !observer.assistant_displayed() {
                print_final_answer(&outcome.final_text);
            }
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            observer.finish();
            let error = format!(
                "stopped after {} model steps without completing",
                outcome.steps
            );
            if let Some(ref mut session) = opened_session {
                let _ = session.store.record_turn(TurnCommit {
                    started_at_ms,
                    prompt: &prompt,
                    status: TurnStatus::StepLimit,
                    steps: outcome.steps,
                    error: Some(&error),
                    messages: harness.messages(),
                    checkpoint: harness.messages(),
                });
            }
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 1,
                        "model": model_name,
                        "steps": outcome.steps,
                        "session_id": opened_session.as_ref().map(|s| s.store.session_id()),
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json(),
                        "error": error
                    })
                );
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            observer.finish();
            if let Some(ref mut session) = opened_session {
                let err_str = error.to_string();
                let _ = session.store.record_turn(TurnCommit {
                    started_at_ms,
                    prompt: &prompt,
                    status: TurnStatus::Failed,
                    steps: 0,
                    error: Some(&err_str),
                    messages: harness.messages(),
                    checkpoint: harness.messages(),
                });
            }
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": "",
                        "exit_code": 1,
                        "model": model_name,
                        "steps": 0,
                        "session_id": opened_session.as_ref().map(|s| s.store.session_id()),
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json(),
                        "error": error.to_string()
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
