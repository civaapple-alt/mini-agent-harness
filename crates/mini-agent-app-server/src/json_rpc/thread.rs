use super::*;
use mini_agent_host::BuiltinToolSelection;

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
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let goals = match self.thread_goal_request_processor() {
            Ok(goals) => goals,
            Err(error) => return response_error(request.id, error),
        };
        action_response(
            request.id,
            goals.set_thread_goal_action(params.objective, params.status, params.token_budget),
            |state| ThreadGoalSetResponse {
                goal: crate::goal_runtime::project_goal(params.thread_id.clone(), state.clone()),
            },
        )
        .await
    }

    pub(super) async fn handle_thread_goal_get(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadGoalGetParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let goals = match self.thread_goal_request_processor() {
            Ok(goals) => goals,
            Err(error) => return response_error(request.id, error),
        };
        action_response(request.id, goals.get_thread_goal_action(), |state| {
            ThreadGoalGetResponse {
                goal: state.clone().map(|state| {
                    crate::goal_runtime::project_goal(params.thread_id.clone(), state)
                }),
            }
        })
        .await
    }

    pub(super) async fn handle_thread_goal_clear(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadGoalClearParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let goals = match self.thread_goal_request_processor() {
            Ok(goals) => goals,
            Err(error) => return response_error(request.id, error),
        };
        action_response(request.id, goals.clear_thread_goal_action(), |cleared| {
            ThreadGoalClearResponse { cleared: *cleared }
        })
        .await
    }

    pub(super) async fn handle_thread_settings_update(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadSettingsUpdateParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let settings = match self.thread_settings_service() {
            Ok(settings) => settings,
            Err(error) => return response_error(request.id, error),
        };
        let builtin_tools = match params.builtin_tools {
            Some(names) => match BuiltinToolSelection::from_names(names) {
                Ok(selection) => Some(selection),
                Err(error) => {
                    return response_error(request.id, JsonRpcError::invalid_params(error));
                }
            },
            None => None,
        };
        let active = matches!(params.collaboration_mode.mode, CollaborationModeKind::Plan);
        action_response(
            request.id,
            settings.update_action(active, builtin_tools),
            |tools| ThreadSettingsUpdateResult {
                collaboration_mode: params.collaboration_mode,
                builtin_tools: tools.clone(),
            },
        )
        .await
    }

    pub(super) async fn handle_thread_start(
        &mut self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadStartParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let thread_id = params.thread_id.unwrap_or(self.thread_id().await);
        if !self.server.has_thread(&thread_id) {
            return action_response(
                request.id,
                self.server.thread_start_action(thread_id.clone()),
                move |_| ThreadStartResult { thread_id },
            )
            .await;
        }
        response_value(request.id, ThreadStartResult { thread_id })
    }

    pub(super) async fn handle_thread_list(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.params {
            Some(params) => match serde_json::from_value::<ThreadListParams>(params) {
                Ok(params) => params,
                Err(error) => {
                    return response_error(
                        request.id,
                        JsonRpcError::invalid_params(error.to_string()),
                    );
                }
            },
            None => ThreadListParams::default(),
        };
        let start = params
            .cursor
            .as_deref()
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = params
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(usize::MAX);
        let ids = self.server.thread_ids();
        let data = ids
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_cursor =
            (start + data.len() < ids.len()).then(|| (start + data.len()).to_string());
        response_value(request.id, ThreadListResult { data, next_cursor })
    }

    pub(super) async fn handle_thread_fork(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadForkParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        action_response(
            request.id,
            self.server
                .thread_fork_action(params.source_thread_id, params.new_thread_id),
            |thread_id| ThreadForkResult {
                thread_id: thread_id.clone(),
            },
        )
        .await
    }

    pub(super) async fn handle_thread_resume(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadResumeParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        let checkpoint = params.checkpoint;
        let core_checkpoint = mini_agent_core::ThreadCheckpoint {
            thread_id: params.thread_id.clone(),
            session: SessionState::from_messages(checkpoint.messages)
                .with_context_revision(checkpoint.context_revision),
            status: checkpoint.status,
            next_turn_number: checkpoint.next_turn_number,
            last_turn_id: checkpoint.last_turn_id,
            next_event_sequence: checkpoint.next_event_sequence,
        };
        action_response(
            request.id,
            self.server
                .thread_resume_action(params.thread_id, core_checkpoint),
            |thread_id| ThreadResumeResult {
                thread_id: thread_id.clone(),
            },
        )
        .await
    }

    pub(super) async fn handle_thread_read(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadReadParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        match self.server.thread_read_action(params.thread_id).await {
            Ok(response) => {
                let checkpoint = response.value.clone();
                response_action_with(
                    request.id,
                    response,
                    ThreadReadResult {
                        thread_id: checkpoint.thread_id,
                        status: checkpoint.status,
                        messages: checkpoint.session.messages().to_vec(),
                        context_revision: checkpoint.session.context_revision(),
                        next_turn_number: checkpoint.next_turn_number,
                        last_turn_id: checkpoint.last_turn_id,
                        next_event_sequence: checkpoint.next_event_sequence,
                    },
                )
            }
            Err(error) => response_error(request.id, map_action_error(error)),
        }
    }

    pub(super) async fn handle_thread_items_list(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadItemsListParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        action_response(
            request.id,
            self.server.thread_items_action(params),
            Clone::clone,
        )
        .await
    }

    pub(super) async fn handle_thread_close(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ThreadCloseParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_thread(&params.thread_id) {
            return response_error(request.id, error);
        }
        action_response(
            request.id,
            self.server.thread_close_action(params.thread_id),
            |_| serde_json::json!({ "closed": true }),
        )
        .await
    }
}
