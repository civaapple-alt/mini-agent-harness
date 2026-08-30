use super::*;

pub(super) enum PromptOutcome {
    Finished,
    Continue(WorkerCommand),
}

pub(super) struct PromptContext<'a, F> {
    pub(super) prompt: String,
    pub(super) plan_active: bool,
    pub(super) run_control: &'a RunControl,
    pub(super) runtime: &'a mut AppServerRuntime,
    pub(super) model_runtime: &'a tokio::runtime::Runtime,
    pub(super) approval: &'a ApprovalController,
    pub(super) goal_objective: &'a mut Option<String>,
    pub(super) events: &'a mpsc::SyncSender<ReplEvent>,
    pub(super) verify_goal_checkpoint: F,
}

pub(super) fn run_prompt<F>(context: PromptContext<'_, F>) -> PromptOutcome
where
    F: Fn(
        &[mini_agent_app_server::frontend::Message],
        &str,
    ) -> Result<(String, workflow_api::VerifierVerdict), String>,
{
    let PromptContext {
        prompt,
        plan_active,
        run_control,
        runtime,
        model_runtime,
        approval,
        goal_objective,
        events,
        verify_goal_checkpoint,
    } = context;
    run_control.clear_steer();
    let prompt = if plan_active {
        workflow_api::planning_turn_prompt(&prompt)
    } else {
        prompt
    };
    let mut observer = ChannelObserver(events.clone());
    let goal_timeout = if goal_objective.is_some() {
        model_runtime
            .block_on(runtime.client_mut().workflow_state())
            .ok()
            .and_then(|state| state.goal)
            .map(|state| Duration::from_secs(state.milestone_timeout_secs))
    } else {
        None
    };
    let result = if let Some(timeout) = goal_timeout {
        match model_runtime.block_on(async {
            tokio::time::timeout(
                timeout,
                runtime
                    .client_mut()
                    .run_turn_batch(prompt.clone(), &mut observer),
            )
            .await
        }) {
            Ok(result) => result,
            Err(_) => {
                let _ = events.send(ReplEvent::Warning(format!(
                    "goal> milestone timed out after {} seconds",
                    timeout.as_secs()
                )));
                fail_active_goal(runtime, model_runtime, approval, goal_objective);
                return PromptOutcome::Finished;
            }
        }
    } else {
        model_runtime.block_on(
            runtime
                .client_mut()
                .run_turn_batch(prompt.clone(), &mut observer),
        )
    };
    let batch = match result {
        Ok(batch) => batch,
        Err(error) => {
            report_run_error(events, &error);
            fail_active_goal(runtime, model_runtime, approval, goal_objective);
            return PromptOutcome::Finished;
        }
    };
    let Some(outcome) = batch.turns.last() else {
        let _ = events.send(ReplEvent::Warning(
            "error: app server returned an empty turn batch".to_string(),
        ));
        return PromptOutcome::Finished;
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
            if goal_objective.is_some() {
                let _ = model_runtime.block_on(runtime.client_mut().pause_goal());
                approval.set_goal_dir(None);
                *goal_objective = None;
                let _ = events.send(ReplEvent::Notice(
                    "goal> paused by steer; follow-up runs as a regular turn".to_string(),
                ));
            }
        }
        (_, true) => {
            let _ = events.send(ReplEvent::Warning(format!(
                "warning: stopped after {} model steps",
                outcome.steps
            )));
            fail_active_goal(runtime, model_runtime, approval, goal_objective);
        }
        _ => {
            if goal_objective.is_none() {
                return PromptOutcome::Finished;
            }
            let Some(checkpoint_seq) =
                model_runtime.block_on(runtime.client_mut().checkpoint_seq())
            else {
                let _ = events.send(ReplEvent::Warning(
                    "goal> cannot verify without a settled durable checkpoint".to_string(),
                ));
                fail_active_goal(runtime, model_runtime, approval, goal_objective);
                return PromptOutcome::Finished;
            };
            let criteria = match model_runtime.block_on(runtime.client_mut().goal_criteria()) {
                Ok(result) => result.criteria,
                Err(error) => {
                    let _ = events.send(ReplEvent::Warning(format!(
                        "goal> verifier unavailable: {}",
                        error.message
                    )));
                    fail_active_goal(runtime, model_runtime, approval, goal_objective);
                    return PromptOutcome::Finished;
                }
            };
            let checkpoint = match model_runtime.block_on(runtime.client_mut().read_checkpoint()) {
                Ok(checkpoint) => checkpoint,
                Err(error) => {
                    let _ = events.send(ReplEvent::Warning(format!(
                        "goal> checkpoint unavailable: {error}"
                    )));
                    fail_active_goal(runtime, model_runtime, approval, goal_objective);
                    return PromptOutcome::Finished;
                }
            };
            let (verifier_output, verdict) =
                match verify_goal_checkpoint(checkpoint.session.messages(), &criteria) {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = events.send(ReplEvent::Warning(format!(
                            "goal> verifier failed: {error}"
                        )));
                        fail_active_goal(runtime, model_runtime, approval, goal_objective);
                        return PromptOutcome::Finished;
                    }
                };
            if let Err(error) = model_runtime.block_on(
                runtime
                    .client_mut()
                    .record_verifier_verdict(checkpoint_seq, &verifier_output),
            ) {
                let _ = events.send(ReplEvent::Warning(format!(
                    "goal> cannot persist verifier verdict: {}",
                    error.message
                )));
                fail_active_goal(runtime, model_runtime, approval, goal_objective);
                return PromptOutcome::Finished;
            }
            if verdict.outcome == workflow_api::VerdictOutcome::Invalid {
                let _ = events.send(ReplEvent::Warning(
                    "goal> verifier returned an invalid verdict; goal failed".to_string(),
                ));
                fail_active_goal(runtime, model_runtime, approval, goal_objective);
                return PromptOutcome::Finished;
            }
            let next = match model_runtime.block_on(runtime.client_mut().advance_goal(
                WorkflowGoalAdvanceParams {
                    verdict: Some(protocol_verifier_verdict(&verdict)),
                },
            )) {
                Ok(next) => next,
                Err(error) => {
                    let _ = events.send(ReplEvent::Warning(format!(
                        "goal> cannot advance milestone: {}",
                        error.message
                    )));
                    fail_active_goal(runtime, model_runtime, approval, goal_objective);
                    return PromptOutcome::Finished;
                }
            };
            let _ = events.send(ReplEvent::Notice(format!(
                "goal> verifier: {:?} (milestone {}/{})",
                next.status, next.current_milestone, next.total_milestones
            )));
            if matches!(
                next.status,
                WorkflowGoalStatus::Converged | WorkflowGoalStatus::Failed
            ) {
                approval.set_goal_dir(None);
                *goal_objective = None;
                return PromptOutcome::Finished;
            }
            let objective = goal_objective.clone().unwrap_or(prompt.clone());
            return PromptOutcome::Continue(WorkerCommand::Prompt(workflow_api::goal_turn_prompt(
                &objective,
                next.current_milestone,
                next.total_milestones,
            )));
        }
    }
    PromptOutcome::Finished
}
