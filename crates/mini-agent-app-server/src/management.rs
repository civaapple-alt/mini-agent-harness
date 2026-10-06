//! Runtime management operations shared by local and JSON-RPC clients.

use crate::action::{ActionFailure, ActionResponse, ActionResult};
use crate::goal_runtime::{GoalRuntimeEvent, GoalRuntimeHandle};
use crate::goal_service::ThreadGoalRequestProcessor;
use crate::notification::RuntimeNotification;
use crate::runtime_actor::RuntimeCommand;
use crate::runtime_command::RuntimeCommandClient;
use crate::status::RuntimeStatusHandle;
use crate::thread_settings::{ThreadModelSettings, ThreadSettingsService};
use crate::worker::Command;
use crate::{AppServer, AppServerError, McpRetryResult, RuntimeSessionInfo, RuntimeTurnResult};
use mini_agent_capabilities::TurnStatus as SessionTurnStatus;
use mini_agent_capabilities::{
    ApprovalController, ApprovalPolicy, McpServerConfig, OpenedSession, SecurityPreset,
    SessionItem, TurnCommit,
};
use mini_agent_capabilities::{BackgroundShellManager, ScheduledTaskManager};
use mini_agent_core::{HarnessConfig, ThreadCheckpoint};
use mini_agent_host::WorldState;
use mini_agent_protocol::{Message, Model, ReasoningSelection, ThreadId, TurnSource, TurnStatus};
use tokio::sync::{broadcast, mpsc, oneshot};

fn required_child_param<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, AppServerError> {
    value.ok_or_else(|| AppServerError::Checkpoint(format!("{name} is required")))
}

pub(crate) struct RuntimeActorState {
    pub(crate) management: RuntimeManagementState,
    pub(crate) goal_runtime_handle: GoalRuntimeHandle,
    pub(crate) commands: mpsc::Sender<Command>,
    pub(crate) approval: ApprovalController,
    pub(crate) builtin_tools: mini_agent_host::BuiltinToolSelection,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
    pub(crate) model_selection: Option<mini_agent_protocol::ModelSelection>,
    pub(crate) reasoning_selection: Option<ReasoningSelection>,
    pub(crate) stable_system_prompt: Option<String>,
    pub(crate) settings_notifications: broadcast::Sender<SettingsRuntimeEvent>,
    pub(crate) notifications: broadcast::Sender<RuntimeNotification>,
    pub(crate) status: RuntimeStatusHandle,
    pub(crate) background_shells: BackgroundShellManager,
    pub(crate) scheduled_tasks: ScheduledTaskManager,
    pub(crate) user_questions: Option<crate::UserQuestionBroker>,
    revision: crate::action::RuntimeRevision,
}

#[derive(Clone, Debug)]
pub(crate) struct SettingsRuntimeEvent {
    pub(crate) thread_id: ThreadId,
    pub(crate) active: bool,
    pub(crate) builtin_tools: Vec<String>,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
    pub(crate) model_selection: Option<mini_agent_protocol::ModelSelection>,
    pub(crate) reasoning_selection: Option<ReasoningSelection>,
    pub(crate) state_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThreadSettingsRuntimeSnapshot {
    pub(crate) active: bool,
    pub(crate) builtin_tools: Vec<String>,
    pub(crate) continuation_mode: mini_agent_app_server_protocol::ContinuationMode,
    pub(crate) model_selection: Option<mini_agent_protocol::ModelSelection>,
    pub(crate) reasoning_selection: Option<ReasoningSelection>,
}

pub(crate) struct RuntimeManagementState {
    session: Option<OpenedSession>,
    active_thread_id: ThreadId,
    world: WorldState,
    mcp: McpRuntimeState,
    local_checkpoint_seq: u64,
    pub(crate) base_harness_config: HarnessConfig,
    pub(crate) skill_discovery: Option<mini_agent_capabilities::Discovery>,
    pub(crate) skill_discovery_refresh: Option<mini_agent_host::SkillDiscoveryRefresh>,
    pub(crate) skill_read_roots: mini_agent_capabilities::SkillReadRoots,
    pub(crate) background_shells: BackgroundShellManager,
    pub(crate) scheduled_tasks: ScheduledTaskManager,
}

pub(crate) struct TurnPersistence<'a> {
    pub(crate) result: &'a RuntimeTurnResult,
    pub(crate) messages: &'a [Message],
    pub(crate) tool_arguments: &'a [(String, serde_json::Value)],
    pub(crate) presentation: Option<&'a mini_agent_capabilities::TurnPresentation>,
    pub(crate) execution_resume: bool,
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
    initial_builtin_tools: mini_agent_host::BuiltinToolSelection,
    approval: ApprovalController,
    goal_notifications: broadcast::Sender<GoalRuntimeEvent>,
    settings_notifications: broadcast::Sender<SettingsRuntimeEvent>,
    notifications: broadcast::Sender<RuntimeNotification>,
    status: RuntimeStatusHandle,
    user_questions: Option<crate::UserQuestionBroker>,
}

impl<M> Clone for RuntimeManagementService<M> {
    fn clone(&self) -> Self {
        debug_assert!(self.state.is_none());
        Self {
            server: self.server.clone(),
            client: self.client.clone(),
            state: None,
            initial_builtin_tools: self.initial_builtin_tools.clone(),
            approval: self.approval.clone(),
            goal_notifications: self.goal_notifications.clone(),
            settings_notifications: self.settings_notifications.clone(),
            notifications: self.notifications.clone(),
            status: self.status.clone(),
            user_questions: self.user_questions.clone(),
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
        Self::new_with_harness_config_and_skills(
            server,
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            approval,
            base_harness_config,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_harness_config_and_skills(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
        base_harness_config: HarnessConfig,
        skill_discovery: Option<mini_agent_capabilities::Discovery>,
    ) -> Self {
        Self::new_with_harness_config_and_skills_and_background_shells(
            server,
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            approval,
            base_harness_config,
            skill_discovery,
            BackgroundShellManager::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_harness_config_and_skills_and_background_shells(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
        base_harness_config: HarnessConfig,
        skill_discovery: Option<mini_agent_capabilities::Discovery>,
        background_shells: BackgroundShellManager,
    ) -> Self {
        Self::new_with_harness_config_and_skills_and_task_managers(
            server,
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            approval,
            base_harness_config,
            skill_discovery,
            background_shells,
            ScheduledTaskManager::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_harness_config_and_skills_and_task_managers(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
        base_harness_config: HarnessConfig,
        skill_discovery: Option<mini_agent_capabilities::Discovery>,
        background_shells: BackgroundShellManager,
        scheduled_tasks: ScheduledTaskManager,
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
                skill_discovery,
                skill_discovery_refresh: None,
                skill_read_roots: mini_agent_capabilities::SkillReadRoots::default(),
                background_shells,
                scheduled_tasks,
            }),
            initial_builtin_tools: mini_agent_host::BuiltinToolSelection::default(),
            approval,
            goal_notifications,
            settings_notifications,
            notifications,
            status,
            user_questions: None,
        }
    }

    pub fn with_skill_discovery_refresh(
        mut self,
        refresh: Option<mini_agent_host::SkillDiscoveryRefresh>,
        skill_read_roots: mini_agent_capabilities::SkillReadRoots,
    ) -> Self {
        if let Some(state) = self.state.as_mut() {
            state.skill_discovery_refresh = refresh;
            state.skill_read_roots = skill_read_roots;
        }
        self
    }

    pub fn with_initial_builtin_tools(
        mut self,
        builtin_tools: mini_agent_host::BuiltinToolSelection,
    ) -> Self {
        self.initial_builtin_tools = builtin_tools;
        self
    }

    pub fn with_user_questions(mut self, broker: crate::UserQuestionBroker) -> Self {
        self.user_questions = Some(broker);
        self
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
            initial_builtin_tools,
            approval,
            goal_notifications,
            settings_notifications,
            notifications,
            status,
            user_questions,
        } = self;
        let management = state.ok_or_else(|| "runtime state is already bound".to_string())?;
        let stable_system_prompt = settings.stable_system_prompt().map(str::to_string);
        let persisted_model_settings = management
            .session
            .as_ref()
            .map(|opened| ThreadModelSettings {
                selection: opened.store.model_selection().cloned(),
                reasoning_selection: opened.store.reasoning_selection().cloned(),
            })
            .unwrap_or_default();
        settings.set_initial_model_settings(persisted_model_settings.clone());
        let model_settings = settings.model_settings_handle();
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
        let plan_active = goal_service.plan_active();
        let plan_path = goal_service.plan_file_path();
        let session_plan = (plan_active || plan_path.is_file()).then_some(plan_path);
        approval.set_plan_context(session_plan, plan_active);
        let commands = server.command_sender();
        let background_shells = management.background_shells.clone();
        let scheduled_tasks = management.scheduled_tasks.clone();
        server
            .install_runtime_state(RuntimeActorState {
                management,
                goal_runtime_handle: goal_service,
                commands,
                approval: approval.clone(),
                builtin_tools: initial_builtin_tools.clone(),
                continuation_mode,
                model_selection: persisted_model_settings.selection,
                reasoning_selection: persisted_model_settings.reasoning_selection,
                stable_system_prompt: stable_system_prompt.clone(),
                settings_notifications: settings_notifications.clone(),
                notifications: notifications.clone(),
                status: status.clone(),
                background_shells,
                scheduled_tasks,
                user_questions,
                revision: crate::action::RuntimeRevision::default(),
            })
            .map_err(|error| error.to_string())?;
        let settings =
            ThreadSettingsService::bound(client.clone(), stable_system_prompt, model_settings);
        let goals = ThreadGoalRequestProcessor::bound(client.clone(), verifier_config);
        Ok((
            Self {
                server,
                client,
                state: None,
                initial_builtin_tools,
                approval,
                goal_notifications,
                settings_notifications,
                notifications,
                status,
                user_questions: None,
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

    pub(crate) async fn read_notebook_action(
        &self,
        scope: String,
    ) -> Result<ActionResponse<serde_json::Value>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::ReadNotebook { scope, reply })
            .await
    }

    pub(crate) async fn write_notebook_action(
        &self,
        key: String,
        content: String,
        append: bool,
        importance: String,
        keywords: Option<Vec<String>>,
        evidence: Option<Vec<serde_json::Value>>,
    ) -> Result<ActionResponse<serde_json::Value>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::WriteNotebook {
                key,
                content,
                append,
                importance,
                keywords,
                evidence,
                reply,
            })
            .await
    }

    pub(crate) async fn forget_notebook_action(
        &self,
        key: String,
    ) -> Result<ActionResponse<serde_json::Value>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::ForgetNotebook { key, reply })
            .await
    }

    pub(crate) async fn child_task_action(
        &self,
        params: mini_agent_app_server_protocol::ChildTaskParams,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::ChildTaskResult>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::ChildTask { params, reply })
            .await
    }

    pub(crate) async fn session_control_action(
        &self,
        params: mini_agent_app_server_protocol::SessionControlParams,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::SessionControlResult>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::SessionControl { params, reply })
            .await
    }

    pub(crate) async fn child_steer_request_action(
        &self,
        thread_id: ThreadId,
        request_id: String,
        turn_id: String,
        step: mini_agent_capabilities::ChildSteerRequestStep,
        accepted_status: Option<String>,
    ) -> Result<
        ActionResponse<Option<mini_agent_capabilities::ChildTaskMutationResult>>,
        ActionFailure,
    > {
        self.client
            .request_action(|reply| RuntimeCommand::ChildSteerRequest {
                thread_id,
                request_id,
                turn_id,
                step,
                accepted_status,
                reply,
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fork_session_action(
        &self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
        context_policy: mini_agent_app_server_protocol::ForkContextPolicy,
        operation_id: Option<String>,
        operation_attempt: Option<u32>,
        operation_prompt: Option<String>,
        operation_group_id: Option<String>,
        execution_mode: Option<String>,
        group_sequence: Option<u32>,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::SessionForkResult>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::PrepareSessionFork {
                source_thread_id,
                new_thread_id,
                context_policy,
                operation_id,
                operation_attempt,
                operation_prompt,
                operation_group_id,
                execution_mode,
                group_sequence,
                reply,
            })
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

    pub(crate) async fn refresh_skills_action(
        &self,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::SkillsListResult>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::RefreshSkills { reply })
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

    pub(crate) async fn background_task_list_action(
        &self,
    ) -> Result<ActionResponse<Vec<mini_agent_app_server_protocol::BackgroundTask>>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::BackgroundTaskList { reply })
            .await
    }

    pub(crate) async fn background_task_read_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::BackgroundTask>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::BackgroundTaskRead { task_id, reply })
            .await
    }

    pub(crate) async fn background_task_logs_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::BackgroundTaskLogs>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::BackgroundTaskLogs { task_id, reply })
            .await
    }

    pub(crate) async fn background_task_stop_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::BackgroundTask>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::BackgroundTaskStop { task_id, reply })
            .await
    }

    pub(crate) async fn background_task_restart_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::BackgroundTask>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::BackgroundTaskRestart { task_id, reply })
            .await
    }

    pub(crate) async fn scheduled_task_list_action(
        &self,
    ) -> Result<ActionResponse<Vec<mini_agent_app_server_protocol::ScheduledTask>>, ActionFailure>
    {
        self.client
            .request_action(|reply| RuntimeCommand::ScheduledTaskList { reply })
            .await
    }

    pub(crate) async fn scheduled_task_read_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::ScheduledTask>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::ScheduledTaskRead { task_id, reply })
            .await
    }

    pub(crate) async fn scheduled_task_cancel_action(
        &self,
        task_id: String,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::ScheduledTask>, ActionFailure> {
        self.client
            .request_action(|reply| RuntimeCommand::ScheduledTaskCancel { task_id, reply })
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

pub(crate) fn project_background_task(
    task: mini_agent_capabilities::BackgroundShellTask,
) -> mini_agent_app_server_protocol::BackgroundTask {
    mini_agent_app_server_protocol::BackgroundTask {
        task_id: task.task_id,
        owner_thread_id: ThreadId::new(task.owner_thread_id),
        state: task.state,
        command_summary: task.command_summary,
        command_hash: task.command_hash,
        working_directory: task.working_directory,
        process_id: task.process_id,
        started_at: task.started_at,
        stopped_at: task.stopped_at,
        exit_code: task.exit_code,
        log_bytes: task.log_bytes,
        log_truncated: task.log_truncated,
    }
}

pub(crate) fn project_background_logs(
    logs: mini_agent_capabilities::BackgroundShellLogs,
) -> mini_agent_app_server_protocol::BackgroundTaskLogs {
    mini_agent_app_server_protocol::BackgroundTaskLogs {
        task_id: logs.task_id,
        text: logs.text,
        bytes: logs.bytes,
        truncated: logs.truncated,
    }
}

pub(crate) fn project_scheduled_task(
    task: mini_agent_capabilities::ScheduledTask,
) -> mini_agent_app_server_protocol::ScheduledTask {
    mini_agent_app_server_protocol::ScheduledTask {
        task_id: task.task_id,
        owner_thread_id: ThreadId::new(task.owner_thread_id),
        state: task.state,
        trigger_type: task.trigger_type,
        summary: task.summary,
        created_at: task.created_at,
        due_at: task.due_at,
        ready_at: task.ready_at,
        cancelled_at: task.cancelled_at,
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
    pub(crate) fn refresh_skill_catalog(
        &mut self,
    ) -> Result<(mini_agent_app_server_protocol::SkillsListResult, bool), AppServerError> {
        let Some(refresh) = self.skill_discovery_refresh.as_ref() else {
            return Ok((self.skill_catalog_snapshot(), false));
        };
        let refreshed = refresh.refresh().map_err(AppServerError::SkillDiscovery)?;
        let current_fingerprint = self
            .skill_discovery
            .as_ref()
            .map(mini_agent_capabilities::Discovery::prompt_fingerprint)
            .transpose()
            .map_err(AppServerError::SkillDiscovery)?
            .flatten();
        let refreshed_fingerprint = refreshed
            .prompt_fingerprint()
            .map_err(AppServerError::SkillDiscovery)?;
        let current_skill_roots = self
            .skill_discovery
            .as_ref()
            .map(mini_agent_capabilities::Discovery::skill_read_roots)
            .unwrap_or_default();
        let refreshed_skill_roots = refreshed.skill_read_roots();
        let roots_changed = current_skill_roots != refreshed_skill_roots;
        let changed = current_fingerprint != refreshed_fingerprint || roots_changed;
        if changed {
            if roots_changed {
                self.skill_read_roots
                    .replace(refreshed_skill_roots)
                    .map_err(AppServerError::SkillDiscovery)?;
            }
            if let Some(current) = self.skill_discovery.as_mut() {
                current.replace_skills(refreshed);
            } else {
                self.skill_discovery = Some(refreshed);
            }
        }
        Ok((self.skill_catalog_snapshot(), changed))
    }

    fn skill_catalog_snapshot(&self) -> mini_agent_app_server_protocol::SkillsListResult {
        mini_agent_app_server_protocol::SkillsListResult {
            skills: self
                .skill_discovery
                .as_ref()
                .map(mini_agent_capabilities::Discovery::skill_catalog)
                .unwrap_or_default()
                .into_iter()
                .map(|skill| mini_agent_app_server_protocol::AvailableSkill {
                    name: skill.name,
                    qualified_name: skill.qualified_name,
                    aliases: skill.aliases,
                    description: skill.description,
                    source: skill.source,
                    origin: Some(match skill.origin {
                        mini_agent_capabilities::SkillOrigin::BuiltinGroup => {
                            mini_agent_app_server_protocol::SkillOrigin::BuiltinGroup
                        }
                        mini_agent_capabilities::SkillOrigin::UserAgents => {
                            mini_agent_app_server_protocol::SkillOrigin::UserAgents
                        }
                        mini_agent_capabilities::SkillOrigin::UserMiniAgent => {
                            mini_agent_app_server_protocol::SkillOrigin::UserMiniAgent
                        }
                        mini_agent_capabilities::SkillOrigin::Project => {
                            mini_agent_app_server_protocol::SkillOrigin::Project
                        }
                        mini_agent_capabilities::SkillOrigin::Plugin => {
                            mini_agent_app_server_protocol::SkillOrigin::Plugin
                        }
                    }),
                    group: skill.group,
                    enabled: skill.enabled,
                    model_invocable: skill.model_invocable,
                })
                .collect(),
        }
    }

    pub(crate) fn prepare_skill_context(&self) -> Result<Option<String>, AppServerError> {
        if !self
            .skill_discovery_refresh
            .as_ref()
            .is_some_and(mini_agent_host::SkillDiscoveryRefresh::prompt_enabled)
        {
            return Ok(None);
        }
        self.skill_discovery
            .as_ref()
            .map(mini_agent_capabilities::Discovery::skill_context)
            .transpose()
            .map_err(AppServerError::SkillDiscovery)
    }

    pub(crate) fn execution_journal(
        &self,
        base_messages: &[Message],
    ) -> Option<mini_agent_capabilities::SessionExecutionJournal> {
        self.session
            .as_ref()
            .map(|opened| opened.store.execution_journal(base_messages))
    }

    pub(crate) fn execution_state(&self) -> Option<mini_agent_capabilities::SessionExecutionState> {
        self.session
            .as_ref()
            .and_then(|opened| opened.store.execution_state())
    }

    pub(crate) fn context_manifest_store(
        &self,
    ) -> Option<mini_agent_capabilities::SessionContextManifestStore> {
        self.session
            .as_ref()
            .map(|opened| opened.store.context_manifest_store())
    }

    pub(crate) fn event_replay_store(
        &self,
    ) -> Option<mini_agent_capabilities::SessionEventReplayStore> {
        self.session
            .as_ref()
            .map(|opened| opened.store.event_replay_store())
    }

    pub(crate) fn context_manifest(
        &self,
    ) -> Result<Vec<mini_agent_capabilities::SessionContextManifestEntry>, AppServerError> {
        self.context_manifest_store()
            .map_or_else(|| Ok(Vec::new()), |store| store.entries())
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn reserve_execution_resume(
        &self,
        turn_id: &mini_agent_protocol::TurnId,
        checkpoint_seq: u64,
        request_id: &str,
    ) -> Result<mini_agent_capabilities::ExecutionResumeReservation, AppServerError> {
        self.session
            .as_ref()
            .ok_or_else(|| {
                AppServerError::Checkpoint("session persistence is disabled".to_string())
            })?
            .store
            .reserve_execution_resume(turn_id, checkpoint_seq, request_id)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn reconcile_execution_tool_call(
        &self,
        request: mini_agent_capabilities::SessionReconciliationRequest,
    ) -> Result<mini_agent_capabilities::ReconciliationReservation, AppServerError> {
        if request.turn_id.as_str().is_empty() {
            return Err(AppServerError::Checkpoint(
                "reconciliation Turn id must not be empty".to_string(),
            ));
        }
        self.session
            .as_ref()
            .ok_or_else(|| {
                AppServerError::Checkpoint("session persistence is disabled".to_string())
            })?
            .store
            .reconcile_execution_tool_call(request)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn session_operation_for_turn(
        &self,
        turn_id: &str,
    ) -> Result<Option<mini_agent_capabilities::SessionOperation>, AppServerError> {
        match self.session.as_ref() {
            Some(opened) => opened
                .store
                .operation_for_turn(turn_id)
                .map_err(AppServerError::Checkpoint),
            None => Ok(None),
        }
    }

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

    pub(crate) fn read_notebook(&self, scope: &str) -> Result<serde_json::Value, AppServerError> {
        let session_dir = self
            .session
            .as_ref()
            .and_then(|opened| opened.store.path().parent())
            .ok_or_else(|| {
                AppServerError::Checkpoint("session persistence is disabled".to_string())
            })?;
        let snapshot = mini_agent_capabilities::read_notebook_scope(session_dir, scope)
            .map_err(AppServerError::Checkpoint)?;
        serde_json::to_value(snapshot)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
    }

    pub(crate) fn write_notebook(
        &mut self,
        key: &str,
        content: &str,
        append: bool,
        importance: &str,
        keywords: Option<Vec<String>>,
        evidence: Option<Vec<serde_json::Value>>,
    ) -> Result<serde_json::Value, AppServerError> {
        let session_dir = self
            .session
            .as_ref()
            .and_then(|opened| opened.store.path().parent())
            .ok_or_else(|| {
                AppServerError::Checkpoint("session persistence is disabled".to_string())
            })?
            .to_path_buf();
        let importance = mini_agent_capabilities::NotebookImportance::parse(
            (!importance.is_empty()).then_some(importance),
        )
        .map_err(AppServerError::Checkpoint)?;
        let snapshot = mini_agent_capabilities::upsert_notebook_with_metadata(
            &session_dir.join(mini_agent_capabilities::NOTEBOOK_FILE_NAME),
            key,
            content,
            append,
            importance,
            keywords,
            evidence,
        )
        .map_err(AppServerError::Checkpoint)?;
        serde_json::to_value(snapshot)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
    }

    pub(crate) fn forget_notebook(
        &mut self,
        key: &str,
    ) -> Result<serde_json::Value, AppServerError> {
        let session_dir = self
            .session
            .as_ref()
            .and_then(|opened| opened.store.path().parent())
            .ok_or_else(|| {
                AppServerError::Checkpoint("session persistence is disabled".to_string())
            })?
            .to_path_buf();
        let snapshot = mini_agent_capabilities::forget_notebook(
            &session_dir.join(mini_agent_capabilities::NOTEBOOK_FILE_NAME),
            key,
        )
        .map_err(AppServerError::Checkpoint)?;
        serde_json::to_value(snapshot)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
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

    pub(crate) fn persist_model_settings(
        &mut self,
        selection: Option<mini_agent_protocol::ModelSelection>,
        reasoning_selection: Option<ReasoningSelection>,
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session
            .store
            .set_model_settings(selection, reasoning_selection)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn session_items(&self) -> Option<&[SessionItem]> {
        self.session.as_ref().map(|opened| opened.store.items())
    }

    pub(crate) fn session_turn_source(&self, turn_id: &str) -> Option<TurnSource> {
        self.session
            .as_ref()
            .and_then(|opened| opened.store.turn_source(turn_id))
    }

    pub(crate) fn session_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<mini_agent_capabilities::SessionOperation>, AppServerError> {
        match self.session.as_ref() {
            Some(opened) => opened
                .store
                .operation(operation_id)
                .map_err(AppServerError::Checkpoint),
            None => Ok(None),
        }
    }

    pub(crate) fn session_is_forked(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|opened| opened.store.is_forked())
    }

    pub(crate) fn record_context(
        &mut self,
        checkpoint: &ThreadCheckpoint,
    ) -> Result<(), AppServerError> {
        let context = checkpoint
            .session
            .messages()
            .iter()
            .rev()
            .find(|message| matches!(message, Message::Context { .. }))
            .ok_or_else(|| {
                AppServerError::Checkpoint("no context item is available to persist".to_string())
            })?;
        self.record_context_message(context, checkpoint)
    }

    pub(crate) fn record_context_message(
        &mut self,
        context: &Message,
        checkpoint: &ThreadCheckpoint,
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            self.local_checkpoint_seq = self.local_checkpoint_seq.saturating_add(1);
            return Ok(());
        };
        session
            .store
            .record_context(context, checkpoint.session.messages())
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn record_operation(
        &mut self,
        operation: mini_agent_capabilities::SessionOperation,
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        session
            .store
            .record_operation(operation)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn child_task_action(
        &mut self,
        params: &mini_agent_app_server_protocol::ChildTaskParams,
    ) -> Result<mini_agent_app_server_protocol::ChildTaskResult, AppServerError> {
        let opened = self.session.as_mut().ok_or_else(|| {
            AppServerError::Checkpoint("session persistence is disabled".to_string())
        })?;
        if opened.store.thread_id() != params.thread_id.as_str() {
            return Err(AppServerError::ThreadNotFound(params.thread_id.clone()));
        }
        let context = mini_agent_capabilities::ChildTaskContext {
            parent_thread_id: params.parent_thread_id.clone(),
            operation_id: params.operation_id.clone(),
            attempt: params.attempt,
        };
        let control_source = match params
            .control_source
            .unwrap_or(mini_agent_app_server_protocol::ChildTaskControlSource::MainAgent)
        {
            mini_agent_app_server_protocol::ChildTaskControlSource::MainAgent => {
                mini_agent_capabilities::ChildTaskControlSource::MainAgent
            }
            mini_agent_app_server_protocol::ChildTaskControlSource::UserPanel => {
                mini_agent_capabilities::ChildTaskControlSource::UserPanel
            }
            mini_agent_app_server_protocol::ChildTaskControlSource::ParentFreeze => {
                mini_agent_capabilities::ChildTaskControlSource::ParentFreeze
            }
        };
        let result = match params.action {
            mini_agent_app_server_protocol::ChildTaskAction::Report => {
                opened.store.record_child_report(
                    &context,
                    required_child_param(params.report_id.as_deref(), "reportId")?,
                    required_child_param(params.report.as_deref(), "report")?,
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::UpdateQueued => {
                opened.store.update_queued_child_task(
                    &context,
                    required_child_param(params.prompt.as_deref(), "prompt")?.to_string(),
                    params.request_id.as_deref(),
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::CancelQueued => {
                opened.store.cancel_queued_child_task_from(
                    &context,
                    params.request_id.as_deref(),
                    control_source,
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::QueueFollowUp => {
                opened.store.queue_child_follow_up(
                    &context,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                    required_child_param(params.prompt.as_deref(), "prompt")?.to_string(),
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::Pause => {
                opened.store.pause_child_task_from(
                    &context,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                    required_child_param(params.turn_id.as_deref(), "turnId")?,
                    control_source,
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::CancelActive => {
                opened.store.cancel_active_child_task_from(
                    &context,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                    required_child_param(params.turn_id.as_deref(), "turnId")?,
                    control_source,
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::Retry => {
                opened.store.retry_child_task_from(
                    &context,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                    control_source,
                )
            }
            mini_agent_app_server_protocol::ChildTaskAction::Resume => {
                let request_id = required_child_param(params.request_id.as_deref(), "requestId")?;
                match params.turn_id.as_deref() {
                    Some(turn_id) => opened.store.resume_child_execution_from(
                        &context,
                        request_id,
                        turn_id,
                        control_source,
                    ),
                    None => {
                        opened
                            .store
                            .resume_child_task_from(&context, request_id, control_source)
                    }
                }
            }
            mini_agent_app_server_protocol::ChildTaskAction::StartFailure => {
                opened.store.record_child_start_failure(
                    &context,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                    required_child_param(params.error.as_deref(), "error")?,
                )
            }
        }
        .map_err(AppServerError::Checkpoint)?;
        Ok(mini_agent_app_server_protocol::ChildTaskResult {
            thread_id: params.thread_id.clone(),
            parent_thread_id: params.parent_thread_id.clone(),
            operation_id: params.operation_id.clone(),
            action: params.action,
            request_action: result.request_action.map(|action| match action {
                mini_agent_capabilities::ChildControlRequestAction::Steer => {
                    mini_agent_app_server_protocol::TurnSteerAction::Steer
                }
                mini_agent_capabilities::ChildControlRequestAction::QueueFollowUp => {
                    mini_agent_app_server_protocol::TurnSteerAction::QueueFollowUp
                }
            }),
            status: result.status,
            cursor: result.cursor,
            attempt: result.attempt,
            attempt_kind: result.attempt_kind,
            turn_id: result.turn_id,
            duplicate: result.duplicate,
            timestamp_ms: result.timestamp_ms,
        })
    }

    pub(crate) fn session_control_state_if_persisted(
        &self,
    ) -> Result<Option<mini_agent_capabilities::SessionControlState>, AppServerError> {
        let Some(opened) = self.session.as_ref() else {
            return Ok(None);
        };
        opened
            .store
            .session_control()
            .map(Some)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn session_control_action(
        &mut self,
        params: &mini_agent_app_server_protocol::SessionControlParams,
    ) -> Result<mini_agent_app_server_protocol::SessionControlResult, AppServerError> {
        let opened = self.session.as_mut().ok_or_else(|| {
            AppServerError::Checkpoint("session persistence is disabled".to_string())
        })?;
        if opened.store.thread_id() != params.thread_id.as_str() {
            return Err(AppServerError::ThreadNotFound(params.thread_id.clone()));
        }
        let state = match params.action {
            mini_agent_app_server_protocol::SessionControlAction::Read => {
                opened.store.session_control()
            }
            mini_agent_app_server_protocol::SessionControlAction::Freeze => {
                opened.store.transition_session_control(
                    mini_agent_capabilities::SessionControlAction::Freeze,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                )
            }
            mini_agent_app_server_protocol::SessionControlAction::FreezeSettled => {
                opened.store.transition_session_control(
                    mini_agent_capabilities::SessionControlAction::FreezeSettled,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                )
            }
            mini_agent_app_server_protocol::SessionControlAction::Resume => {
                opened.store.transition_session_control(
                    mini_agent_capabilities::SessionControlAction::Resume,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                )
            }
            mini_agent_app_server_protocol::SessionControlAction::ResumeSettled => {
                opened.store.transition_session_control(
                    mini_agent_capabilities::SessionControlAction::ResumeSettled,
                    required_child_param(params.request_id.as_deref(), "requestId")?,
                )
            }
        }
        .map_err(AppServerError::Checkpoint)?;
        Ok(mini_agent_app_server_protocol::SessionControlResult {
            thread_id: params.thread_id.clone(),
            session_id: state.session_id,
            status: match state.status {
                mini_agent_capabilities::SessionControlStatus::Running => {
                    mini_agent_app_server_protocol::SessionControlStatus::Running
                }
                mini_agent_capabilities::SessionControlStatus::Freezing => {
                    mini_agent_app_server_protocol::SessionControlStatus::Freezing
                }
                mini_agent_capabilities::SessionControlStatus::Frozen => {
                    mini_agent_app_server_protocol::SessionControlStatus::Frozen
                }
                mini_agent_capabilities::SessionControlStatus::Resuming => {
                    mini_agent_app_server_protocol::SessionControlStatus::Resuming
                }
            },
            request_id: state.request_id,
            updated_at_ms: state.updated_at_ms,
        })
    }

    pub(crate) fn child_steer_request(
        &mut self,
        thread_id: &ThreadId,
        request_id: &str,
        turn_id: &str,
        step: mini_agent_capabilities::ChildSteerRequestStep,
        accepted_status: Option<&str>,
    ) -> Result<Option<mini_agent_capabilities::ChildTaskMutationResult>, AppServerError> {
        let opened = self.session.as_mut().ok_or_else(|| {
            AppServerError::Checkpoint("session persistence is disabled".to_string())
        })?;
        if opened.store.thread_id() != thread_id.as_str() {
            return Err(AppServerError::ThreadNotFound(thread_id.clone()));
        }
        let Some(context) = opened
            .store
            .child_task_context()
            .map_err(AppServerError::Checkpoint)?
        else {
            return Ok(None);
        };
        opened
            .store
            .transition_child_steer_request(&context, request_id, turn_id, step, accepted_status)
            .map_err(AppServerError::Checkpoint)
    }

    pub(crate) fn record_turn(
        &mut self,
        started_at_ms: u64,
        prompt: &str,
        turn: TurnPersistence<'_>,
        checkpoint: &[Message],
    ) -> Result<(), AppServerError> {
        let Some(session) = self.session.as_mut() else {
            self.local_checkpoint_seq = self.local_checkpoint_seq.saturating_add(1);
            return Ok(());
        };
        let status = match turn.result.status {
            TurnStatus::Completed => SessionTurnStatus::Completed,
            TurnStatus::StepLimit => SessionTurnStatus::StepLimit,
            TurnStatus::Steered => SessionTurnStatus::Steered,
            TurnStatus::Cancelled => SessionTurnStatus::Cancelled,
            TurnStatus::Failed | TurnStatus::InProgress => SessionTurnStatus::Failed,
        };
        let commit = TurnCommit {
            started_at_ms,
            prompt,
            status,
            steps: turn.result.steps,
            error: turn.result.error.as_deref(),
            messages: turn.messages,
            tool_arguments: turn.tool_arguments,
            presentation: turn.presentation,
            checkpoint,
        };
        let result = if turn.execution_resume {
            session
                .store
                .record_resumed_turn_with_id(turn.result.turn_id.as_str(), commit)
        } else {
            session
                .store
                .record_turn_with_id(turn.result.turn_id.as_str(), commit)
        };
        result.map_err(AppServerError::Checkpoint)
    }
}
