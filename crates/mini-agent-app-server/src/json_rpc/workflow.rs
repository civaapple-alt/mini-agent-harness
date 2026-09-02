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
        let thread_id = self.thread_id().await;
        match workflows.state_action().await {
            Ok(response) => {
                let (plan_active, goal) = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    WorkflowState {
                        collaboration_mode: collaboration_mode(plan_active),
                        goal: goal.map(|state| crate::goal_runtime::project_goal(thread_id, state)),
                    },
                )
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

fn collaboration_mode(active: bool) -> CollaborationMode {
    CollaborationMode {
        mode: if active {
            CollaborationModeKind::Plan
        } else {
            CollaborationModeKind::Default
        },
    }
}
