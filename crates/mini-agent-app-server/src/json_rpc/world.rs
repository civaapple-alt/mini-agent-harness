use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_session_info(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.session_info_action().await {
            Ok(response) => {
                let value = response.value.as_ref().map(|info| SessionInfoResult {
                    session_id: info.session_id.clone(),
                    thread_id: info.thread_id.clone(),
                    path: info.path.clone(),
                    resumed: info.resumed,
                });
                response_action_with(request.id, response, value)
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_world_state(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match world_state_result_action(management).await {
            Ok(response) => response_action(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_world_refresh(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.refresh_world_action().await {
            Ok(response) => {
                let changed = response.value;
                match world_state_value(management).await {
                    Ok(state) => response_action_with(
                        request.id,
                        response,
                        WorldRefreshResult { changed, state },
                    ),
                    Err(error) => response_error(request.id, workflow_error(error)),
                }
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_world_set_execution(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<WorldSetExecutionParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let approval = match params.approval.as_str() {
            "interactive" => ApprovalMode::Interactive,
            "automatic" => ApprovalMode::Automatic,
            _ => {
                return response_error(
                    request.id,
                    JsonRpcError::invalid_params("approval must be interactive or automatic"),
                );
            }
        };
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management
            .set_execution_action(approval, params.copilot)
            .await
        {
            Ok(response) => {
                let changed = response.value;
                match world_state_value(management).await {
                    Ok(state) => response_action_with(
                        request.id,
                        response,
                        WorldSetExecutionResult { changed, state },
                    ),
                    Err(error) => response_error(request.id, workflow_error(error)),
                }
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_mcp_status(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.mcp_status_action().await {
            Ok(response) => {
                let status = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    McpStatusResult {
                        enabled_servers: status.enabled_servers,
                        inactive_servers: status.inactive_servers,
                        tool_count: status.tool_count,
                        retry_available: status.retry_available,
                    },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_mcp_retry(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.retry_mcp_action().await {
            Ok(response) => {
                let result = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    ProtocolMcpRetryResult {
                        enabled_servers: result.enabled_servers,
                        inactive_servers: result.inactive_servers,
                        diagnostics: result.diagnostics,
                        tool_count: result.tool_count,
                    },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) fn management_service(&self) -> Result<&RuntimeManagementService<M>, JsonRpcError> {
        self.runtime
            .as_ref()
            .map(RuntimeServices::management)
            .ok_or_else(|| JsonRpcError::server_error("runtime management is unavailable"))
    }
}

async fn world_state_value<M: Model + Send + 'static>(
    management: &RuntimeManagementService<M>,
) -> Result<WorldStateResult, String> {
    let world = management.world().await?;
    Ok(world_state_result(&world))
}

async fn world_state_result_action<M: Model + Send + 'static>(
    management: &RuntimeManagementService<M>,
) -> Result<ActionResponse<WorldStateResult>, ActionFailure> {
    let response = management.world_action().await?;
    let value = world_state_result(&response.value);
    Ok(response.map_value(value))
}

fn world_state_result(world: &mini_agent_host::WorldState) -> WorldStateResult {
    WorldStateResult {
        workspace: world.workspace().display().to_string(),
        status: world.status_json(),
        lines: world.status_lines(),
        context: world.model_context().unwrap_or_default(),
    }
}
