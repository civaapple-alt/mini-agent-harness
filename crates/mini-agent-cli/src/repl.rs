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
use mini_agent_app_server::frontend::WorkflowScope;
use mini_agent_app_server::frontend::harness_config_auto;
use mini_agent_app_server::frontend::load_workspace_profile;
use mini_agent_app_server::frontend::observer::RunObserver;
use mini_agent_app_server::frontend::print_auto_warning;
use mini_agent_app_server::frontend::skills;
use mini_agent_app_server::local::LocalRuntimeRequest;
use mini_agent_app_server::mentor;
use mini_agent_app_server::workflows as workflow_api;
use std::collections::VecDeque;
use std::io;
use std::io::IsTerminal;
use std::io::Write;
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
                request_approval(&approval_events, action)
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
    let worker = spawn_worker(
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
        .map(|(name, _)| name.as_str())
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
                startup_profile.sandbox,
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
                    command
                        if command.strip_prefix("/plan").is_some_and(|rest| {
                            rest.is_empty() || rest.starts_with(char::is_whitespace)
                        }) =>
                    {
                        let action = match workflow_api::parse_plan_slash(command) {
                            Some(action) => action,
                            None => unreachable!("plan command matched its parser guard"),
                        };
                        match action {
                            workflow_api::PlanSlash::Disable => queue_work(
                                &worker_tx,
                                WorkerCommand::SetPlanMode {
                                    active: false,
                                    prompt: None,
                                },
                                &mut pending_work,
                            ),
                            workflow_api::PlanSlash::Enable { prompt } => queue_work(
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
        let auto_max_steps = launch.runtime_config.copilot_max_steps();
        let web_search_enabled = launch.runtime_config.web_search();
        let runtime_config = launch.runtime_config.clone();
        let workflow_scope = launch.profile.workflows;
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
        let workflow_service = runtime.workflows();
        let mut copilot = copilot;
        let mut goal_objective: Option<String> = None;
        let mut world = match model_runtime.block_on(runtime.client_mut().world_state()) {
            Ok(world) => world,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error.message));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let stable_system_prompt = runtime.stable_system_prompt().to_string();
        let mcp_status = match model_runtime.block_on(runtime.client_mut().mcp_status()) {
            Ok(status) => status,
            Err(error) => {
                let _ = events.send(ReplEvent::InitializationFailed(error.message));
                let _ = events.send(ReplEvent::Exited);
                return;
            }
        };
        let enabled_mcp_servers = mcp_status.enabled_servers;
        let mcp_tool_count = mcp_status.tool_count;
        if let Ok(Some(opened)) = model_runtime.block_on(runtime.client_mut().session_info()) {
            if opened.resumed {
                let _ = model_runtime.block_on(runtime.update_thread(ThreadUpdate::AppendContext(
                    "[Session resumed. Note: previously running background processes and result preview handles from prior sessions have expired.]".to_string(),
                )));
                let _ = model_runtime.block_on(
                    runtime.update_thread(ThreadUpdate::AppendContext(world.context.clone())),
                );
                if let Ok(Some(state)) = workflow_service.load_goal_state()
                    && state.status == workflow_api::GoalStatus::Running
                {
                    let _ = workflow_service.pause_goal();
                    let _ = events.send(ReplEvent::Warning(
                        "goal> paused on restart; reissue /goal to continue".to_string(),
                    ));
                }
            }
            let label = if opened.resumed { "resumed" } else { "new" };
            let _ = events.send(ReplEvent::Notice(format!(
                "session> {label} {} | thread {} | {}",
                opened.session_id, opened.thread_id, opened.path
            )));
            if let Ok(checkpoint) = model_runtime.block_on(runtime.read_checkpoint()) {
                let _ = runtime.record_context(&checkpoint);
            }
        }
        if !enabled_mcp_servers.is_empty() {
            let _ = events.send(ReplEvent::Notice(format!(
                "mcp> enabled — {} ({mcp_tool_count} tool(s))",
                bounded_names(&enabled_mcp_servers)
            )));
        }
        if !mcp_status.inactive_servers.is_empty() {
            let inactive = mcp_status.inactive_servers;
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
                            workflow_api::planning_turn_prompt(&prompt)
                        } else {
                            prompt
                        };
                        let started_at_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let mut observer = ChannelObserver(events.clone());
                        let goal_timeout = approval.goal_dir().and_then(|goal_dir| {
                            goal_dir
                                .parent()
                                .and_then(|_| workflow_service.load_goal_state().ok().flatten())
                                .map(|state| Duration::from_secs(state.milestone_timeout_secs))
                        });
                        let result = if let Some(timeout) = goal_timeout {
                            match model_runtime.block_on(async {
                                tokio::time::timeout(
                                    timeout,
                                    runtime.run_turn_batch(prompt.clone(), &mut observer),
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
                                        &workflow_service,
                                    );
                                    break;
                                }
                            }
                        } else {
                            model_runtime
                                .block_on(runtime.run_turn_batch(prompt.clone(), &mut observer))
                        };
                        let batch = match result {
                            Ok(batch) => batch,
                            Err(error) => {
                                report_run_error(&events, &error);
                                fail_active_goal(&approval, &mut goal_objective, &workflow_service);
                                break;
                            }
                        };
                        if let Err(error) = runtime.record_batch(started_at_ms, &prompt, &batch) {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "warning: session persistence stopped: {error}"
                            )));
                        }
                        let Some(outcome) = batch.turns.last() else {
                            let _ = events.send(ReplEvent::Warning(
                                "error: app server returned an empty turn batch".to_string(),
                            ));
                            break;
                        };
                        let steered = batch
                            .turns
                            .iter()
                            .any(|turn| turn.stop_reason == Some(StopReason::Steered));
                        let step_limited = outcome.status == TurnStatus::StepLimit;
                        match (steered, step_limited) {
                            (true, _) => {
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "steer> checkpoint saved after {} model step(s); continuing with the new message",
                                    outcome.steps
                                )));
                                if approval.goal_dir().is_some() {
                                    let _ = workflow_service.pause_goal();
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    let _ = events.send(ReplEvent::Notice(
                                        "goal> paused by steer; follow-up runs as a regular turn"
                                            .to_string(),
                                    ));
                                }
                            }
                            (_, true) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "warning: stopped after {} model steps",
                                    outcome.steps
                                )));
                                fail_active_goal(&approval, &mut goal_objective, &workflow_service);
                            }
                            _ => {
                                let Some(_goal_dir) = approval.goal_dir() else {
                                    break;
                                };
                                let Some(checkpoint_seq) = runtime.checkpoint_seq() else {
                                    let _ = events.send(ReplEvent::Warning(
                                        "goal> cannot verify without a settled durable checkpoint"
                                            .to_string(),
                                    ));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        &workflow_service,
                                    );
                                    break;
                                };
                                let criteria = match workflow_service.verification_criteria() {
                                    Ok(criteria) => criteria,
                                    Err(error) => {
                                        let _ = events.send(ReplEvent::Warning(format!(
                                            "goal> verifier unavailable: {error}"
                                        )));
                                        fail_active_goal(
                                            &approval,
                                            &mut goal_objective,
                                            &workflow_service,
                                        );
                                        break;
                                    }
                                };
                                let checkpoint =
                                    match model_runtime.block_on(runtime.read_checkpoint()) {
                                        Ok(checkpoint) => checkpoint,
                                        Err(error) => {
                                            let _ = events.send(ReplEvent::Warning(format!(
                                                "goal> checkpoint unavailable: {error}"
                                            )));
                                            fail_active_goal(
                                                &approval,
                                                &mut goal_objective,
                                                &workflow_service,
                                            );
                                            break;
                                        }
                                    };
                                let (verifier_output, verdict) =
                                    match model_runtime.block_on(mentor::verify_checkpoint(
                                        &runtime_config,
                                        checkpoint.session.messages(),
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
                                                &workflow_service,
                                            );
                                            break;
                                        }
                                    };
                                if let Err(error) = workflow_service
                                    .record_verifier_verdict(checkpoint_seq, &verifier_output)
                                {
                                    let _ = events.send(ReplEvent::Warning(format!(
                                        "goal> cannot persist verifier verdict: {error}"
                                    )));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        &workflow_service,
                                    );
                                    break;
                                }
                                if verdict.outcome == workflow_api::VerdictOutcome::Invalid {
                                    let _ = events.send(ReplEvent::Warning(
                                        "goal> verifier returned an invalid verdict; goal failed"
                                            .to_string(),
                                    ));
                                    fail_active_goal(
                                        &approval,
                                        &mut goal_objective,
                                        &workflow_service,
                                    );
                                    break;
                                }
                                let next = match workflow_service.advance_goal(Some(verdict)) {
                                    Ok(next) => next,
                                    Err(error) => {
                                        let _ = events.send(ReplEvent::Warning(format!(
                                            "goal> cannot advance milestone: {error}"
                                        )));
                                        fail_active_goal(
                                            &approval,
                                            &mut goal_objective,
                                            &workflow_service,
                                        );
                                        break;
                                    }
                                };
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "goal> verifier: {:?} (milestone {}/{})",
                                    next.status, next.current_milestone, next.total_milestones
                                )));
                                if next.status == workflow_api::GoalStatus::Converged
                                    || next.status == workflow_api::GoalStatus::Failed
                                {
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    break;
                                }
                                let objective = goal_objective.clone().unwrap_or(prompt.clone());
                                command = WorkerCommand::Prompt(workflow_api::goal_turn_prompt(
                                    &objective,
                                    next.current_milestone,
                                    next.total_milestones,
                                ));
                                continue;
                            }
                        }
                    }
                    WorkerCommand::ClearHistory => {
                        let has_session = model_runtime
                            .block_on(runtime.client_mut().session_info())
                            .ok()
                            .flatten()
                            .is_some();
                        if has_session
                            && let Err(error) = model_runtime.block_on(runtime.start_new_thread())
                        {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "warning: session persistence stopped: {error}"
                            )));
                        }
                        if let Some(error) = model_runtime
                            .block_on(runtime.update_thread(ThreadUpdate::ClearHistory))
                            .err()
                        {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "error: cannot clear conversation: {error}"
                            )));
                            continue;
                        }
                        match model_runtime.block_on(
                            runtime
                                .update_thread(ThreadUpdate::AppendContext(world.context.clone())),
                        ) {
                            Ok(()) => {
                                persist_latest_context(&mut runtime, &model_runtime, &events);
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
                        let session_str = if let Ok(Some(opened)) =
                            model_runtime.block_on(runtime.client_mut().session_info())
                        {
                            format!(
                                "{} (thread {}) [durable: {}]",
                                opened.session_id, opened.thread_id, opened.path
                            )
                        } else {
                            "unavailable".to_string()
                        };
                        let workspace_str = world.workspace.clone();

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
                        let manifest = runtime.capability_manifest();
                        let disabled = manifest
                            .disabled
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(",");
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> capabilities:      profile={} enabled={} disabled={}",
                            manifest.profile,
                            manifest.enabled.join(","),
                            disabled
                        )));
                        let _ = events.send(ReplEvent::Notice(format!(
                            "status> session:          {session_str}"
                        )));
                    }
                    WorkerCommand::ShowWorld => {
                        for line in &world.lines {
                            let _ = events.send(ReplEvent::Notice(format!("world> {line}")));
                        }
                    }
                    WorkerCommand::RefreshWorld => {
                        match model_runtime.block_on(runtime.client_mut().refresh_world()) {
                            Ok(result) if result.changed => {
                                world = result.state;
                                let _ = events.send(ReplEvent::Notice(
                                    "world> refreshed and appended to context".to_string(),
                                ));
                            }
                            Ok(_result) => {
                                let _ = events.send(ReplEvent::Notice(
                                    "world> unchanged; no context item appended".to_string(),
                                ));
                            }
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "error: cannot refresh world state: {}",
                                    error.message
                                )));
                            }
                        }
                    }
                    WorkerCommand::EnableMcp => {
                        let status = match model_runtime.block_on(runtime.client_mut().mcp_status())
                        {
                            Ok(status) => status,
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "mcp> cannot read status: {}",
                                    error.message
                                )));
                                continue;
                            }
                        };
                        if !status.retry_available {
                            let _ = events.send(ReplEvent::Notice(
                                "no MCP servers are waiting to be enabled".to_string(),
                            ));
                        } else {
                            let result =
                                match model_runtime.block_on(runtime.client_mut().retry_mcp()) {
                                    Ok(result) => result,
                                    Err(error) => {
                                        let _ = events.send(ReplEvent::Warning(format!(
                                            "mcp> cannot enable tools: {}",
                                            error.message
                                        )));
                                        continue;
                                    }
                                };
                            for diagnostic in result.diagnostics {
                                let _ = events
                                    .send(ReplEvent::Warning(format!("warning: {diagnostic}")));
                            }
                            let message = if result.enabled_servers.is_empty() {
                                "mcp> inactive — no servers enabled; use /mcp to retry".to_string()
                            } else {
                                format!(
                                    "mcp> enabled — {} ({tool_count} tool(s))",
                                    bounded_names(&result.enabled_servers),
                                    tool_count = result.tool_count,
                                )
                            };
                            let _ = events.send(ReplEvent::Notice(message));
                            if !result.inactive_servers.is_empty() {
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "mcp> inactive — {}; use /mcp to retry",
                                    bounded_names(&result.inactive_servers)
                                )));
                            }
                        }
                    }
                    WorkerCommand::ShowSession => {
                        match model_runtime.block_on(runtime.client_mut().session_info()) {
                            Ok(Some(session)) => {
                                let _ = events.send(ReplEvent::Notice(format!(
                                    "session> durable {} | thread {} | {}",
                                    session.session_id, session.thread_id, session.path
                                )));
                            }
                            Ok(None) => {
                                let _ = events.send(ReplEvent::Notice(
                                    "session> no durable session is attached".to_string(),
                                ));
                            }
                            Err(error) => {
                                let _ = events.send(ReplEvent::Warning(format!(
                                    "session> cannot read session info: {}",
                                    error.message
                                )));
                            }
                        }
                    }
                    WorkerCommand::SetExecution {
                        approval: mode,
                        copilot: new_copilot,
                    } => {
                        if !new_copilot && approval.goal_dir().is_some() {
                            let _ = workflow_service.pause_goal();
                            approval.set_goal_dir(None);
                            goal_objective = None;
                        }
                        copilot = new_copilot;
                        approval.set_mode(mode);
                        let mut config = harness_config_auto(copilot, auto_max_steps);
                        config.system_prompt.clone_from(&stable_system_prompt);
                        if let Err(error) = model_runtime
                            .block_on(runtime.update_thread(ThreadUpdate::ReplaceConfig(config)))
                        {
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
                    WorkerCommand::SetPlanMode { active, ref prompt } => {
                        if active
                            && !matches!(
                                workflow_scope,
                                WorkflowScope::Plan | WorkflowScope::PlanAndGoal
                            )
                        {
                            let _ = events.send(ReplEvent::Warning(
                                "plan> disabled by the active runtime profile".to_string(),
                            ));
                            continue;
                        }
                        if active {
                            match workflow_service.init_plan_mode(prompt.as_deref()) {
                                Ok(plan_file) => {
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    approval.set_living_plan(Some(plan_file.clone()));
                                    let mut config = harness_config_auto(copilot, auto_max_steps);
                                    config.system_prompt =
                                        workflow_api::with_plan_mode_overlay(&stable_system_prompt);
                                    let _ =
                                        model_runtime
                                            .block_on(runtime.update_thread(
                                                ThreadUpdate::ReplaceConfig(config),
                                            ));
                                    let context = format!(
                                        "[Plan Mode active: living plan at {}. Plan only — research and update plan.md. Do not produce the final deliverable. Relative path plan.md maps to that file. Workspace modifications are locked.]",
                                        plan_file.display()
                                    );
                                    let _ = model_runtime.block_on(
                                        runtime.update_thread(ThreadUpdate::AppendContext(context)),
                                    );
                                    persist_latest_context(&mut runtime, &model_runtime, &events);
                                    let _ = events.send(ReplEvent::Notice(format!(
                                    "plan mode on: workspace modifications locked. Living plan at {}",
                                    plan_file.display()
                                )));
                                    if let Some(prompt) = prompt.clone() {
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
                            let _ = workflow_service.disable_plan_mode();
                            let mut config = harness_config_auto(copilot, auto_max_steps);
                            config.system_prompt.clone_from(&stable_system_prompt);
                            let _ = model_runtime.block_on(
                                runtime.update_thread(ThreadUpdate::ReplaceConfig(config)),
                            );
                            let _ = events.send(ReplEvent::Notice(
                                "plan mode off: resumed standard execution mode".to_string(),
                            ));
                        }
                    }
                    WorkerCommand::StartGoal(ref objective) => {
                        if !matches!(
                            workflow_scope,
                            WorkflowScope::Goal | WorkflowScope::PlanAndGoal
                        ) {
                            let _ = events.send(ReplEvent::Warning(
                                "goal> disabled by the active runtime profile".to_string(),
                            ));
                            continue;
                        }
                        let session = model_runtime
                            .block_on(runtime.client_mut().session_info())
                            .ok()
                            .flatten();
                        if session.is_none() {
                            let _ = events.send(ReplEvent::Warning(
                                "goal> requires a durable session".to_string(),
                            ));
                            break;
                        }
                        if let Err(error) = runtime_config.mentor_provider_settings() {
                            let _ = events.send(ReplEvent::Warning(format!(
                                "goal> requires an independent verifier: {error}"
                            )));
                            break;
                        }
                        let session_dir = session
                            .and_then(|opened| {
                                std::path::Path::new(&opened.path)
                                    .parent()
                                    .map(std::path::Path::to_path_buf)
                            })
                            .unwrap_or_else(|| std::path::PathBuf::from(&world.workspace));
                        match workflow_service.init_goal(objective) {
                            Ok(state) => {
                                goal_objective = Some(objective.clone());
                                approval.set_living_plan(None);
                                let goal_dir = session_dir.join("goal");
                                approval.set_goal_dir(Some(goal_dir.clone()));
                                let _ = workflow_service.disable_plan_mode();
                                copilot = true;
                                approval.set_mode(ApprovalMode::Automatic);
                                let mut config = harness_config_auto(true, auto_max_steps);
                                config.max_steps = if config.max_steps == 0 {
                                    state.milestone_step_budget
                                } else {
                                    config.max_steps.min(state.milestone_step_budget)
                                };
                                config.system_prompt.clone_from(&stable_system_prompt);
                                let _ = model_runtime.block_on(
                                    runtime.update_thread(ThreadUpdate::ReplaceConfig(config)),
                                );
                                let goal_plan = goal_dir.join("plan.md");
                                let context = format!(
                                    "[Autonomous Goal Mode active: goal_id={}. Execute now. Current milestone {}/{}. Goal plan at {}. Relative path goal/plan.md maps to that file. Workspace mutations are allowed.]",
                                    state.goal_id,
                                    state.current_milestone,
                                    state.total_milestones,
                                    goal_plan.display()
                                );
                                let _ = model_runtime.block_on(
                                    runtime.update_thread(ThreadUpdate::AppendContext(context)),
                                );
                                persist_latest_context(&mut runtime, &model_runtime, &events);
                                let _ = events.send(ReplEvent::Notice(format!(
                                "goal mode on [goal_id: {}]: executing milestone {}/{} (auto-approve, copilot on)",
                                state.goal_id, state.current_milestone, state.total_milestones
                            )));
                                command = WorkerCommand::Prompt(workflow_api::goal_turn_prompt(
                                    objective,
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
    runtime: &mut AppServerRuntime,
    model_runtime: &tokio::runtime::Runtime,
    events: &mpsc::SyncSender<ReplEvent>,
) {
    if let Err(error) = model_runtime
        .block_on(runtime.read_checkpoint())
        .and_then(|checkpoint| runtime.record_context(&checkpoint))
    {
        let _ = events.send(ReplEvent::Warning(format!(
            "warning: session persistence stopped: {error}"
        )));
    }
}

fn fail_active_goal(
    approval: &ApprovalController,
    goal_objective: &mut Option<String>,
    workflows: &mini_agent_app_server::WorkflowService,
) {
    if approval.goal_dir().is_some() {
        let _ = workflows.fail_goal();
        approval.set_goal_dir(None);
        *goal_objective = None;
    }
}

fn report_run_error(events: &mpsc::SyncSender<ReplEvent>, error: &str) {
    let _ = events.send(ReplEvent::Warning(format!("error: {error}")));
    if error.contains("context") {
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
