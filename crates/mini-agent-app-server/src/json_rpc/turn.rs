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
            .submit_start_action(params.thread_id, TurnStart::new(params.input), None)
            .await
        {
            Ok(response) => response_action(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
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
        match self.server.turn_read_action(params.turn_id.clone()).await {
            Ok(response) => match response.value.clone() {
                Some(result) => response_action_with(
                    request.id,
                    response,
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
                        items: result.outcome.as_ref().map_or_else(Vec::new, |outcome| {
                            mini_agent_app_server_protocol::ThreadItem::from_messages(
                                &outcome.messages,
                            )
                        }),
                        error: result.error,
                    },
                ),
                None => response_error(
                    request.id,
                    map_server_error(AppServerError::TurnNotFound(params.turn_id)),
                ),
            },
            Err(error) => response_error(request.id, map_action_error(error)),
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
            .submit_start_action(
                params.thread_id,
                TurnStart::new(TurnInput::new(TurnInputMode::Steer, params.text)),
                Some(params.turn_id),
            )
            .await
        {
            Ok(response) => response_action(request.id, response),
            Err(error) => response_error(request.id, map_action_error(error)),
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
            .turn_cancel_action(params.thread_id, TurnCancel::new(params.turn_id))
            .await
        {
            Ok(response) => response_action_with(
                request.id,
                response,
                serde_json::json!({ "accepted": true }),
            ),
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }
}
