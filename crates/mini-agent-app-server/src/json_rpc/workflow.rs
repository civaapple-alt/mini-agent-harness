use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_thread_goal_set(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadGoalSetParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows
            .set_goal_action(params.objective, params.status, params.token_budget)
            .await
        {
            Ok(response) => {
                let goal = thread_goal_state(params.thread_id, response.value.clone());
                response_action_with(request.id, response, ThreadGoalSetResponse { goal })
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_thread_goal_get(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadGoalGetParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.get_goal_action().await {
            Ok(response) => {
                let goal = response
                    .value
                    .clone()
                    .map(|state| thread_goal_state(params.thread_id.clone(), state));
                response_action_with(request.id, response, ThreadGoalGetResponse { goal })
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_thread_goal_clear(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadGoalClearParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.clear_goal_action().await {
            Ok(response) => {
                let cleared = response.value;
                response_action_with(request.id, response, ThreadGoalClearResponse { cleared })
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_state(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.state_action().await {
            Ok(response) => {
                let (plan_active, goal) = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    WorkflowState {
                        collaboration_mode: collaboration_mode(plan_active),
                        goal: goal.map(workflow_goal_state),
                    },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_goal_start(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WorkflowGoalStartParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.init_goal_action(&params.objective).await {
            Ok(response) => workflow_goal_response(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_goal_pause(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        let response = match workflows.pause_goal_action().await {
            Ok(response) => response,
            Err(error) => return response_error(request.id, map_action_error(error)),
        };
        match workflows.load_goal_state().await {
            Ok(Some(state)) => {
                response_action_with(request.id, response, workflow_goal_state(state))
            }
            Ok(None) => response_error(request.id, JsonRpcError::server_error("no active goal")),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
        }
    }

    pub(super) async fn handle_workflow_goal_fail(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.fail_goal_action().await {
            Ok(response) => workflow_goal_response(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_goal_criteria(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows.verification_criteria_action().await {
            Ok(response) => {
                let criteria = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    mini_agent_app_server_protocol::WorkflowGoalCriteriaResult { criteria },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_goal_advance(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WorkflowGoalAdvanceParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        let verdict = params.verdict.map(host_verifier_verdict);
        match workflows.advance_goal_action(verdict).await {
            Ok(response) => workflow_goal_response(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_workflow_goal_record_verdict(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WorkflowGoalRecordVerdictParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        match workflows
            .record_verifier_verdict_action(params.checkpoint_seq, &params.output)
            .await
        {
            Ok(response) => {
                response_action_with(request.id, response, serde_json::json!({"recorded": true}))
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) fn workflow_service(&self) -> Result<&WorkflowService, JsonRpcError> {
        self.runtime
            .as_ref()
            .map(RuntimeServices::workflows)
            .ok_or_else(|| JsonRpcError::server_error("workflow service is unavailable"))
    }
}

fn workflow_goal_state(state: crate::workflows::GoalState) -> WorkflowGoalState {
    WorkflowGoalState {
        schema_version: state.schema_version,
        goal_id: state.goal_id,
        status: match state.status {
            crate::workflows::GoalStatus::Running => WorkflowGoalStatus::Running,
            crate::workflows::GoalStatus::Converged => WorkflowGoalStatus::Converged,
            crate::workflows::GoalStatus::Failed => WorkflowGoalStatus::Failed,
            crate::workflows::GoalStatus::UserPaused => WorkflowGoalStatus::UserPaused,
        },
        current_milestone: state.current_milestone,
        total_milestones: state.total_milestones,
        loop_count: state.loop_count,
        max_loops: state.max_loops,
        milestone_step_budget: state.milestone_step_budget,
        milestone_timeout_secs: state.milestone_timeout_secs,
        verifier_model: state.verifier_model,
        last_verifier_score: state.last_verifier_score,
        updated_at_ms: state.updated_at_ms,
    }
}

fn thread_goal_state(
    thread_id: mini_agent_protocol::ThreadId,
    state: crate::workflows::GoalState,
) -> ThreadGoal {
    ThreadGoal {
        thread_id,
        objective: state.objective,
        status: match state.status {
            crate::workflows::GoalStatus::Running => ThreadGoalStatus::Active,
            crate::workflows::GoalStatus::Converged => ThreadGoalStatus::Complete,
            crate::workflows::GoalStatus::Failed => ThreadGoalStatus::Blocked,
            crate::workflows::GoalStatus::UserPaused => ThreadGoalStatus::Paused,
        },
        token_budget: state.token_budget,
        tokens_used: 0,
        time_used_seconds: if state.created_at_ms == 0 {
            0
        } else {
            state.updated_at_ms.saturating_sub(state.created_at_ms) / 1000
        } as i64,
        created_at: (state.created_at_ms / 1000) as i64,
        updated_at: (state.updated_at_ms / 1000) as i64,
    }
}

fn collaboration_mode(active: bool) -> CollaborationMode {
    CollaborationMode {
        mode: if active {
            CollaborationModeKind::Plan
        } else {
            CollaborationModeKind::Default
        },
    }
}

fn workflow_goal_response(
    id: Option<Value>,
    response: ActionResponse<crate::workflows::GoalState>,
) -> Option<JsonRpcResponse> {
    let state = workflow_goal_state(response.value.clone());
    response_action_with(id, response, state)
}

fn host_verifier_verdict(verdict: WorkflowVerifierVerdict) -> crate::workflows::VerifierVerdict {
    crate::workflows::VerifierVerdict {
        outcome: match verdict.outcome {
            WorkflowVerdictOutcome::Approved => crate::workflows::VerdictOutcome::Approved,
            WorkflowVerdictOutcome::Rejected => crate::workflows::VerdictOutcome::Rejected,
            WorkflowVerdictOutcome::NeedsClarification => {
                crate::workflows::VerdictOutcome::NeedsClarification
            }
            WorkflowVerdictOutcome::Invalid => crate::workflows::VerdictOutcome::Invalid,
        },
        score: verdict.score,
        summary: verdict.summary,
    }
}
