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
use crate::workflows::WorkflowService;
use mini_agent_app_server_protocol::CapabilityManifest as ProtocolCapabilityManifest;
use mini_agent_app_server_protocol::ContextLimits as ProtocolContextLimits;
use mini_agent_app_server_protocol::DisabledCapability;
use mini_agent_app_server_protocol::RulePolicy as ProtocolRulePolicy;
use mini_agent_app_server_protocol::RuleSourceStatus as ProtocolRuleSourceStatus;
use mini_agent_app_server_protocol::SourceFingerprint as ProtocolSourceFingerprint;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::McpServerConfig;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_capabilities::OpenedSession;
use mini_agent_capabilities::SessionRequest;
use mini_agent_capabilities::SessionStore;
use mini_agent_capabilities::TurnCommit;
use mini_agent_capabilities::TurnStatus as SessionTurnStatus;
use mini_agent_capabilities::workspace::ApprovalController;
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
use mini_agent_host::CapabilityManifest;
use mini_agent_host::HostRuntimeFactory;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_host::WorldState;
use mini_agent_host::tool_outcome::classify_tools;
use std::path::PathBuf;

/// The settled result projected by the local App Server runtime.
pub type RuntimeTurnResult = TurnReadResult;

/// All turns settled while one start request was being serviced. A steer or
/// follow-up may cause the App Server to settle more than one turn before the
/// service becomes idle.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTurnBatch {
    pub turns: Vec<RuntimeTurnResult>,
}

/// Stable session metadata exposed to local and remote management clients.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSessionInfo {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub resumed: bool,
}

/// Result of retrying MCP servers that were deferred during startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpRetryResult {
    pub enabled_servers: Vec<String>,
    pub inactive_servers: Vec<String>,
    pub diagnostics: Vec<String>,
    pub tool_count: usize,
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
    retry_mcp_servers: Vec<mini_agent_capabilities::McpServerConfig>,
    capability_manifest: CapabilityManifest,
    approval: ApprovalController,
    workflow_service: WorkflowService,
}

impl AppServerRuntime {
    /// Builds a Host runtime and starts the same App Server protocol used by
    /// external clients. The returned client is initialized before use.
    pub async fn start(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        session_request: SessionRequest,
    ) -> Result<Self, String> {
        Self::start_with_control_and_profile(
            runtime_config,
            approval,
            config,
            session_request,
            std::sync::Arc::new(RunControl::new()),
            RuntimeProfile::default(),
        )
        .await
    }

    /// Builds a runtime with a control handle shared by the local input loop.
    pub async fn start_with_control(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        session_request: SessionRequest,
        control: std::sync::Arc<RunControl>,
    ) -> Result<Self, String> {
        Self::start_with_control_and_profile(
            runtime_config,
            approval,
            config,
            session_request,
            control,
            RuntimeProfile::default(),
        )
        .await
    }

    pub async fn start_with_profile(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        session_request: SessionRequest,
        profile: RuntimeProfile,
    ) -> Result<Self, String> {
        Self::start_with_control_and_profile(
            runtime_config,
            approval,
            config,
            session_request,
            std::sync::Arc::new(RunControl::new()),
            profile,
        )
        .await
    }

    pub async fn start_with_control_and_profile(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        session_request: SessionRequest,
        control: std::sync::Arc<RunControl>,
        profile: RuntimeProfile,
    ) -> Result<Self, String> {
        Self::start_with_control_and_profile_and_registry(
            runtime_config,
            approval,
            config,
            session_request,
            control,
            profile,
            CapabilityRegistry::builtin(),
        )
        .await
    }

    /// Builds a runtime with a host-embedded capability registry.
    pub async fn start_with_control_and_profile_and_registry(
        runtime_config: RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        session_request: SessionRequest,
        control: std::sync::Arc<RunControl>,
        profile: RuntimeProfile,
        registry: CapabilityRegistry,
    ) -> Result<Self, String> {
        let workspace = runtime_config.workspace();
        let goal_limits = runtime_config.goal_limits();
        let model_name = runtime_config.model().unwrap_or_default().to_string();
        let session = match session_request {
            SessionRequest::Disabled => None,
            other => {
                let opened = SessionStore::open(&workspace, other)
                    .map_err(|error| format!("cannot open session: {error}"))?;
                approval.bind_session_file(opened.store.path());
                Some(opened)
            }
        };
        let results = session
            .as_ref()
            .map(|opened| opened.store.result_store())
            .unwrap_or_default();
        let mini_agent_host::HarnessBuild {
            harness,
            images,
            stable_system_prompt,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            capability_manifest,
        } = HostRuntimeFactory::new(&runtime_config, approval.clone(), config)
            .with_registry(registry)
            .build(profile, results)?;
        let mut harness = harness;
        if let Some(opened) = &session {
            images.bind_session_file(opened.store.path());
            if opened.resumed {
                harness
                    .restore_session(opened.state.clone())
                    .map_err(|error| format!("cannot restore session: {error}"))?;
            }
        }
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
        let workflow_service = WorkflowService::new(
            session
                .as_ref()
                .and_then(|opened| opened.store.path().parent().map(PathBuf::from))
                .unwrap_or_else(|| world.workspace().to_path_buf()),
            goal_limits,
        );
        let connection = AppServerConnection::with_capability_manifest(
            server.clone(),
            capability_manifest_to_protocol(&capability_manifest),
        )
        .with_workflow_service(workflow_service.clone());
        let mut client = LocalAppServerClient::new(connection);
        client
            .initialize_with_profile(
                "mini-agent-cli",
                env!("CARGO_PKG_VERSION"),
                Some(capability_manifest.profile.clone()),
            )
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
            capability_manifest,
            approval,
            workflow_service,
        })
    }

    pub fn client_mut(&mut self) -> &mut LocalAppServerClient<OpenAiModel> {
        &mut self.client
    }

    /// Transfers the host-built service into an external protocol adapter.
    ///
    /// The local client and host bookkeeping are dropped; the App Server
    /// remains the single owner of the Thread execution loop.
    pub fn into_server(self) -> AppServer<OpenAiModel> {
        self.server
    }

    /// Transfers the host-built service and its capability manifest into a
    /// protocol connection for an external adapter such as ACP.
    pub fn into_connection(self) -> AppServerConnection<OpenAiModel> {
        let manifest = capability_manifest_to_protocol(&self.capability_manifest);
        AppServerConnection::with_capability_manifest(self.server, manifest)
            .with_workflow_service(self.workflow_service)
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

    pub fn capability_manifest(&self) -> &CapabilityManifest {
        &self.capability_manifest
    }

    pub fn retry_mcp_servers(&self) -> &[mini_agent_capabilities::McpServerConfig] {
        &self.retry_mcp_servers
    }

    /// Returns workflow operations bound to this runtime's session directory.
    /// A non-durable runtime uses the workspace as its local Plan directory;
    /// Goal creation still requires a durable session at the CLI policy edge.
    pub fn workflows(&self) -> WorkflowService {
        self.workflow_service.clone()
    }

    /// Returns bounded metadata without exposing the persistence store.
    pub fn session_info(&self) -> Option<RuntimeSessionInfo> {
        self.session.as_ref().map(|opened| RuntimeSessionInfo {
            session_id: opened.store.session_id().to_string(),
            thread_id: opened.store.thread_id().to_string(),
            path: opened.store.path().display().to_string(),
            resumed: opened.resumed,
        })
    }

    /// Re-detects the world and appends changed state to the same Thread
    /// context. Persistence remains owned by the App Server runtime.
    pub async fn refresh_world(&mut self) -> Result<bool, String> {
        let refreshed = WorldState::detect(
            self.world.workspace(),
            self.world.approval(),
            self.world.copilot(),
            self.world.sandbox(),
        );
        self.update_world(refreshed).await
    }

    /// Applies a resolved world snapshot to the service-owned Thread.
    pub async fn update_world(&mut self, updated: WorldState) -> Result<bool, String> {
        if updated == self.world {
            return Ok(false);
        }
        let context = updated.model_context()?;
        self.update_thread(ThreadUpdate::AppendContext(context))
            .await?;
        self.world = updated;
        let checkpoint = self.read_checkpoint().await?;
        self.record_context(&checkpoint)?;
        Ok(true)
    }

    /// Retries MCP servers deferred at startup and atomically adds any tools
    /// that loaded successfully to the service-owned Thread.
    pub async fn retry_mcp(&mut self) -> Result<McpRetryResult, String> {
        if self.retry_mcp_servers.is_empty() {
            return Ok(McpRetryResult {
                enabled_servers: Vec::new(),
                inactive_servers: Vec::new(),
                diagnostics: Vec::new(),
                tool_count: 0,
            });
        }
        let mini_agent_capabilities::McpLoadResult {
            tools,
            loaded_servers,
            diagnostics,
        } = mini_agent_capabilities::mcp::load(&self.retry_mcp_servers, self.approval.clone());
        let inactive_servers = self
            .retry_mcp_servers
            .iter()
            .filter(|server| {
                !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
            })
            .map(|server: &McpServerConfig| {
                format!("{}/{}", server.plugin_name, server.server_name)
            })
            .collect::<Vec<_>>();
        let enabled_servers = loaded_servers.iter().cloned().collect::<Vec<_>>();
        let tool_count = tools.len();
        self.update_thread(ThreadUpdate::ExtendTools(classify_tools(tools)))
            .await?;
        self.retry_mcp_servers = self
            .retry_mcp_servers
            .iter()
            .filter(|server| {
                !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
            })
            .cloned()
            .collect();
        self.enabled_mcp_servers
            .extend(enabled_servers.iter().cloned());
        self.mcp_tool_count += tool_count;
        Ok(McpRetryResult {
            enabled_servers,
            inactive_servers,
            diagnostics,
            tool_count,
        })
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

pub fn capability_manifest_to_protocol(
    manifest: &mini_agent_host::CapabilityManifest,
) -> ProtocolCapabilityManifest {
    ProtocolCapabilityManifest {
        profile: manifest.profile.clone(),
        model_provider: manifest.model_provider.clone(),
        tool_provider: manifest.tool_provider.clone(),
        extension_provider: manifest.extension_provider.clone(),
        policy_provider: manifest.policy_provider.clone(),
        enabled: manifest.enabled.clone(),
        disabled: manifest
            .disabled
            .iter()
            .map(|(name, reason)| DisabledCapability {
                name: name.clone(),
                reason: reason.clone(),
            })
            .collect(),
        extension_depth: serde_json::to_value(manifest.extension_depth)
            .expect("extension depth is serializable")
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        selected_extensions: manifest.selected_extensions.clone(),
        prompt_sources: manifest.prompt_sources.clone(),
        rule_sources: manifest.rule_sources.clone(),
        rule_source_status: manifest
            .rule_source_status
            .iter()
            .map(|status| ProtocolRuleSourceStatus {
                source: status.source.clone(),
                state: serde_json::to_value(status.state)
                    .expect("rule source state is serializable")
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                reason: status.reason.clone(),
            })
            .collect(),
        prompt_source_fingerprints: manifest
            .prompt_source_fingerprints
            .iter()
            .map(|source| ProtocolSourceFingerprint {
                source: source.source.clone(),
                fingerprint: source.fingerprint.clone(),
            })
            .collect(),
        rule_source_fingerprints: manifest
            .rule_source_fingerprints
            .iter()
            .map(|source| ProtocolSourceFingerprint {
                source: source.source.clone(),
                fingerprint: source.fingerprint.clone(),
            })
            .collect(),
        prompt_rule_precedence: manifest.prompt_rule_precedence.clone(),
        rule_resolution: manifest.rule_resolution.clone(),
        rule_conflicts: manifest.rule_conflicts.clone(),
        rule_policy: ProtocolRulePolicy {
            workspace_write: manifest.rule_policy.workspace_write,
            shell_execution: manifest.rule_policy.shell_execution,
            process_execution: manifest.rule_policy.process_execution,
            workflow_scope: serde_json::to_value(manifest.rule_policy.workflow_scope)
                .expect("workflow scope is serializable")
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
        },
        context_limits: ProtocolContextLimits {
            max_context_bytes: manifest.context_limits.max_context_bytes,
            max_context_item_bytes: manifest.context_limits.max_context_item_bytes,
            max_user_input_bytes: manifest.context_limits.max_user_input_bytes,
            max_model_response_bytes: manifest.context_limits.max_model_response_bytes,
            max_tool_output_bytes: manifest.context_limits.max_tool_output_bytes,
            max_tool_calls_per_step: manifest.context_limits.max_tool_calls_per_step,
        },
        sandbox: manifest.sandbox.clone(),
        security: manifest.security.clone(),
    }
}
