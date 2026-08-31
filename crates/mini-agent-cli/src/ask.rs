use mini_agent_app_server::JsonlTrace;
use mini_agent_app_server::SessionRequest;
use mini_agent_app_server::frontend::ApprovalController;
use mini_agent_app_server::frontend::ApprovalMode;
use mini_agent_app_server::frontend::EventEnvelope;
use mini_agent_app_server::frontend::EventSink;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPreset;
use mini_agent_app_server::frontend::TurnStatus;
use mini_agent_app_server::frontend::observer::RunObserver;
use mini_agent_app_server::frontend::observer::ScriptFormat;
use mini_agent_app_server::frontend::observer::print_final_answer;
use mini_agent_app_server::frontend::print_auto_warning;
use mini_agent_app_server::local::LocalRuntimeRequest;
use serde_json::json;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::IsTerminal;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
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
    trace_path: Option<PathBuf>,
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

    let format = if json_output {
        ScriptFormat::Json
    } else {
        ScriptFormat::Text
    };
    let mut observer = match CliObserver::new(automatic, format, trace_path.as_deref()) {
        Ok(observer) => observer,
        Err(error) => return preflight_error(json_output, &error),
    };

    let result = runtime
        .client_mut()
        .run_turn(prompt.clone(), &mut observer)
        .await;
    let session_id = runtime
        .client_mut()
        .session_info()
        .await
        .ok()
        .flatten()
        .map(|session| session.session_id);
    if let Err(error) = observer.finish() {
        let error = format!("trace export failed: {error}");
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
                    "error": error,
                    "capabilities": runtime.capability_manifest()
                })
            );
        }
        return ExitCode::FAILURE;
    }

    match result {
        Ok(outcome) if !matches!(outcome.status, TurnStatus::StepLimit | TurnStatus::Failed) => {
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
            let error = format!(
                "stopped after {} model steps without completing",
                outcome.steps
            );
            let mut outcome = outcome;
            outcome.error = Some(error.clone());
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

struct CliObserver {
    output: RunObserver,
    trace: Option<JsonlTrace<File>>,
}

impl CliObserver {
    fn new(
        automatic: bool,
        format: ScriptFormat,
        trace_path: Option<&Path>,
    ) -> Result<Self, String> {
        let output = if automatic && !matches!(format, ScriptFormat::Json) {
            RunObserver::new()
        } else {
            RunObserver::for_script(format)
        };
        let trace = trace_path
            .map(open_trace)
            .transpose()?
            .map(|file| JsonlTrace::new(format!("cli-{}", std::process::id()), file))
            .transpose()
            .map_err(|error| format!("cannot initialize trace: {error}"))?;
        Ok(Self { output, trace })
    }

    fn finish(&mut self) -> Result<(), String> {
        self.output.finish();
        if let Some(trace) = self.trace.take() {
            trace
                .finish()
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        }
    }

    fn stats_json(&self) -> serde_json::Value {
        self.output.stats_json()
    }

    fn tool_calls_json(&self) -> &[serde_json::Value] {
        self.output.tool_calls_json()
    }

    fn assistant_displayed(&self) -> bool {
        self.output.assistant_displayed()
    }
}

impl EventSink for CliObserver {
    fn emit(&mut self, event: EventEnvelope) {
        if let Some(trace) = self.trace.as_mut() {
            trace.emit(event.clone());
        }
        self.output.emit(event);
    }
}

fn open_trace(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create trace file {}: {error}", path.display()))
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
