use crate::build_openai_harness;
use crate::harness_config;
use crate::observer::RunObserver;
use crate::print_auto_warning;
use crate::workspace::ApprovalController;
use crate::workspace::ApprovalMode;
use mini_codex_core::Event;
use mini_codex_core::HarnessError;
use mini_codex_core::LimitKind;
use mini_codex_core::Observer;
use mini_codex_core::StopReason;
use mini_codex_core::ToolError;
use std::collections::VecDeque;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

const MAX_QUEUED_INPUTS: usize = 16;
const MAX_INPUT_BYTES: usize = 32 * 1024;
const EVENT_BUFFER: usize = 64;

enum ReplEvent {
    Input(Result<Option<String>, String>),
    Observed(Event),
    WorkFinished,
    Approval {
        action: String,
        response: mpsc::SyncSender<bool>,
    },
    Notice(String),
    Warning(String),
    Exited,
}

enum WorkerCommand {
    Prompt(String),
    ClearHistory,
    SetMode(ApprovalMode),
    Shutdown,
}

struct ChannelObserver(mpsc::SyncSender<ReplEvent>);

impl Observer for ChannelObserver {
    fn observe(&mut self, event: &Event) {
        let _ = self.0.send(ReplEvent::Observed(event.clone()));
    }
}

pub async fn run(trace: Option<PathBuf>, initial_mode: ApprovalMode) -> ExitCode {
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_BUFFER);
    let approval_events = event_tx.clone();
    let interactive_terminal = io::stdin().is_terminal();
    let approval = ApprovalController::with_callback(initial_mode, move |action| {
        if interactive_terminal {
            request_approval(&approval_events, action)
        } else {
            Err(ToolError(format!(
                "denied non-interactive action: {action}"
            )))
        }
    });
    let harness = match build_openai_harness(approval.clone(), harness_config(initial_mode)) {
        Ok(harness) => harness,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return ExitCode::FAILURE;
        }
    };
    spawn_input_reader(event_tx.clone());
    let (worker_tx, worker_rx) = mpsc::channel();
    let worker = spawn_worker(harness, approval, worker_rx, event_tx);

    println!("mini-codex — /auto /queue /new /help /exit");
    if initial_mode == ApprovalMode::Automatic {
        print_auto_warning();
        println!("auto mode on");
    }
    print_prompt();

    let mut pending_work = 0usize;
    let mut pending_approval: VecDeque<mpsc::SyncSender<bool>> = VecDeque::new();
    let mut exiting = false;
    while let Ok(event) = event_rx.recv() {
        match event {
            ReplEvent::Input(Ok(Some(line))) => {
                if let Some(response) = pending_approval.pop_front() {
                    let approved = matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                    let _ = response.send(approved);
                    continue;
                }
                let input = line.trim();
                if input.is_empty() || exiting {
                    continue;
                }
                match input {
                    "/exit" => {
                        exiting = true;
                        let _ = worker_tx.send(WorkerCommand::Shutdown);
                        if pending_work > 0 {
                            println!("exit queued after {pending_work} pending operation(s)");
                        }
                    }
                    "/help" => {
                        print_help();
                        if pending_work == 0 {
                            print_prompt();
                        }
                    }
                    "/queue" => {
                        println!("{pending_work} pending operation(s)");
                        if pending_work == 0 {
                            print_prompt();
                        }
                    }
                    "/new" => {
                        queue_work(&worker_tx, WorkerCommand::ClearHistory, &mut pending_work)
                    }
                    "/auto" | "/auto on" => queue_work(
                        &worker_tx,
                        WorkerCommand::SetMode(ApprovalMode::Automatic),
                        &mut pending_work,
                    ),
                    "/auto off" => queue_work(
                        &worker_tx,
                        WorkerCommand::SetMode(ApprovalMode::Interactive),
                        &mut pending_work,
                    ),
                    command if command.starts_with('/') => {
                        eprintln!("unknown local command: {command}");
                        if pending_work == 0 {
                            print_prompt();
                        }
                    }
                    _ if input.len() > MAX_INPUT_BYTES => {
                        eprintln!("input exceeds {MAX_INPUT_BYTES} byte limit");
                        if pending_work == 0 {
                            print_prompt();
                        }
                    }
                    _ if pending_work >= MAX_QUEUED_INPUTS => {
                        eprintln!("input queue limit reached: {MAX_QUEUED_INPUTS}");
                    }
                    _ => queue_work(
                        &worker_tx,
                        WorkerCommand::Prompt(input.to_string()),
                        &mut pending_work,
                    ),
                }
            }
            ReplEvent::Input(Ok(None)) => {
                if !exiting {
                    exiting = true;
                    let _ = worker_tx.send(WorkerCommand::Shutdown);
                }
            }
            ReplEvent::Input(Err(error)) => {
                eprintln!("error: cannot read input: {error}");
                exiting = true;
                let _ = worker_tx.send(WorkerCommand::Shutdown);
            }
            ReplEvent::Observed(event) => observer.observe(&event),
            ReplEvent::WorkFinished => {
                observer.finish();
                pending_work = pending_work.saturating_sub(1);
                if pending_work == 0 && !exiting {
                    print_prompt();
                }
            }
            ReplEvent::Approval { action, response } => {
                eprint!("approve {action}? [y/N] ");
                let _ = io::stderr().flush();
                pending_approval.push_back(response);
            }
            ReplEvent::Notice(message) => println!("{message}"),
            ReplEvent::Warning(message) => eprintln!("{message}"),
            ReplEvent::Exited => break,
        }
    }

    observer.finish();
    let _ = worker.join();
    if exiting {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn spawn_worker(
    mut harness: mini_codex_core::Harness<crate::openai::OpenAiModel>,
    approval: ApprovalController,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::SyncSender<ReplEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
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
        while let Ok(command) = commands.recv() {
            match command {
                WorkerCommand::Prompt(prompt) => {
                    let mut observer = ChannelObserver(events.clone());
                    match runtime.block_on(harness.run(prompt, &mut observer)) {
                        Ok(outcome) if outcome.stop_reason == StopReason::StepLimit => {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "warning: stopped after {} model steps",
                                outcome.steps
                            )));
                        }
                        Ok(_) => {}
                        Err(error) => report_run_error(&events, &error),
                    }
                }
                WorkerCommand::ClearHistory => {
                    harness.clear_history();
                    let _ = events.send(ReplEvent::Notice("new conversation".to_string()));
                }
                WorkerCommand::SetMode(mode) => {
                    approval.set_mode(mode);
                    harness.replace_config(harness_config(mode));
                    match mode {
                        ApprovalMode::Automatic => {
                            let _ = events.send(ReplEvent::Warning("warning: auto mode runs workspace writes and unsandboxed shell commands without approval".to_string()));
                            let _ = events.send(ReplEvent::Notice("auto mode on".to_string()));
                        }
                        ApprovalMode::Interactive => {
                            let _ = events.send(ReplEvent::Notice(
                                "auto mode off; writes and shell commands require approval"
                                    .to_string(),
                            ));
                        }
                    }
                }
                WorkerCommand::Shutdown => break,
            }
            let _ = events.send(ReplEvent::WorkFinished);
        }
        let _ = events.send(ReplEvent::Exited);
    })
}

fn report_run_error(
    events: &mpsc::SyncSender<ReplEvent>,
    error: &HarnessError<crate::openai::OpenAiError>,
) {
    let _ = events.send(ReplEvent::Warning(format!("error: {error}")));
    if matches!(error, HarnessError::Limit(limit) if limit.kind == LimitKind::ContextBytes) {
        let _ = events.send(ReplEvent::Warning(
            "hint: use /new to clear this conversation".to_string(),
        ));
    }
}

fn request_approval(events: &mpsc::SyncSender<ReplEvent>, action: &str) -> Result<bool, ToolError> {
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

fn spawn_input_reader(events: mpsc::SyncSender<ReplEvent>) {
    thread::spawn(move || {
        loop {
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) => {
                    let _ = events.send(ReplEvent::Input(Ok(None)));
                    break;
                }
                Ok(_) => {
                    if events.send(ReplEvent::Input(Ok(Some(line)))).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = events.send(ReplEvent::Input(Err(error.to_string())));
                    break;
                }
            }
        }
    });
}

fn queue_work(
    worker: &mpsc::Sender<WorkerCommand>,
    command: WorkerCommand,
    pending_work: &mut usize,
) {
    if *pending_work >= MAX_QUEUED_INPUTS {
        eprintln!("input queue limit reached: {MAX_QUEUED_INPUTS}");
        return;
    }
    if worker.send(command).is_ok() {
        *pending_work = pending_work.saturating_add(1);
        if *pending_work > 1 {
            println!("queued ({pending_work} pending)");
        }
    }
}

fn print_help() {
    println!("/auto      enable automatic execution");
    println!("/auto off  require approval for writes and shell commands");
    println!("/queue     show pending operations");
    println!("/new       clear this in-memory conversation");
    println!("/help      show local commands");
    println!("/exit      finish queued operations and quit");
}

fn print_prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}
