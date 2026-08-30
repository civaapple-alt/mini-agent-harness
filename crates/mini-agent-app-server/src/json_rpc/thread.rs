use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
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
            return match self.server.thread_start(thread_id.clone()).await {
                Ok(thread_id) => response_value(request.id, ThreadStartResult { thread_id }),
                Err(error) => response_error(request.id, map_server_error(error)),
            };
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
        match self
            .server
            .thread_fork(params.source_thread_id, params.new_thread_id)
            .await
        {
            Ok(thread_id) => response_value(request.id, ThreadForkResult { thread_id }),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
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
        match self
            .server
            .thread_resume(params.thread_id, core_checkpoint)
            .await
        {
            Ok(thread_id) => response_value(request.id, ThreadResumeResult { thread_id }),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
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
        match self.server.thread_read_for(params.thread_id).await {
            Ok(checkpoint) => response_value(
                request.id,
                ThreadReadResult {
                    thread_id: checkpoint.thread_id,
                    status: checkpoint.status,
                    messages: checkpoint.session.messages().to_vec(),
                    context_revision: checkpoint.session.context_revision(),
                    next_turn_number: checkpoint.next_turn_number,
                    last_turn_id: checkpoint.last_turn_id,
                    next_event_sequence: checkpoint.next_event_sequence,
                },
            ),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
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
        match self.server.thread_close_for(params.thread_id).await {
            Ok(()) => response_value(request.id, serde_json::json!({ "closed": true })),
            Err(error) => response_error(request.id, map_server_error(error)),
        }
    }
}
