//! Host-backed runtime used by local clients.
//!
//! `AppServerRuntime` is the composition root for an in-process service. The
//! host creates the provider-backed Thread and persistence state here, then
//! all turn execution goes through the same protocol client used by ACP and
//! the external JSON-RPC transport.

use crate::AppServer;
use crate::AppServerConnection;
use crate::LocalAppServerClient;
use crate::ThreadUpdate;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_core::Event;
use mini_agent_core::EventSink;
use mini_agent_core::HarnessConfig;
use mini_agent_core::RunControl;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStatus;
use mini_agent_host::ApprovalController;
use mini_agent_host::ImageStore;
use mini_agent_host::OpenAiModel;
use mini_agent_host::OpenedSession;
use mini_agent_host::RuntimeBuilder;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::SandboxKind;
use mini_agent_host::SessionRequest;
use mini_agent_host::SessionStore;
use mini_agent_host::TurnCommit;
use mini_agent_host::TurnStatus as SessionTurnStatus;

/// The settled result projected by the local App Server runtime.
pub type RuntimeTurnResult = TurnReadResult;

/// All turns settled while one start request was being serviced. A steer or
/// follow-up may cause the App Server to settle more than one turn before the
/// service becomes idle.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTurnBatch {
    pub turns: Vec<RuntimeTurnResult>,
}

/// A provider-backed App Server plus the host state needed by local clients.
pub struct AppServerRuntime {
    server: AppServer<OpenAiModel>,
    control: std::sync::Arc<RunControl>,
    client: LocalAppServerClient<OpenAiModel>,
    images: ImageStore,
    session: Option<OpenedSession>,
    thread_id: ThreadId,
    model_name: String,
    stable_system_prompt: String,
    world: mini_agent_host::WorldState,
    enabled_mcp_servers: Vec<String>,
    mcp_tool_count: usize,
    retry_mcp_servers: Vec<mini_agent_host::skills::McpServerConfig>,
}

impl AppServerRuntime {
    /// Builds a Host runtime and starts the same App Server protocol used by
    /// external clients. The returned client is initialized before use.
    pub async fn start(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        sandbox: SandboxKind,
        session_request: SessionRequest,
    ) -> Result<Self, String> {
        Self::start_with_control(
            runtime_config,
            approval,
            config,
            sandbox,
            session_request,
            std::sync::Arc::new(RunControl::new()),
        )
        .await
    }

    /// Builds a runtime with a control handle shared by the local input loop.
    pub async fn start_with_control(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        sandbox: SandboxKind,
        session_request: SessionRequest,
        control: std::sync::Arc<RunControl>,
    ) -> Result<Self, String> {
        let workspace = runtime_config.workspace();
        let model_name = runtime_config.model().unwrap_or_default().to_string();
        let mini_agent_host::HarnessBuild {
            harness,
            images,
            stable_system_prompt,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
        } = RuntimeBuilder::new(&runtime_config, approval.clone(), config, sandbox).build()?;
        let mut harness = harness;
        let session = match session_request {
            SessionRequest::Disabled => None,
            other => {
                let opened = SessionStore::open(&workspace, other)
                    .map_err(|error| format!("cannot open session: {error}"))?;
                approval.bind_session_file(opened.store.path());
                images.bind_session_file(opened.store.path());
                if opened.resumed {
                    harness
                        .restore_session(opened.state.clone())
                        .map_err(|error| format!("cannot restore session: {error}"))?;
                }
                Some(opened)
            }
        };
        let thread_id = session
            .as_ref()
            .map(|opened| ThreadId::new(opened.store.thread_id().to_string()))
            .unwrap_or_else(|| ThreadId::new("default"));
        let mut thread = Thread::new(thread_id.clone(), harness);
        if let Some(opened) = &session {
            thread.set_next_turn_number(opened.store.thread_turn_count() as u64 + 1);
        }
        let server = AppServer::new_with_control(
            ThreadStart::new(thread_id.clone()),
            thread,
            control.clone(),
        );
        let connection = AppServerConnection::new(server.clone());
        let mut client = LocalAppServerClient::new(connection);
        client
            .initialize("mini-agent-cli", env!("CARGO_PKG_VERSION"))
            .await
            .map_err(|error| format!("cannot initialize app server: {}", error.message))?;
        Ok(Self {
            server,
            control,
            client,
            images,
            session,
            thread_id,
            model_name,
            stable_system_prompt,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
        })
    }

    pub fn client_mut(&mut self) -> &mut LocalAppServerClient<OpenAiModel> {
        &mut self.client
    }

    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    pub fn session(&self) -> Option<&OpenedSession> {
        self.session.as_ref()
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    pub fn pending_input_count(&self) -> usize {
        self.control.pending_input_count()
    }

    pub fn stable_system_prompt(&self) -> &str {
        &self.stable_system_prompt
    }

    pub fn world(&self) -> &mini_agent_host::WorldState {
        &self.world
    }

    pub fn enabled_mcp_servers(&self) -> &[String] {
        &self.enabled_mcp_servers
    }

    pub fn mcp_tool_count(&self) -> usize {
        self.mcp_tool_count
    }

    pub fn retry_mcp_servers(&self) -> &[mini_agent_host::skills::McpServerConfig] {
        &self.retry_mcp_servers
    }

    pub async fn update_thread(&self, update: ThreadUpdate) -> Result<(), String> {
        self.server
            .thread_update_for(self.thread_id.clone(), update)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn start_new_thread(&mut self) -> Result<(), String> {
        let new_thread_id = {
            let Some(session) = self.session.as_mut() else {
                return Err("session persistence is disabled".to_string());
            };
            session.store.start_thread()?;
            ThreadId::new(session.store.thread_id().to_string())
        };
        self.server
            .thread_reset(self.thread_id.clone(), new_thread_id.clone(), 1)
            .await
            .map_err(|error| error.to_string())?;
        self.thread_id = new_thread_id;
        Ok(())
    }

    pub async fn read_checkpoint(&self) -> Result<mini_agent_core::ThreadCheckpoint, String> {
        self.server
            .thread_read_for(self.thread_id.clone())
            .await
            .map_err(|error| error.to_string())
    }

    pub fn record_context(
        &mut self,
        checkpoint: &mini_agent_core::ThreadCheckpoint,
    ) -> Result<(), String> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let context = checkpoint
            .session
            .messages()
            .iter()
            .rev()
            .find(|message| matches!(message, mini_agent_core::Message::Context { .. }))
            .ok_or_else(|| "no context item is available to persist".to_string())?;
        session
            .store
            .record_context(context, checkpoint.session.messages())
    }

    /// Runs one turn through the protocol client and returns the final settled
    /// turn in the service batch.
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

    /// Runs a start request and drains all queued steer/follow-up turns until
    /// the App Server reports the thread idle. This makes the service queue
    /// observable without asking a frontend to maintain a second turn loop.
    pub async fn run_turn_batch<S: EventSink + Send>(
        &mut self,
        prompt: impl Into<String>,
        sink: &mut S,
    ) -> Result<RuntimeTurnBatch, String> {
        let submission = self
            .client
            .start_turn(
                self.thread_id.clone(),
                TurnInput::new(TurnInputMode::Start, prompt.into()),
            )
            .await
            .map_err(|error| error.message)?;
        match submission {
            mini_agent_core::TurnSubmission::Started { .. } => {}
            other => return Err(format!("turn was not started: {other:?}")),
        }
        let mut finished_turn_ids = Vec::new();
        loop {
            let event = self
                .client
                .next_event()
                .await
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
            let result = self.read_settled_turn(turn_id).await?;
            turns.push(result);
        }
        for _ in 0..8 {
            if let Some(input) = self
                .control
                .take_steer_input()
                .or_else(|| self.control.take_follow_up_input())
            {
                let mut next = Box::pin(self.run_turn_batch(input.text, sink)).await?;
                let mut turns = turns;
                turns.append(&mut next.turns);
                return Ok(RuntimeTurnBatch { turns });
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        Ok(RuntimeTurnBatch { turns })
    }

    async fn read_settled_turn(
        &mut self,
        turn_id: mini_agent_core::TurnId,
    ) -> Result<mini_agent_app_server_protocol::TurnReadResult, String> {
        let mut last_error = None;
        for _ in 0..16 {
            match self.client.read_turn(turn_id.clone()).await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    last_error = Some(error.message);
                    tokio::task::yield_now().await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "turn result is unavailable".to_string()))
    }

    async fn read_idle_checkpoint(
        &mut self,
    ) -> Result<mini_agent_app_server_protocol::ThreadReadResult, String> {
        let mut last_error = None;
        for _ in 0..256 {
            match self.client.read_thread(self.thread_id.clone()).await {
                Ok(checkpoint) => return Ok(checkpoint),
                Err(error) => {
                    last_error = Some(error.message);
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "thread did not become idle".to_string()))
    }

    /// Persists a settled turn using the same session format as the legacy
    /// direct Harness path.
    pub fn record_turn(
        &mut self,
        started_at_ms: u64,
        prompt: &str,
        result: &RuntimeTurnResult,
    ) -> Result<(), String> {
        self.record_turn_with_messages(
            started_at_ms,
            prompt,
            result,
            &result.messages,
            &result.messages,
        )
    }

    fn record_turn_with_messages(
        &mut self,
        started_at_ms: u64,
        prompt: &str,
        result: &RuntimeTurnResult,
        messages: &[mini_agent_core::Message],
        checkpoint: &[mini_agent_core::Message],
    ) -> Result<(), String> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let status = match result.status {
            TurnStatus::Completed => SessionTurnStatus::Completed,
            TurnStatus::StepLimit => SessionTurnStatus::StepLimit,
            TurnStatus::Steered => SessionTurnStatus::Steered,
            TurnStatus::Cancelled => SessionTurnStatus::Cancelled,
            TurnStatus::Failed | TurnStatus::InProgress => SessionTurnStatus::Failed,
        };
        session.store.record_turn_with_id(
            result.turn_id.as_str(),
            TurnCommit {
                started_at_ms,
                prompt,
                status,
                steps: result.steps,
                error: result.error.as_deref(),
                messages,
                checkpoint,
            },
        )
    }

    pub fn record_batch(
        &mut self,
        started_at_ms: u64,
        fallback_prompt: &str,
        batch: &RuntimeTurnBatch,
    ) -> Result<(), String> {
        let mut previous_message_count = 0;
        for result in &batch.turns {
            let prompt = result
                .messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    mini_agent_core::Message::User { text } => Some(text.as_str()),
                    _ => None,
                })
                .unwrap_or(fallback_prompt);
            let turn_messages = result
                .messages
                .get(previous_message_count..)
                .unwrap_or(&result.messages)
                .to_vec();
            previous_message_count = result.messages.len();
            self.record_turn_with_messages(
                started_at_ms,
                prompt,
                result,
                &turn_messages,
                &result.messages,
            )?;
        }
        Ok(())
    }
}
