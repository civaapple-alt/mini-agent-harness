use crate::harness_config;
use crate::mcp;
use crate::observer::RunObserver;
use crate::print_auto_warning;
use crate::session;
use crate::session::OpenedSession;
use crate::session::SessionRequest;
use crate::session::SessionStore;
use crate::session::TurnCommit;
use crate::session::TurnStatus;
use crate::skills;
use crate::workspace::ApprovalController;
use crate::workspace::ApprovalMode;
use crate::world::WorldState;
use mini_agent_core::Event;
use mini_agent_core::HarnessError;
use mini_agent_core::LimitKind;
use mini_agent_core::Observer;
use mini_agent_core::StopReason;
use mini_agent_core::ToolError;
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
const MAX_WELCOME_NAMES: usize = 8;

enum ReplEvent {
    Input(Result<Option<String>, String>),
    Observed(Event),
    Ready,
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

enum WorkerCommand {
    Prompt(String),
    ClearHistory,
    ShowWorld,
    RefreshWorld,
    ShowSession,
    EnableMcp,
    SetMode(ApprovalMode),
    Shutdown,
}

struct ChannelObserver(mpsc::SyncSender<ReplEvent>);

impl Observer for ChannelObserver {
    fn observe(&mut self, event: &Event) {
        let _ = self.0.send(ReplEvent::Observed(event.clone()));
    }
}

pub async fn run(
    trace: Option<PathBuf>,
    initial_mode: ApprovalMode,
    session_request: SessionRequest,
) -> ExitCode {
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
    let mut observer = match RunObserver::new(trace) {
        Ok(observer) => observer,
        Err(error) => {
            eprintln!("error: cannot create trace: {error}");
            return ExitCode::FAILURE;
        }
    };
    let workspace = std::env::current_dir().ok();
    let startup_extensions = workspace
        .as_ref()
        .map(|workspace| skills::discover(workspace));
    let startup_world = workspace
        .as_ref()
        .map(|workspace| WorldState::detect(workspace, initial_mode));
    spawn_input_reader(event_tx.clone());
    let (worker_tx, worker_rx) = mpsc::channel();
    let worker = spawn_worker(initial_mode, approval, session_request, worker_rx, event_tx);

    println!("mini-agent — /auto /world /session /mcp /queue /new /help /exit");
    if initial_mode == ApprovalMode::Automatic {
        print_auto_warning();
        println!("auto mode on");
    }
    if let Some(discovery) = startup_extensions {
        print_extension_summary(&discovery);
    }
    if let Some(world) = startup_world {
        println!("world> {}", world.summary());
    }
    println!("initializing extensions...");

    let mut pending_work = 0usize;
    let mut pending_approval: VecDeque<mpsc::SyncSender<bool>> = VecDeque::new();
    let mut ready = false;
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
                    "/world" => queue_work(&worker_tx, WorkerCommand::ShowWorld, &mut pending_work),
                    "/world refresh" => {
                        queue_work(&worker_tx, WorkerCommand::RefreshWorld, &mut pending_work)
                    }
                    "/session" => {
                        queue_work(&worker_tx, WorkerCommand::ShowSession, &mut pending_work)
                    }
                    "/mcp" => queue_work(&worker_tx, WorkerCommand::EnableMcp, &mut pending_work),
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
            ReplEvent::Ready => {
                ready = true;
                if pending_work == 0 && !exiting {
                    print_prompt();
                }
            }
            ReplEvent::WorkFinished => {
                observer.finish();
                pending_work = pending_work.saturating_sub(1);
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

fn spawn_worker(
    initial_mode: ApprovalMode,
    approval: ApprovalController,
    session_request: SessionRequest,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::SyncSender<ReplEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let build = match crate::build_repl_harness(approval.clone(), harness_config(initial_mode))
        {
            Ok(build) => build,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let crate::ReplHarnessBuild {
            mut harness,
            stable_system_prompt,
            mut world,
            enabled_mcp_servers,
            mcp_tool_count,
            mut retry_mcp_servers,
        } = build;
        let mut durable = match session_request {
            SessionRequest::Disabled => None,
            request => match SessionStore::open(world.workspace(), request) {
                Ok(opened) => Some(opened),
                Err(error) => {
                    let _ = events.send(ReplEvent::InitializationFailed(error));
                    let _ = events.send(ReplEvent::Exited);
                    return;
                }
            },
        };
        if let Some(opened) = &mut durable {
            if opened.resumed {
                if let Err(error) = harness.restore_history(std::mem::take(&mut opened.messages)) {
                    let _ = events.send(ReplEvent::InitializationFailed(format!(
                        "cannot restore session history: {error}"
                    )));
                    let _ = events.send(ReplEvent::Exited);
                    return;
                }
                match world.model_context() {
                    Ok(context) => {
                        if let Err(error) = harness.append_context(context) {
                            let _ = events.send(ReplEvent::InitializationFailed(format!(
                                "cannot append current world state: {error}"
                            )));
                            let _ = events.send(ReplEvent::Exited);
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = events.send(ReplEvent::InitializationFailed(error));
                        let _ = events.send(ReplEvent::Exited);
                        return;
                    }
                }
            }
            let label = if opened.resumed { "resumed" } else { "new" };
            let _ = events.send(ReplEvent::Notice(format!(
                "session> {label} {} | thread {} | {}",
                opened.store.session_id(),
                opened.store.thread_id(),
                opened.store.path().display()
            )));
            persist_latest_context(&mut durable, &harness, &events);
        }
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
        if !enabled_mcp_servers.is_empty() {
            let _ = events.send(ReplEvent::Notice(format!(
                "mcp> enabled — {} ({mcp_tool_count} tool(s))",
                bounded_names(&enabled_mcp_servers)
            )));
        }
        if !retry_mcp_servers.is_empty() {
            let inactive = retry_mcp_servers
                .iter()
                .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
                .collect::<Vec<_>>();
            let _ = events.send(ReplEvent::Notice(format!(
                "mcp> inactive — {}; use /mcp to retry",
                bounded_names(&inactive)
            )));
        }
        if events.send(ReplEvent::Ready).is_err() {
            return;
        }
        while let Ok(command) = commands.recv() {
            match command {
                WorkerCommand::Prompt(prompt) => {
                    let started_at_ms = session::timestamp_ms();
                    let previous_messages = harness.messages().to_vec();
                    let mut observer = ChannelObserver(events.clone());
                    let result = runtime.block_on(harness.run(prompt.clone(), &mut observer));
                    let (status, steps, error) = match &result {
                        Ok(outcome) if outcome.stop_reason == StopReason::StepLimit => {
                            (TurnStatus::StepLimit, outcome.steps, None)
                        }
                        Ok(outcome) => (TurnStatus::Completed, outcome.steps, None),
                        Err(error) => (TurnStatus::Failed, 0, Some(error.to_string())),
                    };
                    let turn_messages = harness
                        .messages()
                        .strip_prefix(previous_messages.as_slice())
                        .unwrap_or_else(|| harness.messages());
                    if let Some(opened) = &mut durable
                        && let Err(error) = opened.store.record_turn(TurnCommit {
                            started_at_ms,
                            prompt: &prompt,
                            status,
                            steps,
                            error: error.as_deref(),
                            messages: turn_messages,
                            checkpoint: harness.messages(),
                        })
                    {
                        let _ = events.send(ReplEvent::Warning(format!(
                            "warning: session persistence stopped: {error}"
                        )));
                        durable = None;
                    }
                    match result {
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
                    if let Some(opened) = &mut durable
                        && let Err(error) = opened.store.start_thread()
                    {
                        let _ = events.send(ReplEvent::Warning(format!(
                            "warning: session persistence stopped: {error}"
                        )));
                        durable = None;
                    }
                    harness.clear_history();
                    match world.model_context() {
                        Ok(context) => match harness.append_context(context) {
                            Ok(()) => {
                                persist_latest_context(&mut durable, &harness, &events);
                                let _ =
                                    events.send(ReplEvent::Notice("new conversation".to_string()));
                            }
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot restore world state: {error}"
                                )));
                            }
                        },
                        Err(error) => {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "error: cannot restore world state: {error}"
                            )));
                        }
                    }
                }
                WorkerCommand::ShowWorld => {
                    for line in world.status_lines() {
                        let _ = events.send(ReplEvent::Notice(format!("world> {line}")));
                    }
                }
                WorkerCommand::RefreshWorld => {
                    let refreshed = WorldState::detect(world.workspace(), world.mode());
                    if refreshed != world {
                        match refreshed.model_context() {
                            Ok(context) => match harness.append_context(context) {
                                Ok(()) => {
                                    world = refreshed;
                                    persist_latest_context(&mut durable, &harness, &events);
                                    let _ = events.send(ReplEvent::Notice(
                                        "world> refreshed and appended to context".to_string(),
                                    ));
                                }
                                Err(error) => {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "error: cannot append world state: {error}"
                                    )));
                                }
                            },
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot refresh world state: {error}"
                                )));
                            }
                        }
                    } else {
                        let _ = events.send(ReplEvent::Notice(
                            "world> unchanged; no context item appended".to_string(),
                        ));
                    }
                }
                WorkerCommand::EnableMcp => {
                    if retry_mcp_servers.is_empty() {
                        let _ = events.send(ReplEvent::Notice(
                            "no MCP servers are waiting to be enabled".to_string(),
                        ));
                    } else {
                        let mcp::LoadResult {
                            tools,
                            loaded_servers,
                            diagnostics,
                        } = mcp::load(&retry_mcp_servers, approval.clone());
                        for diagnostic in diagnostics {
                            let _ =
                                events.send(ReplEvent::Warning(format!("warning: {diagnostic}")));
                        }
                        retry_mcp_servers.retain(|server| {
                            !loaded_servers
                                .contains(&format!("{}/{}", server.plugin_name, server.server_name))
                        });
                        let enabled = loaded_servers.iter().cloned().collect::<Vec<_>>();
                        let tool_count = tools.len();
                        harness.extend_tools(tools);
                        let message = if enabled.is_empty() {
                            "mcp> inactive — no servers enabled; use /mcp to retry".to_string()
                        } else {
                            format!(
                                "mcp> enabled — {} ({tool_count} tool(s))",
                                bounded_names(&enabled)
                            )
                        };
                        let _ = events.send(ReplEvent::Notice(message));
                        if !retry_mcp_servers.is_empty() {
                            let inactive = retry_mcp_servers
                                .iter()
                                .map(|server| {
                                    format!("{}/{}", server.plugin_name, server.server_name)
                                })
                                .collect::<Vec<_>>();
                            let _ = events.send(ReplEvent::Notice(format!(
                                "mcp> inactive — {}; use /mcp to retry",
                                bounded_names(&inactive)
                            )));
                        }
                    }
                }
                WorkerCommand::ShowSession => match &durable {
                    Some(opened) => {
                        let _ = events.send(ReplEvent::Notice(format!(
                            "session> durable {} | thread {} | {}",
                            opened.store.session_id(),
                            opened.store.thread_id(),
                            opened.store.path().display()
                        )));
                    }
                    None => {
                        let _ = events.send(ReplEvent::Notice(
                            "session> in-memory; restart with --persist to make it durable"
                                .to_string(),
                        ));
                    }
                },
                WorkerCommand::SetMode(mode) => {
                    approval.set_mode(mode);
                    let mut config = harness_config(mode);
                    config.system_prompt.clone_from(&stable_system_prompt);
                    harness.replace_config(config);
                    let updated_world = world.with_mode(mode);
                    if updated_world != world {
                        match updated_world.model_context() {
                            Ok(context) => match harness.append_context(context) {
                                Ok(()) => {
                                    world = updated_world;
                                    persist_latest_context(&mut durable, &harness, &events);
                                }
                                Err(error) => {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "error: cannot append execution mode state: {error}"
                                    )));
                                }
                            },
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot render execution mode state: {error}"
                                )));
                            }
                        }
                    }
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

fn persist_latest_context(
    durable: &mut Option<OpenedSession>,
    harness: &mini_agent_core::Harness<crate::openai::OpenAiModel>,
    events: &mpsc::SyncSender<ReplEvent>,
) {
    let context = harness
        .messages()
        .iter()
        .rev()
        .find(|message| matches!(message, mini_agent_core::Message::Context { .. }))
        .cloned();
    let error = durable.as_mut().and_then(|opened| {
        let context = context.as_ref()?;
        opened
            .store
            .record_context(context, harness.messages())
            .err()
    });
    if let Some(error) = error {
        let _ = events.send(ReplEvent::Warning(format!(
            "warning: session persistence stopped: {error}"
        )));
        *durable = None;
    }
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
    println!("/auto      enable automatic execution");
    println!("/auto off  require approval for writes and shell commands");
    println!("/mcp       retry configured MCP servers that are not enabled");
    println!("/world     show detected environment, mode, and command capabilities");
    println!("/world refresh  detect changes and append a new world-state item");
    println!("/session    show current in-memory or durable session identity");
    println!("/queue     show pending operations");
    println!("/new       clear this in-memory conversation");
    println!("/help      show local commands");
    println!("/exit      finish queued operations and quit");
}

fn print_prompt() {
    print!("> ");
    let _ = io::stdout().flush();
}
