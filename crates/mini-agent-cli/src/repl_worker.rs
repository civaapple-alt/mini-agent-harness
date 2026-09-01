//! Worker-side REPL orchestration.
//!
//! The parent module owns terminal input and rendering; this module owns the
//! App Server worker lifecycle and core turn execution.

use super::*;

#[path = "repl_worker/prompt.rs"]
mod prompt;

pub(super) enum ReplEvent {
    Input(Result<Option<String>, String>),
    Observed(EventEnvelope),
    Ready,
    WorkStarted,
    WorkFinished,
    Approval {
        action: String,
        response: mpsc::SyncSender<bool>,
    },
    InitializationFailed(String),
    Notice(String),
    Warning(String),
    Exited,
}

pub(super) enum WorkerCommand {
    Prompt(String),
    ClearHistory,
    SetExecution {
        approval: ApprovalMode,
        copilot: bool,
    },
    Shutdown,
}

struct ChannelObserver(mpsc::SyncSender<ReplEvent>);

impl EventSink for ChannelObserver {
    fn emit(&mut self, event: EventEnvelope) {
        let _ = self.0.send(ReplEvent::Observed(event));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_worker(
    copilot: bool,
    no_tools: bool,
    approval: ApprovalController,
    session_request: SessionRequest,
    preset: SecurityPreset,
    security_preset_explicit: bool,
    sandbox_kind: SandboxKind,
    sandbox_kind_explicit: bool,
    web_search_override: Option<bool>,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::SyncSender<ReplEvent>,
    run_control: RunControl,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let model_runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(ReplEvent::Warning(format!(
                    "error: cannot start REPL worker: {error}"
                )));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let launch = match mini_agent_app_server::local::prepare(LocalRuntimeRequest {
            automatic: copilot,
            no_tools,
            security_preset: preset,
            security_preset_explicit,
            sandbox_kind,
            sandbox_kind_explicit,
            web_search_override,
            session_request,
            max_steps: None,
        }) {
            Ok(launch) => launch,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let auto_max_steps = launch.copilot_max_steps();
        let mut runtime = match model_runtime.block_on(
            launch.start_with_control(approval.clone(), std::sync::Arc::new(run_control.clone())),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let mut world = match model_runtime.block_on(runtime.client_mut().world_state()) {
            Ok(world) => world,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error.message));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let stable_system_prompt = runtime.stable_system_prompt().to_string();
        if let Ok(Some(opened)) = model_runtime.block_on(runtime.client_mut().session_info())
            && opened.resumed
        {
            let _ = model_runtime.block_on(
                runtime.client_mut().update_thread(ThreadUpdate::AppendContext(
                    "[Session resumed. Note: previously running background processes and result preview handles from prior sessions have expired.]".to_string(),
                )),
            );
            let _ = model_runtime.block_on(
                runtime
                    .client_mut()
                    .update_thread(ThreadUpdate::AppendContext(world.context.clone())),
            );
        }
        if events.send(ReplEvent::Ready).is_err() {
            return;
        }
        'work: while let Ok(command) = commands.recv() {
            let _ = events.send(ReplEvent::WorkStarted);
            loop {
                match command {
                    WorkerCommand::Prompt(prompt) => {
                        prompt::run_prompt(prompt::PromptContext {
                            prompt,
                            run_control: &run_control,
                            runtime: &mut runtime,
                            model_runtime: &model_runtime,
                            events: &events,
                        });
                    }
                    WorkerCommand::ClearHistory => {
                        let has_session = model_runtime
                            .block_on(runtime.client_mut().session_info())
                            .ok()
                            .flatten()
                            .is_some();
                        if has_session
                            && let Err(error) =
                                model_runtime.block_on(runtime.client_mut().start_new_thread())
                        {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "warning: session persistence stopped: {error}"
                            )));
                        }
                        if let Some(error) = model_runtime
                            .block_on(
                                runtime
                                    .client_mut()
                                    .update_thread(ThreadUpdate::ClearHistory),
                            )
                            .err()
                        {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "error: cannot clear conversation: {error}"
                            )));
                            continue;
                        }
                        match model_runtime.block_on(
                            runtime
                                .client_mut()
                                .update_thread(ThreadUpdate::AppendContext(world.context.clone())),
                        ) {
                            Ok(()) => {
                                let _ =
                                    events.send(ReplEvent::Notice("new conversation".to_string()));
                            }
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot restore world state: {error}"
                                )));
                            }
                        }
                    }
                    WorkerCommand::SetExecution {
                        approval: mode,
                        copilot,
                    } => {
                        approval.set_mode(mode);
                        let mut config = harness_config_auto(copilot, auto_max_steps);
                        config.system_prompt.clone_from(&stable_system_prompt);
                        if let Err(error) = model_runtime.block_on(
                            runtime
                                .client_mut()
                                .update_thread(ThreadUpdate::ReplaceConfig(config)),
                        ) {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "error: cannot change execution mode: {error}"
                            )));
                        }
                        match model_runtime.block_on(runtime.client_mut().set_world_execution(
                            match mode {
                                ApprovalMode::Automatic => "automatic",
                                ApprovalMode::Interactive => "interactive",
                            },
                            copilot,
                        )) {
                            Ok(result) => world = result.state,
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot append execution mode state: {}",
                                    error.message
                                )));
                            }
                        }
                        if copilot {
                            let _ = events.send(ReplEvent::Warning("warning: auto mode runs workspace writes and unsandboxed shell commands without approval".to_string()));
                            let _ = events.send(ReplEvent::Notice("auto mode on".to_string()));
                        } else if mode == ApprovalMode::Interactive {
                            let _ = events.send(ReplEvent::Notice(
                                "auto mode off; writes and shell commands require approval"
                                    .to_string(),
                            ));
                        }
                    }
                    WorkerCommand::Shutdown => break 'work,
                }
                break;
            }
            let _ = events.send(ReplEvent::WorkFinished);
        }
        let _ = events.send(ReplEvent::Exited);
    })
}

fn report_run_error(events: &mpsc::SyncSender<ReplEvent>, error: &str) {
    let _ = events.send(ReplEvent::Warning(format!("error: {error}")));
    if error.contains("context") {
        let _ = events.send(ReplEvent::Warning(
            "hint: use /new to clear this conversation".to_string(),
        ));
    }
}

pub(super) fn request_approval(
    events: &mpsc::SyncSender<ReplEvent>,
    action: &str,
) -> Result<bool, ToolError> {
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    events
        .send(ReplEvent::Approval {
            action: action.to_string(),
            response: response_tx,
        })
        .map_err(|_| ToolError("approval UI is unavailable".to_string()))?;
    response_rx
        .recv()
        .map_err(|_| ToolError("approval UI closed".to_string()))
}
