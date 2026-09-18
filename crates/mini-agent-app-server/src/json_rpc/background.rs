use super::*;

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_background_task_list(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<BackgroundTaskListParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        action_response(
            request.id,
            management.background_task_list_action(),
            |tasks| BackgroundTaskListResult {
                data: tasks.clone(),
            },
        )
        .await
    }

    pub(super) async fn handle_background_task_read(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_background_task_action(request, BackgroundTaskAction::Read)
            .await
    }

    pub(super) async fn handle_background_task_logs(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_background_task_action(request, BackgroundTaskAction::Logs)
            .await
    }

    pub(super) async fn handle_background_task_stop(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_background_task_action(request, BackgroundTaskAction::Stop)
            .await
    }

    pub(super) async fn handle_background_task_restart(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_background_task_action(request, BackgroundTaskAction::Restart)
            .await
    }

    async fn handle_background_task_action(
        &self,
        request: JsonRpcRequest,
        action: BackgroundTaskAction,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<BackgroundTaskParams>() {
            Ok(params) => params,
            Err(error) => return response_error(request.id, error),
        };
        if let Err(error) = self.check_runtime_thread(&params.thread_id).await {
            return response_error(request.id, error);
        }
        let management = match self.management_service() {
            Ok(management) => management,
            Err(error) => return response_error(request.id, error),
        };
        match action {
            BackgroundTaskAction::Read => {
                action_response(
                    request.id,
                    management.background_task_read_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
            BackgroundTaskAction::Logs => {
                action_response(
                    request.id,
                    management.background_task_logs_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
            BackgroundTaskAction::Stop => {
                action_response(
                    request.id,
                    management.background_task_stop_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
            BackgroundTaskAction::Restart => {
                action_response(
                    request.id,
                    management.background_task_restart_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
        }
    }
}

enum BackgroundTaskAction {
    Read,
    Logs,
    Stop,
    Restart,
}
