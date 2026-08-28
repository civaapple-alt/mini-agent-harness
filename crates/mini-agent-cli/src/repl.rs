use crate::config::RuntimeConfig;
use crate::harness_config_auto;
use crate::mcp;
use crate::mentor;
use crate::observer::RunObserver;
use crate::print_auto_warning;
use crate::session;
use crate::session::OpenedSession;
use crate::session::SessionRequest;
use crate::session::SessionStore;
use crate::session::TurnCommit;
use crate::session::TurnStatus;
use crate::skills;
use crate::tool_outcome::classify_tools;
use crate::workspace::ApprovalController;
use crate::workspace::ApprovalMode;
use crate::world::WorldState;
use mini_agent_core::DEFAULT_MAX_PENDING_INPUTS;
use mini_agent_core::EventEnvelope;
use mini_agent_core::EventSink;
use mini_agent_core::HarnessError;
use mini_agent_core::InputQueueError;
use mini_agent_core::LimitKind;
use mini_agent_core::RunControl;
use mini_agent_core::SessionState;
use mini_agent_core::SteeringMode;
use mini_agent_core::StopReason;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ToolError;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use std::collections::VecDeque;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const MAX_INPUT_BYTES: usize = 32 * 1024;
const EVENT_BUFFER: usize = 64;
const MAX_WELCOME_NAMES: usize = 8;

enum ReplEvent {
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

enum WorkerCommand {
    Prompt(String),
    ClearHistory,
    ShowStatus,
    ShowWorld,
    RefreshWorld,
    ShowSession,
    EnableMcp,
    SetExecution {
        approval: ApprovalMode,
        copilot: bool,
    },
    SetPlanMode {
        active: bool,
        prompt: Option<String>,
    },
    StartGoal(String),
    Shutdown,
}

struct ChannelObserver(mpsc::SyncSender<ReplEvent>);

impl EventSink for ChannelObserver {
    fn emit(&mut self, event: EventEnvelope) {
        let _ = self.0.send(ReplEvent::Observed(event));
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    trace: Option<PathBuf>,
    initial_approval: ApprovalMode,
    copilot: bool,
    session_request: SessionRequest,
    preset: crate::security::SecurityPreset,
    sandbox_kind: crate::sandbox::SandboxKind,
    web_search_override: Option<bool>,
) -> ExitCode {
    let (event_tx, event_rx) = mpsc::sync_channel(EVENT_BUFFER);
    let approval_events = event_tx.clone();
    let interactive_terminal = io::stdin().is_terminal();
    let approval = ApprovalController::with_policy_and_callback(
        initial_approval,
        crate::security::SecurityPolicy::for_preset(preset),
        move |action| {
            if interactive_terminal {
                request_approval(&approval_events, action)
            } else {
                Err(ToolError(format!(
                    "denied non-interactive action: {action}"
                )))
            }
        },
    );
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
        .map(|workspace| WorldState::detect(workspace, initial_approval, copilot, sandbox_kind));
    spawn_input_reader(event_tx.clone());
    let (worker_tx, worker_rx) = mpsc::channel();
    let run_control = RunControl::new();
    let worker = spawn_worker(
        copilot,
        approval,
        session_request,
        preset,
        sandbox_kind,
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
                    command
                        if command.strip_prefix("/plan").is_some_and(|rest| {
                            rest.is_empty() || rest.starts_with(char::is_whitespace)
                        }) =>
                    {
                        let action = match crate::goal::parse_plan_slash(command) {
                            Some(action) => action,
                            None => unreachable!("plan command matched its parser guard"),
                        };
                        match action {
                            crate::goal::PlanSlash::Disable => queue_work(
                                &worker_tx,
                                WorkerCommand::SetPlanMode {
                                    active: false,
                                    prompt: None,
                                },
                                &mut pending_work,
                            ),
                            crate::goal::PlanSlash::Enable { prompt } => queue_work(
                                &worker_tx,
                                WorkerCommand::SetPlanMode {
                                    active: true,
                                    prompt,
                                },
                                &mut pending_work,
                            ),
                        }
                    }
                    command if command.starts_with("/goal ") => {
                        let objective = command[6..].trim().to_string();
                        if objective.is_empty() {
                            eprintln!("usage: /goal <objective>");
                            if ready && pending_work == 0 {
                                print_prompt();
                            }
                        } else {
                            queue_work(
                                &worker_tx,
                                WorkerCommand::StartGoal(objective),
                                &mut pending_work,
                            );
                        }
                    }
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
fn spawn_worker(
    copilot: bool,
    approval: ApprovalController,
    session_request: SessionRequest,
    preset: crate::security::SecurityPreset,
    sandbox_kind: crate::sandbox::SandboxKind,
    web_search_override: Option<bool>,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::SyncSender<ReplEvent>,
    run_control: RunControl,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (build, auto_max_steps, web_search_enabled, runtime_config) =
            match RuntimeConfig::load().and_then(|mut runtime| {
                if let Some(enabled) = web_search_override {
                    runtime = runtime.with_web_search(enabled);
                }
                let web_search_enabled = runtime.web_search();
                let auto_max_steps = runtime.copilot_max_steps();
                crate::prepare_openai_harness(
                    &runtime,
                    approval.clone(),
                    harness_config_auto(copilot, auto_max_steps),
                    sandbox_kind,
                )
                .map(|build| (build, auto_max_steps, web_search_enabled, runtime))
            }) {
                Ok(loaded) => loaded,
                Err(error) => {
                    let _ = events.send(ReplEvent::InitializationFailed(error));
                    let _ = events.send(ReplEvent::Exited);
                    return;
                }
            };
        let crate::HarnessBuild {
            harness,
            images,
            stable_system_prompt,
            mut world,
            enabled_mcp_servers,
            mcp_tool_count,
            mut retry_mcp_servers,
        } = build;
        let mut harness = Thread::new(ThreadId::new("ephemeral"), harness);
        let mut copilot = copilot;
        let mut goal_objective: Option<String> = None;
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
            harness.set_id(ThreadId::new(opened.store.thread_id()));
            harness.set_next_turn_number(opened.store.thread_turn_count() as u64 + 1);
            approval.bind_session_file(opened.store.path());
            images.bind_session_file(opened.store.path());
            if opened.resumed {
                if let Err(error) = harness
                    .restore_session(std::mem::replace(&mut opened.state, SessionState::new()))
                {
                    let _ = events.send(ReplEvent::InitializationFailed(format!(
                        "cannot restore session history: {error}"
                    )));
                    let _ = events.send(ReplEvent::Exited);
                    return;
                }
                let _ = harness.append_context(
                    "[Session resumed. Note: previously running background processes and result preview handles from prior sessions have expired.]",
                );
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
                if let Some(session_dir) = opened.store.path().parent()
                    && let Ok(Some(state)) = crate::goal::load_goal_state(session_dir)
                    && state.status == crate::goal::GoalStatus::Running
                {
                    let _ = crate::goal::pause_goal(session_dir);
                    let _ = events.send(ReplEvent::Warning(
                        "goal> paused on restart; reissue /goal to continue".to_string(),
                    ));
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
        'work: while let Ok(mut command) = commands.recv() {
            let _ = events.send(ReplEvent::WorkStarted);
            loop {
                match command {
                    WorkerCommand::Prompt(prompt) => {
                        run_control.clear_steer();
                        let prompt = if approval.living_plan().is_some() {
                            crate::goal::planning_turn_prompt(&prompt)
                        } else {
                            prompt
                        };
                        let started_at_ms = session::timestamp_ms();
                        let previous_messages = harness.messages().to_vec();
                        let mut observer = ChannelObserver(events.clone());
                        let goal_timeout = approval.goal_dir().and_then(|goal_dir| {
                            goal_dir
                                .parent()
                                .and_then(|dir| crate::goal::load_goal_state(dir).ok().flatten())
                                .map(|state| Duration::from_secs(state.milestone_timeout_secs))
                        });
                        let result = if let Some(timeout) = goal_timeout {
                            match model_runtime.block_on(async {
                                tokio::time::timeout(
                                    timeout,
                                    harness.run_turn_with_events_outcome(
                                        TurnInput::new(TurnInputMode::Start, prompt.clone()),
                                        &mut observer,
                                        &run_control,
                                        SteeringMode::StopAtCheckpoint,
                                    ),
                                )
                                .await
                            }) {
                                Ok(result) => result,
                                Err(_) => {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "goal> milestone timed out after {} seconds",
                                        timeout.as_secs()
                                    )));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        world.workspace(),
                                    );
                                    break;
                                }
                            }
                        } else {
                            model_runtime.block_on(harness.run_turn_with_events_outcome(
                                TurnInput::new(TurnInputMode::Start, prompt.clone()),
                                &mut observer,
                                &run_control,
                                SteeringMode::StopAtCheckpoint,
                            ))
                        };
                        let (status, steps, error) = match &result {
                            Ok(outcome) if outcome.stop_reason == StopReason::Steered => {
                                (TurnStatus::Steered, outcome.steps, None)
                            }
                            Ok(outcome) if outcome.stop_reason == StopReason::StepLimit => {
                                (TurnStatus::StepLimit, outcome.steps, None)
                            }
                            Ok(outcome) if outcome.stop_reason == StopReason::Cancelled => {
                                (TurnStatus::Cancelled, outcome.steps, None)
                            }
                            Ok(outcome) => (TurnStatus::Completed, outcome.steps, None),
                            Err(error) => (TurnStatus::Failed, 0, Some(error.to_string())),
                        };
                        let turn_messages = harness
                            .messages()
                            .strip_prefix(previous_messages.as_slice())
                            .unwrap_or_else(|| harness.messages());
                        if let Some(opened) = &mut durable {
                            let commit = TurnCommit {
                                started_at_ms,
                                prompt: &prompt,
                                status,
                                steps,
                                error: error.as_deref(),
                                messages: turn_messages,
                                checkpoint: harness.messages(),
                            };
                            let result = match harness.last_turn_id() {
                                Some(turn_id) => {
                                    opened.store.record_turn_with_id(turn_id.as_str(), commit)
                                }
                                None => opened.store.record_turn(commit),
                            };
                            if let Err(error) = result {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "warning: session persistence stopped: {error}"
                                )));
                                durable = None;
                            }
                        }
                        match result {
                            Ok(outcome) if outcome.stop_reason == StopReason::Steered => {
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "steer> checkpoint saved after {} model step(s); continuing with the new message",
                                    outcome.steps
                                )));
                                if approval.goal_dir().is_some() {
                                    let session_dir = approval
                                        .goal_dir()
                                        .and_then(|goal_dir| goal_dir.parent().map(PathBuf::from))
                                        .unwrap_or_else(|| world.workspace().to_path_buf());
                                    let _ = crate::goal::pause_goal(&session_dir);
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    let _ = events.send(ReplEvent::Notice(
                                        "goal> paused by steer; follow-up runs as a regular turn"
                                            .to_string(),
                                    ));
                                }
                                if let Some(next) = run_control.take_steer_input() {
                                    command = WorkerCommand::Prompt(next.text);
                                    continue;
                                }
                            }
                            Ok(outcome) if outcome.stop_reason == StopReason::StepLimit => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "warning: stopped after {} model steps",
                                    outcome.steps
                                )));
                                fail_active_goal(&approval, &mut goal_objective, world.workspace());
                            }
                            Ok(_outcome) => {
                                let Some(goal_dir) = approval.goal_dir() else {
                                    break;
                                };
                                let session_dir = goal_dir
                                    .parent()
                                    .map(PathBuf::from)
                                    .unwrap_or_else(|| world.workspace().to_path_buf());
                                let Some(checkpoint_seq) =
                                    durable.as_ref().map(|opened| opened.store.checkpoint_seq())
                                else {
                                    let _ = events.send(ReplEvent::Warning(
                                        "goal> cannot verify without a settled durable checkpoint"
                                            .to_string(),
                                    ));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        world.workspace(),
                                    );
                                    break;
                                };
                                let criteria =
                                    match crate::goal::goal_verification_criteria(&session_dir) {
                                        Ok(criteria) => criteria,
                                        Err(error) => {
                                            let _ = events.send(ReplEvent::Warning(format!(
                                                "goal> verifier unavailable: {error}"
                                            )));
                                            fail_active_goal(
                                                &approval,
                                                &mut goal_objective,
                                                world.workspace(),
                                            );
                                            break;
                                        }
                                    };
                                let (verifier_output, verdict) =
                                    match model_runtime.block_on(mentor::verify_checkpoint(
                                        &runtime_config,
                                        harness.messages(),
                                        &criteria,
                                    )) {
                                        Ok(result) => result,
                                        Err(error) => {
                                            let _ = events.send(ReplEvent::Warning(format!(
                                                "goal> verifier failed: {error}"
                                            )));
                                            fail_active_goal(
                                                &approval,
                                                &mut goal_objective,
                                                world.workspace(),
                                            );
                                            break;
                                        }
                                    };
                                if let Err(error) = crate::goal::record_verifier_verdict(
                                    &session_dir,
                                    checkpoint_seq,
                                    &verifier_output,
                                ) {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "goal> cannot persist verifier verdict: {error}"
                                    )));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        world.workspace(),
                                    );
                                    break;
                                }
                                if verdict.outcome == crate::goal::VerdictOutcome::Invalid {
                                    let _ = events.send(ReplEvent::Warning(
                                        "goal> verifier returned an invalid verdict; goal failed"
                                            .to_string(),
                                    ));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        world.workspace(),
                                    );
                                    break;
                                }
                                let next = match crate::goal::advance_goal_milestone(
                                    &session_dir,
                                    Some(verdict),
                                ) {
                                    Ok(next) => next,
                                    Err(error) => {
                                        let _ = events.send(ReplEvent::Warning(format!(
                                            "goal> cannot advance milestone: {error}"
                                        )));
                                        fail_active_goal(
                                            &approval,
                                            &mut goal_objective,
                                            world.workspace(),
                                        );
                                        break;
                                    }
                                };
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "goal> verifier: {:?} (milestone {}/{})",
                                    next.status, next.current_milestone, next.total_milestones
                                )));
                                if next.status == crate::goal::GoalStatus::Converged
                                    || next.status == crate::goal::GoalStatus::Failed
                                {
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    break;
                                }
                                let objective = goal_objective.clone().unwrap_or(prompt.clone());
                                command = WorkerCommand::Prompt(crate::goal::goal_turn_prompt(
                                    &objective,
                                    next.current_milestone,
                                    next.total_milestones,
                                ));
                                continue;
                            }
                            Err(error) => {
                                report_run_error(&events, &error);
                                fail_active_goal(&approval, &mut goal_objective, world.workspace());
                            }
                        }
                        if let Some(next) = run_control.take_steer_input() {
                            command = WorkerCommand::Prompt(next.text);
                            continue;
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
                        } else if let Some(opened) = &durable {
                            harness.set_id(ThreadId::new(opened.store.thread_id()));
                        }
                        harness.clear_history();
                        match world.model_context() {
                            Ok(context) => match harness.append_context(context) {
                                Ok(()) => {
                                    persist_latest_context(&mut durable, &harness, &events);
                                    let _ = events
                                        .send(ReplEvent::Notice("new conversation".to_string()));
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
                    WorkerCommand::ShowStatus => {
                        let mode_str = match approval.mode() {
                            ApprovalMode::Automatic => "automatic (auto-approve)",
                            ApprovalMode::Interactive => "interactive (prompt on shell/sensitive)",
                        };
                        let copilot_str = if copilot {
                            if auto_max_steps == 0 {
                                "on (unlimited steps)".to_string()
                            } else {
                                format!("on (max {auto_max_steps} steps)")
                            }
                        } else {
                            "off".to_string()
                        };
                        let session_str = if let Some(opened) = &durable {
                            format!(
                                "{} (thread {}) [durable: {}]",
                                opened.store.session_id(),
                                opened.store.thread_id(),
                                opened.store.path().display()
                            )
                        } else {
                            "ephemeral (in-memory)".to_string()
                        };
                        let workspace_str = world.workspace().display().to_string();

                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> workspace:        {workspace_str}"
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> security-preset:  {preset}"
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> sandbox:          {sandbox_kind}"
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> approval:         {mode_str}"
                        )));
                        let web_search_str = if web_search_enabled {
                            "enabled (built-in responses web_search)"
                        } else {
                            "disabled"
                        };
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> web search:       {web_search_str}"
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> copilot mode:     {copilot_str}"
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> session:          {session_str}"
                        )));
                    }
                    WorkerCommand::ShowWorld => {
                        for line in world.status_lines() {
                            let _ = events.send(ReplEvent::Notice(format!("world> {line}")));
                        }
                    }
                    WorkerCommand::RefreshWorld => {
                        let refreshed = WorldState::detect(
                            world.workspace(),
                            world.approval(),
                            world.copilot(),
                            world.sandbox(),
                        );
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
                                let _ = events
                                    .send(ReplEvent::Warning(format!("warning: {diagnostic}")));
                            }
                            retry_mcp_servers.retain(|server| {
                                !loaded_servers.contains(&format!(
                                    "{}/{}",
                                    server.plugin_name, server.server_name
                                ))
                            });
                            let enabled = loaded_servers.iter().cloned().collect::<Vec<_>>();
                            let tool_count = tools.len();
                            harness.extend_tools(classify_tools(tools));
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
                    WorkerCommand::SetExecution {
                        approval: mode,
                        copilot: new_copilot,
                    } => {
                        if !new_copilot && let Some(goal_dir) = approval.goal_dir() {
                            let session_dir = goal_dir
                                .parent()
                                .map(PathBuf::from)
                                .unwrap_or_else(|| world.workspace().to_path_buf());
                            let _ = crate::goal::pause_goal(&session_dir);
                            approval.set_goal_dir(None);
                            goal_objective = None;
                        }
                        copilot = new_copilot;
                        approval.set_mode(mode);
                        let mut config = harness_config_auto(copilot, auto_max_steps);
                        config.system_prompt.clone_from(&stable_system_prompt);
                        harness.replace_config(config);
                        let updated_world = world.with_execution(mode, copilot, sandbox_kind);
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
                    WorkerCommand::SetPlanMode { active, prompt } => {
                        let session_dir = durable
                            .as_ref()
                            .and_then(|opened| opened.store.path().parent())
                            .unwrap_or(world.workspace());
                        if active {
                            match crate::goal::init_plan_mode_with_prompt(
                                session_dir,
                                prompt.as_deref(),
                            ) {
                                Ok(plan_file) => {
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    approval.set_living_plan(Some(plan_file.clone()));
                                    let mut config = harness_config_auto(copilot, auto_max_steps);
                                    config.system_prompt =
                                        crate::goal::with_plan_mode_overlay(&stable_system_prompt);
                                    harness.replace_config(config);
                                    let _ = harness.append_context(format!(
                                    "[Plan Mode active: living plan at {}. Plan only — research and update plan.md. Do not produce the final deliverable. Relative path plan.md maps to that file. Workspace modifications are locked.]",
                                    plan_file.display()
                                ));
                                    persist_latest_context(&mut durable, &harness, &events);
                                    let _ = events.send(ReplEvent::Notice(format!(
                                    "plan mode on: workspace modifications locked. Living plan at {}",
                                    plan_file.display()
                                )));
                                    if let Some(prompt) = prompt {
                                        command = WorkerCommand::Prompt(prompt);
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "error: cannot init plan mode: {e}"
                                    )));
                                }
                            }
                        } else {
                            approval.set_living_plan(None);
                            let _ = crate::goal::disable_plan_mode(session_dir);
                            let mut config = harness_config_auto(copilot, auto_max_steps);
                            config.system_prompt.clone_from(&stable_system_prompt);
                            harness.replace_config(config);
                            let _ = events.send(ReplEvent::Notice(
                                "plan mode off: resumed standard execution mode".to_string(),
                            ));
                        }
                    }
                    WorkerCommand::StartGoal(objective) => {
                        if durable.is_none() {
                            let _ = events.send(ReplEvent::Warning(
                                "goal> requires a durable session; restart without --ephemeral"
                                    .to_string(),
                            ));
                            break;
                        }
                        if let Err(error) = runtime_config.mentor_provider_settings() {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "goal> requires an independent verifier: {error}"
                            )));
                            break;
                        }
                        let session_dir = durable
                            .as_ref()
                            .and_then(|opened| opened.store.path().parent())
                            .unwrap_or(world.workspace());
                        match crate::goal::init_goal_workspace(session_dir, &objective, 20) {
                            Ok(state) => {
                                goal_objective = Some(objective.clone());
                                approval.set_living_plan(None);
                                let goal_dir = session_dir.join("goal");
                                approval.set_goal_dir(Some(goal_dir.clone()));
                                let _ = crate::goal::disable_plan_mode(session_dir);
                                copilot = true;
                                approval.set_mode(ApprovalMode::Automatic);
                                let mut config = harness_config_auto(true, auto_max_steps);
                                config.max_steps = if config.max_steps == 0 {
                                    state.milestone_step_budget
                                } else {
                                    config.max_steps.min(state.milestone_step_budget)
                                };
                                config.system_prompt.clone_from(&stable_system_prompt);
                                harness.replace_config(config);
                                let goal_plan = goal_dir.join("plan.md");
                                let _ = harness.append_context(format!(
                                "[Autonomous Goal Mode active: goal_id={}. Execute now. Current milestone {}/{}. Goal plan at {}. Relative path goal/plan.md maps to that file. Workspace mutations are allowed.]",
                                state.goal_id, state.current_milestone, state.total_milestones, goal_plan.display()
                            ));
                                persist_latest_context(&mut durable, &harness, &events);
                                let _ = events.send(ReplEvent::Notice(format!(
                                "goal mode on [goal_id: {}]: executing milestone {}/{} (auto-approve, copilot on)",
                                state.goal_id, state.current_milestone, state.total_milestones
                            )));
                                command = WorkerCommand::Prompt(crate::goal::goal_turn_prompt(
                                    &objective,
                                    state.current_milestone,
                                    state.total_milestones,
                                ));
                                continue;
                            }
                            Err(e) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot start goal mode: {e}"
                                )));
                            }
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

fn fail_active_goal(
    approval: &ApprovalController,
    goal_objective: &mut Option<String>,
    fallback_session_dir: &Path,
) {
    if let Some(goal_dir) = approval.goal_dir() {
        let session_dir = goal_dir.parent().unwrap_or(fallback_session_dir);
        let _ = crate::goal::fail_goal(session_dir);
        approval.set_goal_dir(None);
        *goal_objective = None;
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
        "/plan [prompt] Enable Plan Mode and optionally start drafting the session living plan"
    );
    println!("/plan off      Exit Plan Mode and resume standard execution");
    println!(
        "/goal <goal>   Start Goal Mode and immediately execute the objective (auto-approve, copilot on)"
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
