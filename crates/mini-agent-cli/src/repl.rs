use mini_agent_app_server::AppServerRuntime;
use mini_agent_app_server::SessionRequest;
use mini_agent_app_server::ThreadUpdate;
use mini_agent_app_server::frontend::ApprovalController;
use mini_agent_app_server::frontend::ApprovalMode;
use mini_agent_app_server::frontend::DEFAULT_MAX_PENDING_INPUTS;
use mini_agent_app_server::frontend::EventEnvelope;
use mini_agent_app_server::frontend::EventSink;
use mini_agent_app_server::frontend::InputQueueError;
use mini_agent_app_server::frontend::RunControl;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPolicy;
use mini_agent_app_server::frontend::SecurityPreset;
use mini_agent_app_server::frontend::StopReason;
use mini_agent_app_server::frontend::ToolError;
use mini_agent_app_server::frontend::TurnInput;
use mini_agent_app_server::frontend::TurnInputMode;
use mini_agent_app_server::frontend::TurnStatus;
use mini_agent_app_server::frontend::observer::RunObserver;
use mini_agent_app_server::frontend::print_auto_warning;
use mini_agent_app_server::local::LocalRuntimeRequest;
use std::collections::VecDeque;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

const MAX_INPUT_BYTES: usize = 32 * 1024;
const EVENT_BUFFER: usize = 64;

#[path = "repl_worker.rs"]
mod repl_worker;

use repl_worker::{ReplEvent, WorkerCommand};

#[allow(clippy::too_many_arguments)]
pub async fn run(
    initial_approval: ApprovalMode,
    copilot: bool,
    no_tools: bool,
    session_request: SessionRequest,
    preset: SecurityPreset,
    security_preset_explicit: bool,
    sandbox_kind: SandboxKind,
    sandbox_kind_explicit: bool,
    web_search_override: Option<bool>,
) -> ExitCode {
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_BUFFER);
    let approval_events = event_tx.clone();
    let interactive_terminal = io::stdin().is_terminal();
    let approval = ApprovalController::with_policy_and_callback(
        initial_approval,
        SecurityPolicy::for_preset(preset),
        move |action| {
            if interactive_terminal {
                repl_worker::request_approval(&approval_events, action)
            } else {
                Err(ToolError(format!(
                    "denied non-interactive action: {action}"
                )))
            }
        },
    );
    let mut observer = RunObserver::new();
    spawn_input_reader(event_tx.clone());
    let (worker_tx, worker_rx) = mpsc::channel();
    let run_control = RunControl::new();
    let worker = repl_worker::spawn_worker(
        copilot,
        no_tools,
        approval,
        session_request,
        preset,
        security_preset_explicit,
        sandbox_kind,
        sandbox_kind_explicit,
        web_search_override,
        worker_rx,
        event_tx,
        run_control.clone(),
    );

    println!("{}", crate::version_line());
    println!("mini-agent — /steer /exit");
    print_auto_warning();
    if copilot {
        println!("auto mode on");
    }
    let mut pending_work = 0usize;
    let mut pending_approval: VecDeque<mpsc::SyncSender<bool>> = VecDeque::new();
    let mut ready = false;
    let mut active_turn = false;
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
                    command
                        if command.strip_prefix("/steer").is_some_and(|rest| {
                            rest.is_empty() || rest.starts_with(char::is_whitespace)
                        }) =>
                    {
                        let prompt = command[6..].trim();
                        if prompt.is_empty() {
                            eprintln!("usage: /steer <message>");
                        } else if prompt.len() > MAX_INPUT_BYTES {
                            eprintln!("input exceeds {MAX_INPUT_BYTES} byte limit");
                        } else if active_turn {
                            match run_control
                                .submit(TurnInput::new(TurnInputMode::Steer, prompt.to_string()))
                            {
                                Ok(()) => println!(
                                    "steer requested; current turn will stop at the next safe checkpoint"
                                ),
                                Err(InputQueueError::Full { capacity }) => {
                                    eprintln!("steer queue limit reached: {capacity}");
                                }
                                Err(InputQueueError::UnsupportedMode(_)) => {
                                    eprintln!("cannot queue steer input")
                                }
                            }
                        } else {
                            queue_work(
                                &worker_tx,
                                WorkerCommand::Prompt(prompt.to_string()),
                                &mut pending_work,
                            );
                        }
                    }
                    command if command.starts_with('/') => {
                        eprintln!("unknown local command: {command}");
                        print_prompt_if_idle(ready, pending_work, exiting);
                    }
                    _ if input.len() > MAX_INPUT_BYTES => {
                        eprintln!("input exceeds {MAX_INPUT_BYTES} byte limit");
                        print_prompt_if_idle(ready, pending_work, exiting);
                    }
                    _ if active_turn => {
                        match run_control
                            .submit(TurnInput::new(TurnInputMode::FollowUp, input.to_string()))
                        {
                            Ok(()) => {
                                pending_work = pending_work.saturating_add(1);
                                println!("queued ({pending_work} pending)");
                            }
                            Err(InputQueueError::Full { capacity }) => {
                                eprintln!("input queue limit reached: {capacity}");
                            }
                            Err(InputQueueError::UnsupportedMode(_)) => {
                                eprintln!("cannot queue follow-up input");
                            }
                        }
                    }
                    _ => queue_work(
                        &worker_tx,
                        WorkerCommand::Prompt(input.to_string()),
                        &mut pending_work,
                    ),
                }
            }
            ReplEvent::Input(Ok(None)) => {
                request_shutdown(&worker_tx, &mut exiting);
            }
            ReplEvent::Input(Err(error)) => {
                eprintln!("error: cannot read input: {error}");
                request_shutdown(&worker_tx, &mut exiting);
            }
            ReplEvent::Observed(event) => observer.emit(event),
            ReplEvent::Ready => {
                ready = true;
                print_prompt_if_idle(ready, pending_work, exiting);
            }
            ReplEvent::WorkStarted => {
                active_turn = true;
            }
            ReplEvent::WorkFinished => {
                observer.finish();
                active_turn = false;
                pending_work = pending_work.saturating_sub(1);
                if let Some(input) = run_control.take_steer_input() {
                    let _ = worker_tx.send(WorkerCommand::Prompt(input.text));
                } else if let Some(input) = run_control.take_follow_up_input() {
                    let _ = worker_tx.send(WorkerCommand::Prompt(input.text));
                }
                print_prompt_if_idle(ready, pending_work, exiting);
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
    if !ready {
        ExitCode::from(2)
    } else if exiting {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn spawn_input_reader(events: mpsc::SyncSender<ReplEvent>) {
    thread::spawn(move || {
        for result in io::stdin().lines() {
            let failed = result.is_err();
            let event = ReplEvent::Input(result.map(Some).map_err(|error| error.to_string()));
            if events.send(event).is_err() || failed {
                return;
            }
        }
        let _ = events.send(ReplEvent::Input(Ok(None)));
    });
}

fn request_shutdown(worker: &mpsc::Sender<WorkerCommand>, exiting: &mut bool) {
    if !*exiting {
        *exiting = true;
        let _ = worker.send(WorkerCommand::Shutdown);
    }
}

fn print_prompt_if_idle(ready: bool, pending_work: usize, exiting: bool) {
    if ready && pending_work == 0 && !exiting {
        print_prompt();
    }
}

fn queue_work(
    worker: &mpsc::Sender<WorkerCommand>,
    command: WorkerCommand,
    pending_work: &mut usize,
) {
    if *pending_work >= DEFAULT_MAX_PENDING_INPUTS {
        eprintln!("input queue limit reached: {DEFAULT_MAX_PENDING_INPUTS}");
        return;
    }
    if worker.send(command).is_ok() {
        *pending_work = pending_work.saturating_add(1);
        if *pending_work > 1 {
            println!("queued ({pending_work} pending)");
        }
    }
}

fn print_prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}
