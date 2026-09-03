use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_workflow_state(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let runtime = match self.runtime.as_ref() {
            Some(runtime) => runtime,
            None => {
                return response_error(
                    request.id,
                    JsonRpcError::server_error("runtime state service is unavailable"),
                );
            }
        };
        let thread_id = self.thread_id().await;
        match runtime.runtime_state_action().await {
            Ok(response) => {
                let (plan_active, goal, builtin_tools) = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    WorkflowState {
                        collaboration_mode: collaboration_mode(plan_active),
                        builtin_tools,
                        goal: goal.map(|state| crate::goal_runtime::project_goal(thread_id, state)),
                    },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
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
