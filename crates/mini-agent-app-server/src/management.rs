//! Runtime management operations shared by local and JSON-RPC clients.

use crate::action::{ActionFailure, ActionResponse, ActionResult};
use crate::goal_runtime::{GoalRuntimeEvent, GoalRuntimeHandle};
use crate::goal_service::ThreadGoalRequestProcessor;
use crate::notification::RuntimeNotification;
use crate::runtime_actor::RuntimeCommand;
use crate::runtime_command::RuntimeCommandClient;
use crate::status::RuntimeStatusHandle;
use crate::thread_settings::ThreadSettingsService;
use crate::worker::Command;
use crate::{AppServer, AppServerError, McpRetryResult, RuntimeSessionInfo, RuntimeTurnResult};
use mini_agent_capabilities::TurnStatus as SessionTurnStatus;
use mini_agent_capabilities::{
    ApprovalController, ApprovalPolicy, McpServerConfig, OpenedSession, SecurityPreset,
    SessionItem, TurnCommit,
};
use mini_agent_core::{HarnessConfig, ThreadCheckpoint};
use mini_agent_host::WorldState;
use mini_agent_protocol::{Message, Model, ThreadId, TurnStatus};
use tokio::sync::{broadcast, mpsc, oneshot};

pub(crate) struct RuntimeActorState {
    pub(crate) management: RuntimeManagementState,
    pub(crate) goal_runtime_handle: GoalRuntimeHandle,
    pub(crate) commands: mpsc::Sender<Command>,
    pub(crate) approval: ApprovalController,
    pub(crate) builtin_tools: mini_agent_host::BuiltinToolSelection,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
    pub(crate) stable_system_prompt: Option<String>,
    pub(crate) settings_notifications: broadcast::Sender<SettingsRuntimeEvent>,
    pub(crate) notifications: broadcast::Sender<RuntimeNotification>,
    pub(crate) status: RuntimeStatusHandle,
    revision: crate::action::RuntimeRevision,
}

#[derive(Clone, Debug)]
pub(crate) struct SettingsRuntimeEvent {
    pub(crate) thread_id: ThreadId,
    pub(crate) active: bool,
    pub(crate) builtin_tools: Vec<String>,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
    pub(crate) state_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThreadSettingsRuntimeSnapshot {
    pub(crate) builtin_tools: Vec<String>,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
}

pub(crate) struct RuntimeManagementState {
    session: Option<OpenedSession>,
    active_thread_id: ThreadId,
    world: WorldState,
    mcp: McpRuntimeState,
    local_checkpoint_seq: u64,
    pub(crate) base_harness_config: HarnessConfig,
}

struct McpRuntimeState {
    enabled_servers: Vec<String>,
    tool_count: usize,
    retry_servers: Vec<McpServerConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpRuntimeSnapshot {
    pub(crate) enabled_servers: Vec<String>,
    pub(crate) inactive_servers: Vec<String>,
    pub(crate) tool_count: usize,
    pub(crate) retry_available: bool,
}

/// Handle for runtime management commands.
///
/// Mutable session, world, and MCP state is owned by the App Server worker
/// after RuntimeServices binds this handle to a workflow store.
pub struct RuntimeManagementService<M> {
    pub(crate) server: AppServer<M>,
    client: RuntimeCommandClient,
    state: Option<RuntimeManagementState>,
    approval: ApprovalController,
    goal_notifications: broadcast::Sender<GoalRuntimeEvent>,
    settings_notifications: broadcast::Sender<SettingsRuntimeEvent>,
    notifications: broadcast::Sender<RuntimeNotification>,
    status: RuntimeStatusHandle,
}

impl<M> Clone for RuntimeManagementService<M> {
    fn clone(&self) -> Self {
        debug_assert!(self.state.is_none());
        Self {
            server: self.server.clone(),
            client: self.client.clone(),
            state: None,
            approval: self.approval.clone(),
            goal_notifications: self.goal_notifications.clone(),
            settings_notifications: self.settings_notifications.clone(),
            notifications: self.notifications.clone(),
            status: self.status.clone(),
        }
    }
}

impl<M: Model + Send + 'static> RuntimeManagementService<M> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
    ) -> Self {
        Self::new_with_harness_config(
            server,
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            approval,
            HarnessConfig::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_harness_config(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
        base_harness_config: HarnessConfig,
    ) -> Self {
        let active_thread_id = server.thread_id().clone();
        let local_checkpoint_seq = 0;
        let (goal_notifications, _) = broadcast::channel(64);
        let (settings_notifications, _) = broadcast::channel(64);
        let notifications = server.notifications();
        let status = server.runtime_status_handle();
        let client =
            RuntimeCommandClient::new(server.command_sender(), server.runtime_revision_handle());
        Self {
            server,
            client,
            state: Some(RuntimeManagementState {
                session,
                active_thread_id,
                world,
                mcp: McpRuntimeState {
                    enabled_servers: enabled_mcp_servers,
                    tool_count: mcp_tool_count,
                    retry_servers: retry_mcp_servers,
                },
                local_checkpoint_seq,
                base_harness_config,
            }),
            approval,
            goal_notifications,
            settings_notifications,
            notifications,
            status,
        }
    }

    pub(crate) fn bind_thread_services(
        self,
        settings: ThreadSettingsService,
        goals: ThreadGoalRequestProcessor,
    ) -> Result<(Self, ThreadSettingsService, ThreadGoalRequestProcessor), String> {
        let Self {
            server,
            client,
            state,
            approval,
            goal_notifications,
            settings_notifications,
            notifications,
            status,
        } = self;
        let management = state.ok_or_else(|| "runtime state is already bound".to_string())?;
        let stable_system_prompt = settings.stable_system_prompt().map(str::to_string);
        let continuation_mode = management
            .session
            .as_ref()
            .and_then(|opened| opened.store.continuation_mode())
            .map(|mode| match mode {
                "continuous" => mini_agent_app_server_protocol::ContinuationMode::Continuous,
                _ => mini_agent_app_server_protocol::ContinuationMode::Manual,
            })
            .unwrap_or_default();
        let verifier_config = goals.verifier_config();
        let store = goals.into_store().map_err(|error| error.to_string())?;
        let goal_dir = store
            .load_goal_state()
            .map_err(|error| error.to_string())?
            .map(|_| store.goal_dir());
        approval.set_goal_dir(goal_dir);
        let goal_service = GoalRuntimeHandle::with_notifications(
            store,
            goal_notifications.clone(),
            verifier_config.clone(),
            Some(notifications.clone()),
        );
        let commands = server.command_sender();
        server
            .install_runtime_state(RuntimeActorState {
                management,
                goal_runtime_handle: goal_service,
                commands,
                approval: approval.clone(),
                builtin_tools: mini_agent_host::BuiltinToolSelection::default(),
                continuation_mode,
                stable_system_prompt: stable_system_prompt.clone(),
                settings_notifications: settings_notifications.clone(),
                notifications: notifications.clone(),
                status: status.clone(),
                revision: crate::action::RuntimeRevision::default(),
            })
            .map_err(|error| error.to_string())?;
        let settings = ThreadSettingsService::bound(client.clone(), stable_system_prompt);
        let goals = ThreadGoalRequestProcessor::bound(client.clone(), verifier_config);
        Ok((
            Self {
                server,
                client,
                state: None,
                approval,
                goal_notifications,
                settings_notifications,
                notifications,
                status,
            },
            settings,
            goals,
        ))
    }

    pub(crate) async fn session_info_action(
        &self,
    ) -> Result<ActionResponse<Option<RuntimeSessionInfo>>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::SessionInfo { reply })
            .await
    }

    pub(crate) fn notifications(&self) -> broadcast::Sender<RuntimeNotification> {
        self.notifications.clone()
    }

    pub async fn checkpoint_seq(&self) -> Result<Option<u64>, String> {
        self.request(|reply| RuntimeCommand::CheckpointSeq { reply })
            .await
    }

    pub(crate) async fn thread_id(&self) -> Result<ThreadId, String> {
        self.request(|reply| RuntimeCommand::ThreadId { reply })
            .await
    }

    pub(crate) async fn world(&self) -> Result<WorldState, String> {
        self.request(|reply| RuntimeCommand::World { reply }).await
    }

    pub(crate) async fn world_action(&self) -> Result<ActionResponse<WorldState>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::World { reply })
            .await
    }

    pub(crate) async fn refresh_world_action(&self) -> Result<ActionResponse<bool>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::RefreshWorld { reply })
            .await
    }

    pub(crate) async fn set_execution_action(
        &self,
        access: SecurityPreset,
        policy: ApprovalPolicy,
    ) -> Result<ActionResponse<bool>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::SetExecution {
                access,
                policy,
                reply,
            })
            .await
    }

    pub(crate) async fn mcp_status_action(
        &self,
    ) -> Result<ActionResponse<McpRuntimeSnapshot>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::McpStatus { reply })
            .await
    }

    pub(crate) async fn retry_mcp_action(
        &self,
    ) -> Result<ActionResponse<McpRetryResult>, ActionFailure> {
        let approval = self.approval.clone();
        self.client
            .request_action(|reply| RuntimeCommand::RetryMcp { approval, reply })
            .await
    }

    pub async fn update_thread(&self, update: crate::ThreadUpdate) -> Result<(), String> {
        self.request(|reply| RuntimeCommand::UpdateThread { update, reply })
            .await
    }

    pub async fn read_checkpoint(&self) -> Result<ThreadCheckpoint, String> {
        self.request(|reply| RuntimeCommand::ReadCheckpoint { reply })
            .await
    }

    pub async fn start_new_thread(&self) -> Result<(), String> {
        self.request(|reply| RuntimeCommand::StartNewThread { reply })
            .await
    }

    async fn request<T, F>(&self, build: F) -> Result<T, String>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> RuntimeCommand,
    {
        self.client
            .request_action(build)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
            .map_err(|error| error.to_string())
    }
}

impl RuntimeActorState {
    pub(crate) fn revision(&self) -> crate::action::RuntimeRevision {
        self.revision
    }

    pub(crate) fn advance_revision(&mut self) -> crate::action::RuntimeRevision {
        self.revision = self.revision.next();
        self.revision
    }
}

impl RuntimeManagementState {
    pub(crate) fn session_info(&self) -> Option<RuntimeSessionInfo> {
        self.session.as_ref().map(|opened| RuntimeSessionInfo {
            session_id: opened.store.session_id().to_string(),
            thread_id: opened.store.thread_id().to_string(),
            path: opened.store.path().display().to_string(),
            resumed: opened.resumed,
        })
    }

    pub(crate) fn checkpoint_seq(&self) -> Option<u64> {
        self.session
            .as_ref()
            .map(|opened| opened.store.checkpoint_seq())
    }

    pub(crate) fn current_checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq().unwrap_or(self.local_checkpoint_seq)
    }

    pub(crate) fn thread_id(&self) -> ThreadId {
        self.session
            .as_ref()
            .map(|opened| ThreadId::new(opened.store.thread_id().to_string()))
            .unwrap_or_else(|| self.active_thread_id.clone())
    }

    pub(crate) fn world(&self) -> WorldState {
        self.world.clone()
    }

    pub(crate) fn set_world(&mut self, world: WorldState) {
        self.world = world;
    }

    pub(crate) fn mcp_status(&self) -> McpRuntimeSnapshot {
        let inactive_servers = self
            .mcp
            .retry_servers
            .iter()
            .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
            .collect::<Vec<_>>();
        McpRuntimeSnapshot {
            enabled_servers: self.mcp.enabled_servers.clone(),
            inactive_servers,
            tool_count: self.mcp.tool_count,
            retry_available: !self.mcp.retry_servers.is_empty(),
        }
    }

    pub(crate) fn retry_mcp_servers(&self) -> Vec<McpServerConfig> {
        self.mcp.retry_servers.clone()
    }

    pub(crate) fn record_mcp_retry(
        &mut self,
        loaded_servers: &[String],
        enabled_servers: &[String],
        tool_count: usize,
    ) {
        self.mcp.retry_servers.retain(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        });
        self.mcp
            .enabled_servers
            .extend(enabled_servers.iter().cloned());
        self.mcp.tool_count += tool_count;
    }

    pub(crate) fn session_mut(&mut self) -> Option<&mut OpenedSession> {
        self.session.as_mut()
    }

    pub(crate) fn persist_continuation_mode(
        &mut self,
        mode: mini_agent_app_server_protocol::ContinuationMode,
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let mode = match mode {
            mini_agent_app_server_protocol::ContinuationMode::Manual => "manual",
            mini_agent_app_server_protocol::ContinuationMode::Continuous => "continuous",
        };
        session
            .store
            .set_continuation_mode(mode)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn session_items(&self) -> Option<&[SessionItem]> {
        self.session.as_ref().map(|opened| opened.store.items())
    }

    pub(crate) fn record_context(
        &mut self,
        checkpoint: &ThreadCheckpoint,
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            self.local_checkpoint_seq = self.local_checkpoint_seq.saturating_add(1);
            return Ok(());
        };
        let context = checkpoint
            .session
            .messages()
            .iter()
            .rev()
            .find(|message| matches!(message, Message::Context { .. }))
            .ok_or_else(|| {
                AppServerError::Checkpoint("no context item is available to persist".to_string())
            })?;
        session
            .store
            .record_context(context, checkpoint.session.messages())
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn record_turn(
        &mut self,
        started_at_ms: u64,
        prompt: &str,
        result: &RuntimeTurnResult,
        messages: &[Message],
        checkpoint: &[Message],
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            self.local_checkpoint_seq = self.local_checkpoint_seq.saturating_add(1);
            return Ok(());
        };
        let status = match result.status {
            TurnStatus::Completed => SessionTurnStatus::Completed,
            TurnStatus::StepLimit => SessionTurnStatus::StepLimit,
            TurnStatus::Steered => SessionTurnStatus::Steered,
            TurnStatus::Cancelled => SessionTurnStatus::Cancelled,
            TurnStatus::Failed | TurnStatus::InProgress => SessionTurnStatus::Failed,
        };
        session
            .store
            .record_turn_with_id(
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
            .map_err(AppServerError::Checkpoint)
    }
}
