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
use mini_agent_app_server::frontend::RuntimeProfile;
use mini_agent_app_server::frontend::SandboxKind;
use mini_agent_app_server::frontend::SecurityPolicy;
use mini_agent_app_server::frontend::SecurityPreset;
use mini_agent_app_server::frontend::StopReason;
use mini_agent_app_server::frontend::ToolError;
use mini_agent_app_server::frontend::TurnInput;
use mini_agent_app_server::frontend::TurnInputMode;
use mini_agent_app_server::frontend::TurnStatus;
use mini_agent_app_server::frontend::harness_config_auto;
use mini_agent_app_server::frontend::load_workspace_profile;
use mini_agent_app_server::frontend::observer::RunObserver;
use mini_agent_app_server::frontend::print_auto_warning;
use mini_agent_app_server::frontend::skills;
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
const MAX_WELCOME_NAMES: usize = 8;

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
    let workspace = std::env::current_dir().ok();
    let startup_extensions = workspace
        .as_ref()
        .filter(|_| !no_tools)
        .map(|workspace| skills::discover(workspace));
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
    println!("mini-agent — /auto /status /world /session /mcp /queue /new /help /exit");
    print_auto_warning();
    if copilot {
        println!("auto mode on");
    }
    let startup_profile = if copilot {
        RuntimeProfile::auto_default()
    } else {
        RuntimeProfile::interactive_default()
    };
    let startup_profile = match workspace
        .as_ref()
        .map(|workspace| load_workspace_profile(workspace, startup_profile.clone()))
    {
        Some(Ok(profile)) => profile,
        Some(Err(error)) => {
            eprintln!("profile> {error}");
            startup_profile
        }
        None => startup_profile,
    };
    let startup_profile = if no_tools {
        startup_profile.without_tools()
    } else {
        startup_profile
    };
    let startup_profile = if sandbox_kind_explicit {
        startup_profile.with_sandbox(sandbox_kind)
    } else {
        startup_profile
    };
    let startup_profile = if security_preset_explicit {
        startup_profile.with_security(preset)
    } else {
        startup_profile
    };
    let manifest = startup_profile.manifest();
    let disabled = manifest
        .disabled
        .iter()
        .map(|capability| capability.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "capabilities> profile={} enabled={}{}",
        manifest.profile,
        manifest.enabled.join(","),
        if disabled.is_empty() {
            String::new()
        } else {
            format!(" disabled={disabled}")
        }
    );
    if let Some(discovery) = startup_extensions {
        print_extension_summary(&discovery);
    }
    if let Some(workspace) = workspace.as_ref() {
        println!(
            "world> {}",
            mini_agent_app_server::local::world_summary(
                workspace,
                initial_approval,
                copilot,
                startup_profile.sandbox(),
            )
        );
    }
    println!("initializing extensions...");

    let mut pending_work = 0usize;
    let mut pending_approval: VecDeque<mpsc::SyncSender<bool>> = VecDeque::new();
    let mut ready = false;
    let mut active_turn = false;
    let mut exiting = false;
    let mut initialization_failed = false;
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
                        if ready && pending_work == 0 {
                            print_prompt();
                        }
                    }
                    "/queue" => {
                        println!("{pending_work} pending operation(s)");
                        if ready && pending_work == 0 {
                            print_prompt();
                        }
                    }
                    "/new" => {
                        queue_work(&worker_tx, WorkerCommand::ClearHistory, &mut pending_work)
                    }
                    "/status" | "/info" => {
                        queue_work(&worker_tx, WorkerCommand::ShowStatus, &mut pending_work)
                    }
                    "/world" => queue_work(&worker_tx, WorkerCommand::ShowWorld, &mut pending_work),
                    "/world refresh" => {
                        queue_work(&worker_tx, WorkerCommand::RefreshWorld, &mut pending_work)
                    }
                    "/session" => {
                        queue_work(&worker_tx, WorkerCommand::ShowSession, &mut pending_work)
                    }
                    "/mcp" => queue_work(&worker_tx, WorkerCommand::EnableMcp, &mut pending_work),
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
                    "/auto" | "/auto on" => queue_work(
                        &worker_tx,
                        WorkerCommand::SetExecution {
                            approval: ApprovalMode::Automatic,
                            copilot: true,
                        },
                        &mut pending_work,
                    ),
                    "/auto off" => queue_work(
                        &worker_tx,
                        WorkerCommand::SetExecution {
                            approval: ApprovalMode::Interactive,
                            copilot: false,
                        },
                        &mut pending_work,
                    ),
                    command if command.starts_with('/') => {
                        eprintln!("unknown local command: {command}");
                        if ready && pending_work == 0 {
                            print_prompt();
                        }
                    }
                    _ if input.len() > MAX_INPUT_BYTES => {
                        eprintln!("input exceeds {MAX_INPUT_BYTES} byte limit");
                        if ready && pending_work == 0 {
                            print_prompt();
                        }
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
            ReplEvent::Observed(event) => observer.emit(event),
            ReplEvent::Ready => {
                ready = true;
                if pending_work == 0 && !exiting {
                    print_prompt();
                }
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
                if ready && pending_work == 0 && !exiting {
                    print_prompt();
                }
            }
            ReplEvent::Approval { action, response } => {
                eprint!("approve {action}? [y/N] ");
                let _ = io::stderr().flush();
                pending_approval.push_back(response);
            }
            ReplEvent::InitializationFailed(error) => {
                eprintln!("error: {error}");
                initialization_failed = true;
            }
            ReplEvent::Notice(message) => println!("{message}"),
            ReplEvent::Warning(message) => eprintln!("{message}"),
            ReplEvent::Exited => break,
        }
    }

    observer.finish();
    let _ = worker.join();
    if initialization_failed {
        ExitCode::from(2)
    } else if exiting {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[allow(clippy::too_many_arguments)]
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

fn print_extension_summary(discovery: &skills::Discovery) {
    print_loaded_extensions("skill", discovery.skill_names());
    print_loaded_extensions("plugin", discovery.plugin_names());
    let mut mcp_servers = discovery.mcp_server_labels();
    mcp_servers.sort();
    if mcp_servers.is_empty() {
        println!("mcp> none configured");
    } else {
        println!(
            "mcp> {} configured, inactive — {}",
            mcp_servers.len(),
            bounded_names(&mcp_servers)
        );
    }
}

fn print_loaded_extensions(label: &str, mut names: Vec<String>) {
    names.sort();
    if names.is_empty() {
        println!("{label}> none");
    } else {
        println!(
            "{label}> {} loaded — {}",
            names.len(),
            bounded_names(&names)
        );
    }
}

fn bounded_names(names: &[String]) -> String {
    let mut summary = names
        .iter()
        .take(MAX_WELCOME_NAMES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_WELCOME_NAMES {
        summary.push_str(&format!(", +{} more", names.len() - MAX_WELCOME_NAMES));
    }
    summary
}

fn print_help() {
    println!(
        "/auto          Enable autonomous copilot loop (unlimited steps, automatic context compaction)"
    );
    println!(
        "/auto off      Switch to manual mode (require per-step approval for writes/shell/MCP)"
    );
    println!(
        "/status        Display runtime status (security preset, sandbox, web search, session, approval)"
    );
    println!("/world         Show detected host, shell, and command capabilities");
    println!("/world refresh Re-scan environment and append updated world state to context");
    println!("/session       Show durable session ID, thread ID, and JSONL persistence path");
    println!("/mcp           Retry connecting configured MCP servers that are currently inactive");
    println!("/queue         Show number of pending operations in input queue");
    println!("/steer <msg>   Stop the running turn at a safe checkpoint, then run <msg>");
    println!("/new           Clear conversation history and start a fresh context");
    println!("/help          Display this list of interactive slash commands");
    println!("/exit          Finish queued work and quit");
    println!(
        "project> https://github.com/civaapple-alt/mini-agent-harness | creator: civaapple-alt | MIT"
    );
}

fn print_prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}
