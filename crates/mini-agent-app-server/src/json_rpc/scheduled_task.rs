use super::*;
use mini_agent_app_server_protocol::{
    ScheduledTaskListParams, ScheduledTaskListResult, ScheduledTaskParams,
};

impl<M> AppServerConnection<M>
where
    M: Model + Send + 'static,
{
    pub(super) async fn handle_scheduled_task_list(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ScheduledTaskListParams>() {
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
            management.scheduled_task_list_action(),
            |tasks| ScheduledTaskListResult {
                data: tasks.clone(),
            },
        )
        .await
    }

    pub(super) async fn handle_scheduled_task_read(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_scheduled_task_action(request, ScheduledTaskAction::Read)
            .await
    }

    pub(super) async fn handle_scheduled_task_cancel(
        &self,
        request: JsonRpcRequest,
    ) -> Option<JsonRpcResponse> {
        self.handle_scheduled_task_action(request, ScheduledTaskAction::Cancel)
            .await
    }

    async fn handle_scheduled_task_action(
        &self,
        request: JsonRpcRequest,
        action: ScheduledTaskAction,
    ) -> Option<JsonRpcResponse> {
        let params = match request.decode_params::<ScheduledTaskParams>() {
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
            ScheduledTaskAction::Read => {
                action_response(
                    request.id,
                    management.scheduled_task_read_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
            ScheduledTaskAction::Cancel => {
                action_response(
                    request.id,
                    management.scheduled_task_cancel_action(params.task_id),
                    Clone::clone,
                )
                .await
            }
        }
    }
}

enum ScheduledTaskAction {
    Read,
    Cancel,
}
