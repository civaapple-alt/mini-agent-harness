//! Worker-side REPL orchestration.
//!
//! The parent module owns terminal input and rendering; this module owns the
//! App Server worker lifecycle and workflow execution.

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
        let web_search_enabled = launch.web_search_enabled();
        let runtime_config = launch.runtime_config();
        let workflow_scope = launch.workflow_scope();
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
        let mut copilot = copilot;
        let mut goal_objective: Option<String> = None;
        let mut plan_active = model_runtime
            .block_on(runtime.client_mut().workflow_state())
            .map(|state| state.plan_active)
            .unwrap_or(false);
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
                if let Ok(state) = model_runtime.block_on(runtime.client_mut().workflow_state())
                    && state
                        .goal
                        .is_some_and(|goal| goal.status == WorkflowGoalStatus::Running)
                {
                    let _ = model_runtime.block_on(runtime.client_mut().pause_goal());
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
            if let Ok(checkpoint) = model_runtime.block_on(runtime.client_mut().read_checkpoint()) {
                let _ = runtime.client_mut().record_context(&checkpoint);
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
                        match prompt::run_prompt(prompt::PromptContext {
                            prompt,
                            plan_active,
                            run_control: &run_control,
                            runtime: &mut runtime,
                            model_runtime: &model_runtime,
                            approval: &approval,
                            goal_objective: &mut goal_objective,
                            events: &events,
                            verify_goal_checkpoint:
                                |messages: &[mini_agent_app_server::frontend::Message],
                                 criteria: &str| {
                                    model_runtime.block_on(verifier::verify_goal_checkpoint(
                                        &runtime_config,
                                        messages,
                                        criteria,
                                    ))
                                },
                        }) {
                            prompt::PromptOutcome::Finished => {}
                            prompt::PromptOutcome::Continue(next) => {
                                command = next;
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
                        if !new_copilot && goal_objective.is_some() {
                            let _ = model_runtime.block_on(runtime.client_mut().pause_goal());
                            approval.set_goal_dir(None);
                            goal_objective = None;
                        }
                        copilot = new_copilot;
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
                            match model_runtime
                                .block_on(runtime.client_mut().set_plan_mode(true, prompt.clone()))
                            {
                                Ok(_) => {
                                    let plan_file = model_runtime
                                        .block_on(runtime.client_mut().session_info())
                                        .ok()
                                        .flatten()
                                        .and_then(|session| {
                                            std::path::Path::new(&session.path)
                                                .parent()
                                                .map(std::path::Path::to_path_buf)
                                        })
                                        .unwrap_or_else(|| {
                                            std::path::PathBuf::from(&world.workspace)
                                        })
                                        .join("plan.md");
                                    plan_active = true;
                                    approval.set_goal_dir(None);
                                    goal_objective = None;
                                    approval.set_living_plan(Some(plan_file.clone()));
                                    let mut config = harness_config_auto(copilot, auto_max_steps);
                                    config.system_prompt =
                                        workflow_api::with_plan_mode_overlay(&stable_system_prompt);
                                    let _ = model_runtime.block_on(
                                        runtime
                                            .client_mut()
                                            .update_thread(ThreadUpdate::ReplaceConfig(config)),
                                    );
                                    let context = format!(
                                        "[Plan Mode active: living plan at {}. Plan only — research and update plan.md. Do not produce the final deliverable. Relative path plan.md maps to that file. Workspace modifications are locked.]",
                                        plan_file.display()
                                    );
                                    let _ = model_runtime.block_on(
                                        runtime
                                            .client_mut()
                                            .update_thread(ThreadUpdate::AppendContext(context)),
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
                                        "error: cannot init plan mode: {}",
                                        e.message
                                    )));
                                }
                            }
                        } else {
                            approval.set_living_plan(None);
                            plan_active = false;
                            let _ = model_runtime
                                .block_on(runtime.client_mut().set_plan_mode(false, None));
                            let mut config = harness_config_auto(copilot, auto_max_steps);
                            config.system_prompt.clone_from(&stable_system_prompt);
                            let _ = model_runtime.block_on(
                                runtime
                                    .client_mut()
                                    .update_thread(ThreadUpdate::ReplaceConfig(config)),
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
                        if let Err(error) = runtime_config.verifier_provider_settings() {
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
                        match model_runtime
                            .block_on(runtime.client_mut().start_goal(objective.clone()))
                        {
                            Ok(state) => {
                                goal_objective = Some(objective.clone());
                                approval.set_living_plan(None);
                                let goal_dir = session_dir.join("goal");
                                approval.set_goal_dir(Some(goal_dir.clone()));
                                plan_active = false;
                                let _ = model_runtime
                                    .block_on(runtime.client_mut().set_plan_mode(false, None));
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
                                    runtime
                                        .client_mut()
                                        .update_thread(ThreadUpdate::ReplaceConfig(config)),
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
                                    runtime
                                        .client_mut()
                                        .update_thread(ThreadUpdate::AppendContext(context)),
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
                                    "error: cannot start goal mode: {}",
                                    e.message
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
        .block_on(runtime.client_mut().read_checkpoint())
        .and_then(|checkpoint| runtime.client_mut().record_context(&checkpoint))
    {
        let _ = events.send(ReplEvent::Warning(format!(
            "warning: session persistence stopped: {error}"
        )));
    }
}

fn fail_active_goal(
    runtime: &mut AppServerRuntime,
    model_runtime: &tokio::runtime::Runtime,
    approval: &ApprovalController,
    goal_objective: &mut Option<String>,
) {
    if goal_objective.is_some() {
        let _ = model_runtime.block_on(runtime.client_mut().fail_goal());
        approval.set_goal_dir(None);
        *goal_objective = None;
    }
}

fn protocol_verifier_verdict(verdict: &workflow_api::VerifierVerdict) -> WorkflowVerifierVerdict {
    WorkflowVerifierVerdict {
        outcome: match verdict.outcome {
            workflow_api::VerdictOutcome::Approved => WorkflowVerdictOutcome::Approved,
            workflow_api::VerdictOutcome::Rejected => WorkflowVerdictOutcome::Rejected,
            workflow_api::VerdictOutcome::NeedsClarification => {
                WorkflowVerdictOutcome::NeedsClarification
            }
            workflow_api::VerdictOutcome::Invalid => WorkflowVerdictOutcome::Invalid,
        },
        score: verdict.score,
        summary: verdict.summary.clone(),
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
