use crate::build_openai_harness_with;
use crate::config::RuntimeConfig;
use crate::harness_config;
use crate::observer::OutputTarget;
use crate::observer::RunObserver;
use crate::print_auto_warning;
use crate::workspace::ApprovalController;
use crate::workspace::ApprovalMode;
use mini_codex_core::StopReason;
use serde_json::json;
use std::io;
use std::io::IsTerminal;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

const MAX_STDIN_PROMPT_BYTES: usize = 32 * 1024;

pub async fn run(
    prompt: String,
    trace: Option<PathBuf>,
    json_output: bool,
    automatic: bool,
) -> ExitCode {
    let prompt = match resolve_prompt(prompt) {
        Ok(prompt) => prompt,
        Err(error) => return preflight_error(json_output, &error),
    };
    let runtime_config = match RuntimeConfig::load() {
        Ok(config) => config,
        Err(error) => return preflight_error(json_output, &error),
    };
    let model_name = runtime_config.model().unwrap_or_default().to_string();
    let mode = if automatic {
        print_auto_warning();
        ApprovalMode::Automatic
    } else {
        ApprovalMode::Interactive
    };
    let approval = ApprovalController::new(mode);
    let mut harness =
        match build_openai_harness_with(&runtime_config, approval, harness_config(mode)) {
            Ok(harness) => harness,
            Err(error) => return preflight_error(json_output, &error),
        };
    let mut observer = match RunObserver::with_target(trace, OutputTarget::Stderr) {
        Ok(observer) => observer,
        Err(error) => {
            return preflight_error(json_output, &format!("cannot create trace: {error}"));
        }
    };

    match harness.run(prompt, &mut observer).await {
        Ok(outcome) if outcome.stop_reason != StopReason::StepLimit => {
            observer.finish();
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 0,
                        "model": model_name,
                        "steps": outcome.steps,
                        "usage": observer.stats_json(),
                        "tool_calls": observer.tool_calls_json()
                    })
                );
            } else {
                println!("{}", outcome.final_text);
            }
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            observer.finish();
            let error = format!(
                "stopped after {} model steps without completing",
                outcome.steps
            );
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": outcome.final_text,
                        "exit_code": 1,
                        "model": model_name,
                        "steps": outcome.steps,
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
            eprintln!("error: {error}");
            if json_output {
                println!(
                    "{}",
                    json!({
                        "output": "",
                        "exit_code": 1,
                        "model": model_name,
                        "steps": 0,
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
