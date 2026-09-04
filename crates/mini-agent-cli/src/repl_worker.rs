//! REPL worker lifecycle and turn execution.

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
    Notice(String),
    Warning(String),
    Exited,
}

pub(super) enum WorkerCommand {
    Prompt(String),
    Shutdown,
}

struct ChannelObserver(mpsc::SyncSender<ReplEvent>);

impl EventSink for ChannelObserver {
    fn emit(&mut self, event: EventEnvelope) {
        send_event(&self.0, ReplEvent::Observed(event));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_worker(
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
        let initialized: Result<_, String> = (|| {
            let model_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("error: cannot start REPL worker: {error}"))?;
            let launch = mini_agent_app_server::local::prepare(LocalRuntimeRequest {
                no_tools,
                security_preset: preset,
                security_preset_explicit,
                sandbox_kind,
                sandbox_kind_explicit,
                web_search_override,
                session_request,
            })?;
            let mut runtime =
                model_runtime
                    .block_on(launch.start_with_control(
                        approval.clone(),
                        std::sync::Arc::new(run_control.clone()),
                    ))
                    .map_err(|error| error.to_string())?;
            let world = model_runtime
                .block_on(runtime.client_mut().world_state())
                .map_err(|error| error.message)?;
            Ok((model_runtime, runtime, world))
        })();
        let (model_runtime, mut runtime, world) = match initialized {
            Ok(initialized) => initialized,
            Err(error) => {
                send_event(&events, ReplEvent::Warning(format!("error: {error}")));
                send_event(&events, ReplEvent::Exited);
                return;
            }
        };
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
            send_event(&events, ReplEvent::WorkStarted);
            match command {
                WorkerCommand::Prompt(prompt) => {
                    prompt::run_prompt(prompt, &run_control, &mut runtime, &model_runtime, &events);
                }
                WorkerCommand::Shutdown => break 'work,
            }
            send_event(&events, ReplEvent::WorkFinished);
        }
        send_event(&events, ReplEvent::Exited);
    })
}

fn report_run_error(events: &mpsc::SyncSender<ReplEvent>, error: &str) {
    send_event(events, ReplEvent::Warning(format!("error: {error}")));
}

fn send_event(events: &mpsc::SyncSender<ReplEvent>, event: ReplEvent) {
    let _ = events.send(event);
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
