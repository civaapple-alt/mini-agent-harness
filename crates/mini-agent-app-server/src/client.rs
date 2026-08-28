//! In-process client for the same versioned service boundary used by JSON-RPC.

use crate::AppServerConnection;
use mini_agent_app_server_protocol::ClientCapabilities;
use mini_agent_app_server_protocol::InitializeParams;
use mini_agent_app_server_protocol::InitializeResult;
use mini_agent_app_server_protocol::JsonRpcError;
use mini_agent_app_server_protocol::JsonRpcRequest;
use mini_agent_app_server_protocol::METHOD_INITIALIZE;
use mini_agent_app_server_protocol::METHOD_THREAD_CLOSE;
use mini_agent_app_server_protocol::METHOD_THREAD_FORK;
use mini_agent_app_server_protocol::METHOD_THREAD_LIST;
use mini_agent_app_server_protocol::METHOD_THREAD_READ;
use mini_agent_app_server_protocol::METHOD_THREAD_RESUME;
use mini_agent_app_server_protocol::METHOD_THREAD_START;
use mini_agent_app_server_protocol::METHOD_TURN_INTERRUPT;
use mini_agent_app_server_protocol::METHOD_TURN_READ;
use mini_agent_app_server_protocol::METHOD_TURN_START;
use mini_agent_app_server_protocol::METHOD_TURN_STEER;
use mini_agent_app_server_protocol::ThreadCloseParams;
use mini_agent_app_server_protocol::ThreadForkParams;
use mini_agent_app_server_protocol::ThreadForkResult;
use mini_agent_app_server_protocol::ThreadListParams;
use mini_agent_app_server_protocol::ThreadListResult;
use mini_agent_app_server_protocol::ThreadReadParams;
use mini_agent_app_server_protocol::ThreadReadResult;
use mini_agent_app_server_protocol::ThreadResumeParams;
use mini_agent_app_server_protocol::ThreadResumeResult;
use mini_agent_app_server_protocol::ThreadStartParams;
use mini_agent_app_server_protocol::ThreadStartResult;
use mini_agent_app_server_protocol::TurnEventNotification;
use mini_agent_app_server_protocol::TurnInterruptParams;
use mini_agent_app_server_protocol::TurnReadParams;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_app_server_protocol::TurnStartParams;
use mini_agent_app_server_protocol::TurnSteerParams;
use mini_agent_core::EventEnvelope;
use mini_agent_core::Model;
use mini_agent_core::ThreadId;
use mini_agent_core::TurnId;
use mini_agent_core::TurnSubmission;
use serde::de::DeserializeOwned;

/// A local client that exercises the app-server protocol without a transport.
///
/// This gives the CLI and embedded callers a migration path to the external
/// service boundary while keeping subprocess framing out of core execution.
pub struct LocalAppServerClient<M> {
    connection: AppServerConnection<M>,
    next_id: u64,
}

impl<M> LocalAppServerClient<M>
where
    M: Model + Send + 'static,
{
    pub fn new(connection: AppServerConnection<M>) -> Self {
        Self {
            connection,
            next_id: 1,
        }
    }

    pub async fn initialize(
        &mut self,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Result<InitializeResult, JsonRpcError> {
        self.initialize_with_profile(client_name, client_version, None)
            .await
    }

    pub async fn initialize_with_profile(
        &mut self,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
        profile: Option<String>,
    ) -> Result<InitializeResult, JsonRpcError> {
        self.call(
            METHOD_INITIALIZE,
            InitializeParams {
                protocol_version: mini_agent_app_server_protocol::PROTOCOL_VERSION,
                client_name: client_name.into(),
                client_version: client_version.into(),
                capabilities: ClientCapabilities::default(),
                profile,
            },
        )
        .await
    }

    pub async fn start_thread(&mut self) -> Result<ThreadStartResult, JsonRpcError> {
        self.call(METHOD_THREAD_START, ThreadStartParams { thread_id: None })
            .await
    }

    pub async fn list_threads(&mut self) -> Result<ThreadListResult, JsonRpcError> {
        self.call(METHOD_THREAD_LIST, ThreadListParams::default())
            .await
    }

    pub async fn read_thread(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<ThreadReadResult, JsonRpcError> {
        self.call(METHOD_THREAD_READ, ThreadReadParams { thread_id })
            .await
    }

    pub async fn close_thread(&mut self, thread_id: ThreadId) -> Result<bool, JsonRpcError> {
        let result: serde_json::Value = self
            .call(METHOD_THREAD_CLOSE, ThreadCloseParams { thread_id })
            .await?;
        Ok(result["closed"].as_bool().unwrap_or(false))
    }

    pub async fn fork_thread(
        &mut self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) -> Result<ThreadForkResult, JsonRpcError> {
        self.call(
            METHOD_THREAD_FORK,
            ThreadForkParams {
                source_thread_id,
                new_thread_id,
            },
        )
        .await
    }

    pub async fn resume_thread(
        &mut self,
        thread_id: ThreadId,
        checkpoint: ThreadReadResult,
    ) -> Result<ThreadResumeResult, JsonRpcError> {
        self.call(
            METHOD_THREAD_RESUME,
            ThreadResumeParams {
                thread_id,
                checkpoint,
            },
        )
        .await
    }

    pub async fn start_turn(
        &mut self,
        thread_id: ThreadId,
        input: mini_agent_core::TurnInput,
    ) -> Result<TurnSubmission, JsonRpcError> {
        self.call(METHOD_TURN_START, TurnStartParams { thread_id, input })
            .await
    }

    pub async fn steer(
        &mut self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: impl Into<String>,
    ) -> Result<TurnSubmission, JsonRpcError> {
        self.call(
            METHOD_TURN_STEER,
            TurnSteerParams {
                thread_id,
                turn_id,
                text: text.into(),
            },
        )
        .await
    }

    pub async fn interrupt(
        &mut self,
        thread_id: ThreadId,
        turn_id: TurnId,
    ) -> Result<bool, JsonRpcError> {
        let result: serde_json::Value = self
            .call(
                METHOD_TURN_INTERRUPT,
                TurnInterruptParams { thread_id, turn_id },
            )
            .await?;
        Ok(result["accepted"].as_bool().unwrap_or(false))
    }

    pub async fn read_turn(&mut self, turn_id: TurnId) -> Result<TurnReadResult, JsonRpcError> {
        self.call(METHOD_TURN_READ, TurnReadParams { turn_id })
            .await
    }

    pub async fn next_event(&mut self) -> Result<EventEnvelope, JsonRpcError> {
        let notification = self
            .connection
            .next_notification()
            .await
            .map_err(|error| JsonRpcError::server_error(error.to_string()))?;
        let params = notification
            .params
            .ok_or_else(|| JsonRpcError::invalid_params("turn/event params are missing"))?;
        let event: TurnEventNotification = serde_json::from_value(params)
            .map_err(|error| JsonRpcError::invalid_params(error.to_string()))?;
        Ok(EventEnvelope::new(
            event.thread_id,
            event.turn_id,
            event.sequence,
            event.event,
        ))
    }

    async fn call<P, T>(&mut self, method: &str, params: P) -> Result<T, JsonRpcError>
    where
        P: serde::Serialize,
        T: DeserializeOwned,
    {
        let id = self.next_id;
        self.next_id += 1;
        let request = JsonRpcRequest::request(
            id,
            method,
            serde_json::to_value(params)
                .map_err(|error| JsonRpcError::invalid_params(error.to_string()))?,
        );
        let response = self
            .connection
            .handle_request(request)
            .await
            .ok_or_else(|| JsonRpcError::server_error("service returned no response"))?;
        if let Some(error) = response.error {
            return Err(error);
        }
        serde_json::from_value(response.result.unwrap_or(serde_json::Value::Null))
            .map_err(|error| JsonRpcError::server_error(error.to_string()))
    }
}
