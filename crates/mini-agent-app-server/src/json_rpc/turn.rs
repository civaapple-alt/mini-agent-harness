use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_turn_events(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<TurnEventsParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        let limit = params.limit.unwrap_or(128).clamp(1, 128) as usize;
        match self
            .server
            .replay_events(&params.thread_id, params.after_sequence, limit)
        {
            Ok(snapshot) => {
                let next_cursor = snapshot.events.last().map(|event| event.sequence);
                response_value(
                    request.id,
                    TurnEventsResult {
                        data: snapshot
                            .events
                            .into_iter()
                            .map(TurnEventNotification::from)
                            .collect(),
                        next_cursor,
                        oldest_sequence: snapshot.oldest_sequence,
                        has_gap: snapshot.has_gap,
                    },
                )
            }
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }

    pub(super) async fn handle_runtime_status(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<RuntimeStatusParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        response_value(request.id, self.server.runtime_status())
    }

    pub(super) async fn handle_turn_start(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let mut params = match request.decode_params::<TurnStartParams>() {
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
        if let Ok(settings) = self.thread_settings_service() {
            let model_settings = settings.model_settings();
            params.input.model_selection = model_settings.selection;
            params.input.reasoning_effort = model_settings.reasoning_effort;
        }
        let mut turn = TurnStart::new(params.input);
        turn.operation_id = params.operation_id;
        turn.operation_attempt = params.operation_attempt;
        turn.operation_attempt_kind = params.operation_attempt_kind;
        turn.operation_group_id = params.operation_group_id;
        turn.execution_mode = params.execution_mode;
        turn.group_sequence = params.group_sequence;
        action_response(
            request.id,
            self.server.submit_start_action_with_source(
                params.thread_id,
                turn,
                None,
                params.turn_source,
            ),
            Clone::clone,
        )
        .await
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
        if params
            .request_id
            .as_deref()
            .is_some_and(|id| id.is_empty() || id.len() > 192)
        {
            return response_error(
                request.id,
                JsonRpcError::invalid_params("requestId must be bounded and non-empty"),
            );
        }
        let Some(request_id) = params.request_id else {
            return action_response(
                request.id,
                self.server.submit_start_action(
                    params.thread_id,
                    TurnStart::new(TurnInput::new(TurnInputMode::Steer, params.text)),
                    Some(params.turn_id),
                ),
                Clone::clone,
            )
            .await;
        };

        // Serialize both steer acceptance and follow-up allocation through
        // this shared request-id namespace. A replay after the child changes
        // state must retain the route selected by the first request.
        let _request_guard = self.server.lock_child_steer_request().await;
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        let reservation = match management
            .child_steer_request_action(
                params.thread_id.clone(),
                request_id.clone(),
                params.turn_id.0.clone(),
                mini_agent_capabilities::ChildSteerRequestStep::Reserve,
                None,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => return response_error(request.id, map_action_error(error)),
        };
        if let Some(control_request) = reservation
            .value
            .as_ref()
            .filter(|result| result.duplicate || result.status == "not_submitted")
            .cloned()
        {
            let result = steer_result_from_control_request(&control_request);
            return response_action_with(request.id, reservation, result);
        }

        let submission = self
            .server
            .submit_start_action(
                params.thread_id.clone(),
                TurnStart::new(TurnInput::new(TurnInputMode::Steer, params.text)),
                Some(params.turn_id.clone()),
            )
            .await;
        let response = match submission {
            Ok(response) => response,
            Err(error) => return response_error(request.id, map_action_error(error)),
        };
        let result = steer_result_from_submission(&response.value);
        let (step, accepted_status) = match &response.value {
            mini_agent_protocol::TurnSubmission::NotSubmitted { .. } => (
                mini_agent_capabilities::ChildSteerRequestStep::NotAccepted,
                None,
            ),
            mini_agent_protocol::TurnSubmission::Queued => (
                mini_agent_capabilities::ChildSteerRequestStep::Accept,
                Some("queued"),
            ),
            mini_agent_protocol::TurnSubmission::Started { .. } => (
                mini_agent_capabilities::ChildSteerRequestStep::Accept,
                Some("started"),
            ),
            mini_agent_protocol::TurnSubmission::Steered { .. } => (
                mini_agent_capabilities::ChildSteerRequestStep::Accept,
                Some("steered"),
            ),
        };
        match management
            .child_steer_request_action(
                params.thread_id,
                request_id,
                params.turn_id.0.clone(),
                step,
                accepted_status.map(str::to_string),
            )
            .await
        {
            Ok(recorded) => match recorded.value.clone() {
                Some(control_request) => {
                    let result = steer_result_from_control_request(&control_request);
                    response_action_with(request.id, recorded, result)
                }
                None => response_action_with(request.id, response, result),
            },
            Err(error) => {
                eprintln!(
                    "child steer outcome could not be persisted: {}",
                    error.error
                );
                response_action_with(
                    request.id,
                    response,
                    pending_steer_result(
                        result.turn_id,
                        "The request outcome is unresolved; it may or may not have been submitted. Do not automatically resend this requestId.",
                    ),
                )
            }
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
        action_response(
            request.id,
            self.server
                .turn_cancel_action(params.thread_id, TurnCancel::new(params.turn_id)),
            |_| serde_json::json!({ "accepted": true }),
        )
        .await
    }
}

fn steer_result_from_control_request(
    request: &mini_agent_capabilities::ChildTaskMutationResult,
) -> mini_agent_app_server_protocol::TurnSteerResult {
    mini_agent_app_server_protocol::TurnSteerResult {
        status: request.status.clone(),
        turn_id: request
            .turn_id
            .clone()
            .map(mini_agent_protocol::TurnId::new),
        duplicate: request.duplicate,
        request_action: request.request_action.map(|action| match action {
            mini_agent_capabilities::ChildControlRequestAction::Steer => {
                mini_agent_app_server_protocol::TurnSteerAction::Steer
            }
            mini_agent_capabilities::ChildControlRequestAction::QueueFollowUp => {
                mini_agent_app_server_protocol::TurnSteerAction::QueueFollowUp
            }
        }),
        operation_id: None,
        attempt: Some(request.attempt),
        attempt_kind: request.attempt_kind,
        reason: (request.status == "pending").then(|| {
            "The request outcome is unresolved; it may or may not have been submitted. Do not automatically resend this requestId.".to_string()
        }),
    }
}

fn pending_steer_result(
    turn_id: Option<mini_agent_protocol::TurnId>,
    reason: &str,
) -> mini_agent_app_server_protocol::TurnSteerResult {
    mini_agent_app_server_protocol::TurnSteerResult {
        status: "pending".to_string(),
        turn_id,
        duplicate: false,
        request_action: Some(mini_agent_app_server_protocol::TurnSteerAction::Steer),
        operation_id: None,
        attempt: None,
        attempt_kind: None,
        reason: Some(reason.to_string()),
    }
}

fn steer_result_from_submission(
    submission: &mini_agent_protocol::TurnSubmission,
) -> mini_agent_app_server_protocol::TurnSteerResult {
    use mini_agent_protocol::TurnSubmission;
    let (status, turn_id, reason) = match submission {
        TurnSubmission::Started { turn_id } => ("started", Some(turn_id.clone()), None),
        TurnSubmission::Steered { turn_id } => ("steered", Some(turn_id.clone()), None),
        TurnSubmission::Queued => ("queued", None, None),
        TurnSubmission::NotSubmitted { reason } => ("not_submitted", None, Some(reason.clone())),
    };
    mini_agent_app_server_protocol::TurnSteerResult {
        status: status.to_string(),
        turn_id,
        duplicate: false,
        request_action: None,
        operation_id: None,
        attempt: None,
        attempt_kind: None,
        reason,
    }
}
