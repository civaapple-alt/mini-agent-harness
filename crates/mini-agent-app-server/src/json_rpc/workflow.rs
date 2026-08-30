use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_workflow_state(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        let (plan_active, goal) = match workflows.state().await {
            Ok(state) => state,
            Err(error) => {
                return response_error(request.id, workflow_error(error.to_string()));
            }
        };
        response_value(
            request.id,
            WorkflowState {
                plan_active,
                goal: goal.map(workflow_goal_state),
            },
        )
    }

    pub(super) async fn handle_workflow_plan_set(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WorkflowPlanSetParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let workflows = match self.workflow_service() {
            Ok(workflows) => workflows,
            Err(error) => return response_error(request.id, error),
        };
        let result = if params.active {
            workflows.enable_plan_mode(params.prompt.as_deref()).await
        } else {
            workflows.disable_plan_mode().await
        };
        if let Err(error) = result {
            return response_error(request.id, workflow_error(error.to_string()));
        }
        self.handle_workflow_state(request).await
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
        match workflows.init_goal(&params.objective).await {
            Ok(state) => response_value(request.id, workflow_goal_state(state)),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
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
        if let Err(error) = workflows.pause_goal().await {
            return response_error(request.id, workflow_error(error.to_string()));
        }
        match workflows.load_goal_state().await {
            Ok(Some(state)) => response_value(request.id, workflow_goal_state(state)),
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
        match workflows.fail_goal().await {
            Ok(state) => response_value(request.id, workflow_goal_state(state)),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
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
        match workflows.verification_criteria().await {
            Ok(criteria) => response_value(
                request.id,
                mini_agent_app_server_protocol::WorkflowGoalCriteriaResult { criteria },
            ),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
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
        match workflows.advance_goal(verdict).await {
            Ok(state) => response_value(request.id, workflow_goal_state(state)),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
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
            .record_verifier_verdict(params.checkpoint_seq, &params.output)
            .await
        {
            Ok(()) => response_value(request.id, serde_json::json!({"recorded": true})),
            Err(error) => response_error(request.id, workflow_error(error.to_string())),
        }
    }

    fn workflow_service(&self) -> Result<&WorkflowService, JsonRpcError> {
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
