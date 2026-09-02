//! In-process client for the same versioned service boundary used by JSON-RPC.

use crate::AppServerConnection;
use crate::ThreadUpdate;
use crate::runtime::{RuntimeTurnBatch, RuntimeTurnResult};
use mini_agent_app_server_protocol::CapabilityProviderSelection;
use mini_agent_app_server_protocol::ClientCapabilities;
use mini_agent_app_server_protocol::CollaborationMode;
use mini_agent_app_server_protocol::CollaborationModeKind;
use mini_agent_app_server_protocol::InitializeParams;
use mini_agent_app_server_protocol::InitializeResult;
use mini_agent_app_server_protocol::JsonRpcError;
use mini_agent_app_server_protocol::JsonRpcRequest;
use mini_agent_app_server_protocol::METHOD_INITIALIZE;
use mini_agent_app_server_protocol::METHOD_MCP_RETRY;
use mini_agent_app_server_protocol::METHOD_MCP_STATUS;
use mini_agent_app_server_protocol::METHOD_SESSION_INFO;
use mini_agent_app_server_protocol::METHOD_THREAD_CLOSE;
use mini_agent_app_server_protocol::METHOD_THREAD_FORK;
use mini_agent_app_server_protocol::METHOD_THREAD_GOAL_CLEAR;
use mini_agent_app_server_protocol::METHOD_THREAD_GOAL_GET;
use mini_agent_app_server_protocol::METHOD_THREAD_GOAL_SET;
use mini_agent_app_server_protocol::METHOD_THREAD_LIST;
use mini_agent_app_server_protocol::METHOD_THREAD_READ;
use mini_agent_app_server_protocol::METHOD_THREAD_RESUME;
use mini_agent_app_server_protocol::METHOD_THREAD_SETTINGS_UPDATE;
use mini_agent_app_server_protocol::METHOD_THREAD_START;
use mini_agent_app_server_protocol::METHOD_TURN_EVENT;
use mini_agent_app_server_protocol::METHOD_TURN_INTERRUPT;
use mini_agent_app_server_protocol::METHOD_TURN_READ;
use mini_agent_app_server_protocol::METHOD_TURN_START;
use mini_agent_app_server_protocol::METHOD_TURN_STEER;
use mini_agent_app_server_protocol::METHOD_WORKFLOW_STATE;
use mini_agent_app_server_protocol::METHOD_WORLD_REFRESH;
use mini_agent_app_server_protocol::METHOD_WORLD_SET_EXECUTION;
use mini_agent_app_server_protocol::METHOD_WORLD_STATE;
use mini_agent_app_server_protocol::McpRetryResult;
use mini_agent_app_server_protocol::McpStatusResult;
use mini_agent_app_server_protocol::SessionInfoResult;
use mini_agent_app_server_protocol::ThreadCloseParams;
use mini_agent_app_server_protocol::ThreadForkParams;
use mini_agent_app_server_protocol::ThreadForkResult;
use mini_agent_app_server_protocol::ThreadGoalClearResponse;
use mini_agent_app_server_protocol::ThreadGoalGetResponse;
use mini_agent_app_server_protocol::ThreadGoalSetParams;
use mini_agent_app_server_protocol::ThreadGoalSetResponse;
use mini_agent_app_server_protocol::ThreadGoalStatus;
use mini_agent_app_server_protocol::ThreadListParams;
use mini_agent_app_server_protocol::ThreadListResult;
use mini_agent_app_server_protocol::ThreadReadParams;
use mini_agent_app_server_protocol::ThreadReadResult;
use mini_agent_app_server_protocol::ThreadResumeParams;
use mini_agent_app_server_protocol::ThreadResumeResult;
use mini_agent_app_server_protocol::ThreadSettingsUpdateParams;
use mini_agent_app_server_protocol::ThreadSettingsUpdateResult;
use mini_agent_app_server_protocol::ThreadStartParams;
use mini_agent_app_server_protocol::ThreadStartResult;
use mini_agent_app_server_protocol::TurnEventNotification;
use mini_agent_app_server_protocol::TurnInterruptParams;
use mini_agent_app_server_protocol::TurnReadParams;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_app_server_protocol::TurnStartParams;
use mini_agent_app_server_protocol::TurnSteerParams;
use mini_agent_app_server_protocol::WorkflowState;
use mini_agent_app_server_protocol::WorldRefreshResult;
use mini_agent_app_server_protocol::WorldSetExecutionParams;
use mini_agent_app_server_protocol::WorldSetExecutionResult;
use mini_agent_app_server_protocol::WorldStateResult;
use mini_agent_core::RunControl;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::EventSink;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnSubmission;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

/// A local client that exercises the app-server protocol without a transport.
///
/// This gives the CLI and embedded callers a migration path to the external
/// service boundary while keeping subprocess framing out of core execution.
pub struct LocalAppServerClient<M> {
    connection: AppServerConnection<M>,
    next_id: u64,
    control: Arc<RunControl>,
}

impl<M> LocalAppServerClient<M>
where
    M: Model + Send + 'static,
{
    pub fn new(connection: AppServerConnection<M>) -> Self {
        Self::with_control(connection, Arc::new(RunControl::new()))
    }

    pub fn with_control(connection: AppServerConnection<M>, control: Arc<RunControl>) -> Self {
        Self {
            connection,
            next_id: 1,
            control,
        }
    }

    pub async fn initialize(
        &mut self,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Result<InitializeResult, JsonRpcError> {
        self.initialize_with_profile_and_providers(client_name, client_version, None, None)
            .await
    }

    pub async fn initialize_with_profile_and_providers(
        &mut self,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
        profile: Option<String>,
        providers: Option<CapabilityProviderSelection>,
    ) -> Result<InitializeResult, JsonRpcError> {
        self.call(
            METHOD_INITIALIZE,
            InitializeParams {
                protocol_version: mini_agent_app_server_protocol::PROTOCOL_VERSION,
                client_name: client_name.into(),
                client_version: client_version.into(),
                capabilities: ClientCapabilities::default(),
                profile,
                providers,
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
        input: mini_agent_protocol::TurnInput,
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

    /// Runs a turn through the same request and event path used by a remote
    /// client, including queued steer and follow-up input owned by this local
    /// frontend.
    pub async fn run_turn<S: EventSink + Send>(
        &mut self,
        prompt: impl Into<String>,
        sink: &mut S,
    ) -> Result<RuntimeTurnResult, String> {
        let batch = self.run_turn_batch(prompt, sink).await?;
        batch
            .turns
            .into_iter()
            .last()
            .ok_or_else(|| "app server settled no turns".to_string())
    }

    /// Runs a start request and drains queued steer/follow-up turns until the
    /// App Server reports the active thread idle.
    pub async fn run_turn_batch<S: EventSink + Send>(
        &mut self,
        prompt: impl Into<String>,
        sink: &mut S,
    ) -> Result<RuntimeTurnBatch, String> {
        self.run_turn_batch_until(prompt, sink, None)
            .await?
            .ok_or_else(|| "turn timed out".to_string())
    }

    /// Runs a turn with a frontend deadline and settles a timed-out turn
    /// before returning. The App Server remains the owner of cancellation,
    /// event ordering, and durable checkpoint persistence.
    pub async fn run_turn_batch_with_timeout<S: EventSink + Send>(
        &mut self,
        prompt: impl Into<String>,
        timeout: Duration,
        sink: &mut S,
    ) -> Result<Option<RuntimeTurnBatch>, String> {
        self.run_turn_batch_until(prompt, sink, Some(Instant::now() + timeout))
            .await
    }

    async fn run_turn_batch_until<S: EventSink + Send>(
        &mut self,
        prompt: impl Into<String>,
        sink: &mut S,
        deadline: Option<Instant>,
    ) -> Result<Option<RuntimeTurnBatch>, String> {
        let thread_id = self.connection.thread_id().await;
        let submission = self
            .start_turn(
                thread_id.clone(),
                TurnInput::new(TurnInputMode::Start, prompt.into()),
            )
            .await
            .map_err(|error| error.message)?;
        let turn_id = match submission {
            TurnSubmission::Started { turn_id } => turn_id,
            other => return Err(format!("turn was not started: {other:?}")),
        };
        let mut finished_turn_ids = Vec::new();
        let mut timed_out = false;
        loop {
            let event = match deadline {
                Some(deadline) if !timed_out => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, self.next_event()).await {
                        Ok(event) => event,
                        Err(_) => {
                            let _ = self.interrupt(thread_id.clone(), turn_id.clone()).await;
                            timed_out = true;
                            self.next_event().await
                        }
                    }
                }
                _ => self.next_event().await,
            }
            .map_err(|error| error.message)?;
            let finished = matches!(event.event, Event::TurnFinished { .. });
            let finished_turn_id = event.turn_id.clone();
            sink.emit(event);
            if finished {
                finished_turn_ids.push(
                    finished_turn_id.clone().ok_or_else(|| {
                        "turn finished event did not include a turn id".to_string()
                    })?,
                );
                let checkpoint = self.read_idle_checkpoint().await?;
                if checkpoint.last_turn_id == finished_turn_id {
                    break;
                }
            }
        }
        let mut turns = Vec::with_capacity(finished_turn_ids.len());
        for turn_id in finished_turn_ids {
            turns.push(self.read_settled_turn(turn_id).await?);
        }
        if timed_out {
            self.control.clear_cancel();
            return Ok(None);
        }
        for _ in 0..8 {
            if let Some(input) = self
                .control
                .take_steer_input()
                .or_else(|| self.control.take_follow_up_input())
            {
                let next = Box::pin(self.run_turn_batch_until(input.text, sink, deadline)).await?;
                let Some(mut next) = next else {
                    return Ok(None);
                };
                let mut turns = turns;
                turns.append(&mut next.turns);
                return Ok(Some(RuntimeTurnBatch { turns }));
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        Ok(Some(RuntimeTurnBatch { turns }))
    }

    /// Applies a host-side update through the local App Server service.
    pub async fn update_thread(&mut self, update: ThreadUpdate) -> Result<(), String> {
        self.connection
            .runtime_management()?
            .update_thread(update)
            .await
    }

    pub async fn start_new_thread(&mut self) -> Result<(), String> {
        self.connection
            .runtime_management()?
            .start_new_thread()
            .await
    }

    pub async fn read_checkpoint(&mut self) -> Result<ThreadCheckpoint, String> {
        self.connection
            .runtime_management()?
            .read_checkpoint()
            .await
    }

    pub async fn checkpoint_seq(&self) -> Option<u64> {
        self.connection
            .runtime_management()
            .ok()?
            .checkpoint_seq()
            .await
            .ok()
            .flatten()
    }

    pub async fn workflow_state(&mut self) -> Result<WorkflowState, JsonRpcError> {
        self.call(METHOD_WORKFLOW_STATE, serde_json::json!({}))
            .await
    }

    pub async fn set_collaboration_mode(
        &mut self,
        mode: CollaborationModeKind,
    ) -> Result<ThreadSettingsUpdateResult, JsonRpcError> {
        self.update_thread_settings(mode, None).await
    }

    pub async fn update_thread_settings(
        &mut self,
        mode: CollaborationModeKind,
        builtin_tools: Option<Vec<String>>,
    ) -> Result<ThreadSettingsUpdateResult, JsonRpcError> {
        let thread_id = self.connection.thread_id().await;
        self.call(
            METHOD_THREAD_SETTINGS_UPDATE,
            ThreadSettingsUpdateParams {
                thread_id,
                collaboration_mode: CollaborationMode { mode },
                builtin_tools,
            },
        )
        .await
    }

    pub async fn set_goal(
        &mut self,
        objective: Option<String>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> Result<ThreadGoalSetResponse, JsonRpcError> {
        let thread_id = self.connection.thread_id().await;
        self.call(
            METHOD_THREAD_GOAL_SET,
            ThreadGoalSetParams {
                thread_id,
                objective,
                status,
                token_budget,
            },
        )
        .await
    }

    pub async fn get_goal(&mut self) -> Result<ThreadGoalGetResponse, JsonRpcError> {
        let thread_id = self.connection.thread_id().await;
        self.call(
            METHOD_THREAD_GOAL_GET,
            mini_agent_app_server_protocol::ThreadGoalGetParams { thread_id },
        )
        .await
    }

    pub async fn clear_goal(&mut self) -> Result<ThreadGoalClearResponse, JsonRpcError> {
        let thread_id = self.connection.thread_id().await;
        self.call(
            METHOD_THREAD_GOAL_CLEAR,
            mini_agent_app_server_protocol::ThreadGoalClearParams { thread_id },
        )
        .await
    }

    pub async fn session_info(&mut self) -> Result<Option<SessionInfoResult>, JsonRpcError> {
        self.call(METHOD_SESSION_INFO, serde_json::json!({})).await
    }

    pub async fn world_state(&mut self) -> Result<WorldStateResult, JsonRpcError> {
        self.call(METHOD_WORLD_STATE, serde_json::json!({})).await
    }

    pub async fn refresh_world(&mut self) -> Result<WorldRefreshResult, JsonRpcError> {
        self.call(METHOD_WORLD_REFRESH, serde_json::json!({})).await
    }

    pub async fn set_world_execution(
        &mut self,
        approval: impl Into<String>,
        copilot: bool,
    ) -> Result<WorldSetExecutionResult, JsonRpcError> {
        self.call(
            METHOD_WORLD_SET_EXECUTION,
            WorldSetExecutionParams {
                approval: approval.into(),
                copilot,
            },
        )
        .await
    }

    pub async fn mcp_status(&mut self) -> Result<McpStatusResult, JsonRpcError> {
        self.call(METHOD_MCP_STATUS, serde_json::json!({})).await
    }

    pub async fn retry_mcp(&mut self) -> Result<McpRetryResult, JsonRpcError> {
        self.call(METHOD_MCP_RETRY, serde_json::json!({})).await
    }

    pub async fn next_event(&mut self) -> Result<EventEnvelope, JsonRpcError> {
        let notification = loop {
            let notification = self.next_notification().await?;
            if notification.method == METHOD_TURN_EVENT {
                break notification;
            }
        };
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

    pub async fn next_notification(&mut self) -> Result<JsonRpcRequest, JsonRpcError> {
        self.connection
            .next_notification()
            .await
            .map_err(|error| JsonRpcError::server_error(error.to_string()))
    }

    async fn read_settled_turn(&mut self, turn_id: TurnId) -> Result<RuntimeTurnResult, String> {
        let mut last_error = None;
        for _ in 0..16 {
            match self.read_turn(turn_id.clone()).await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    last_error = Some(error.message);
                    tokio::task::yield_now().await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "turn result is unavailable".to_string()))
    }

    async fn read_idle_checkpoint(&mut self) -> Result<ThreadReadResult, String> {
        let mut last_error = None;
        for _ in 0..256 {
            match self.read_thread(self.connection.thread_id().await).await {
                Ok(checkpoint) => return Ok(checkpoint),
                Err(error) => {
                    last_error = Some(error.message);
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "thread did not become idle".to_string()))
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
        let result = response.result.unwrap_or(serde_json::Value::Null);
        if result.get("actionId").is_some() {
            return serde_json::from_value::<mini_agent_app_server_protocol::ActionResult<T>>(
                result,
            )
            .map(|envelope| envelope.value)
            .map_err(|error| JsonRpcError::server_error(error.to_string()));
        }
        serde_json::from_value(result)
            .map_err(|error| JsonRpcError::server_error(error.to_string()))
    }
}
