use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_turn_start(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnStartParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if !matches!(
            params.input.mode,
            TurnInputMode::Start | TurnInputMode::StartIfIdle
        ) {
            return response_error(
                request.id,
                JsonRpcError::invalid_params("turn/start requires start or start_if_idle"),
            );
        }
        match self
            .server
            .turn_start_for(params.thread_id, TurnStart::new(params.input))
            .await
        {
            Ok(submission) => response_value(request.id, submission),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    pub(super) async fn handle_turn_read(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnReadParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self.server.turn_read(params.turn_id.clone()).await {
            Ok(result) => response_value(
                request.id,
                mini_agent_app_server_protocol::TurnReadResult {
                    turn_id: result.id,
                    status: result.status,
                    stop_reason: result.outcome.as_ref().map(|outcome| outcome.stop_reason),
                    final_text: result
                        .outcome
                        .as_ref()
                        .map(|outcome| outcome.final_text.clone()),
                    steps: result.outcome.as_ref().map_or(0, |outcome| outcome.steps),
                    messages: result
                        .outcome
                        .as_ref()
                        .map_or_else(Vec::new, |outcome| outcome.messages.clone()),
                    error: result.error,
                },
            ),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    pub(super) async fn handle_turn_steer(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnSteerParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self
            .server
            .turn_steer_for(params.thread_id, params.turn_id, params.text)
            .await
        {
            Ok(submission) => response_value(request.id, submission),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    pub(super) async fn handle_turn_interrupt(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnInterruptParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        match self
            .server
            .turn_cancel_for(params.thread_id, TurnCancel::new(params.turn_id))
            .await
        {
            Ok(()) => response_value(request.id, serde_json::json!({ "accepted": true })),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }
}
