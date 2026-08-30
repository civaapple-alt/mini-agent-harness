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
        match management.session_info() {
            Some(info) => response_value(
                request.id,
                SessionInfoResult {
                    session_id: info.session_id,
                    thread_id: info.thread_id,
                    path: info.path,
                    resumed: info.resumed,
                },
            ),
            None => response_value(request.id, serde_json::Value::Null),
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
        response_value(request.id, world_state_result(management))
    }

    pub(super) async fn handle_world_refresh(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.refresh_world().await {
            Ok(changed) => response_value(
                request.id,
                WorldRefreshResult {
                    changed,
                    state: world_state_result(management),
                },
            ),
            Err(error) => response_error(request.id, workflow_error(error)),
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
        match management.set_execution(approval, params.copilot).await {
            Ok(changed) => response_value(
                request.id,
                WorldSetExecutionResult {
                    changed,
                    state: world_state_result(management),
                },
            ),
            Err(error) => response_error(request.id, workflow_error(error)),
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
        let (enabled_servers, inactive_servers, tool_count) = management.mcp_status();
        response_value(
            request.id,
            McpStatusResult {
                enabled_servers,
                inactive_servers,
                tool_count,
                retry_available: !management.retry_mcp_servers().is_empty(),
            },
        )
    }

    pub(super) async fn handle_mcp_retry(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match management.retry_mcp().await {
            Ok(result) => response_value(
                request.id,
                ProtocolMcpRetryResult {
                    enabled_servers: result.enabled_servers,
                    inactive_servers: result.inactive_servers,
                    diagnostics: result.diagnostics,
                    tool_count: result.tool_count,
                },
            ),
            Err(error) => response_error(request.id, workflow_error(error)),
        }
    }

    pub(super) fn management_service(&self) -> Result<&RuntimeManagementService<M>, JsonRpcError> {
        self.runtime
            .as_ref()
            .map(RuntimeServices::management)
            .ok_or_else(|| JsonRpcError::server_error("runtime management is unavailable"))
    }
}

fn world_state_result<M: Model + Send + 'static>(
    management: &RuntimeManagementService<M>,
) -> WorldStateResult {
    let world = management.world();
    WorldStateResult {
        workspace: world.workspace().display().to_string(),
        status: world.status_json(),
        lines: world.status_lines(),
        context: world.model_context().unwrap_or_default(),
    }
}
