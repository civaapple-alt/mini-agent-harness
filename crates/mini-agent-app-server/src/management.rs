//! Runtime management operations shared by local and JSON-RPC clients.

use crate::AppServer;
use crate::McpRetryResult;
use crate::RuntimeSessionInfo;
use crate::ThreadUpdate;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_capabilities::McpLoadResult;
use mini_agent_capabilities::McpServerConfig;
use mini_agent_capabilities::OpenedSession;
use mini_agent_capabilities::TurnCommit;
use mini_agent_capabilities::TurnStatus as SessionTurnStatus;
use mini_agent_capabilities::load_mcp;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_host::WorldState;
use mini_agent_host::tool_outcome::classify_tools;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnStatus;
use std::sync::Arc;
use std::sync::Mutex;

pub struct RuntimeManagementService<M> {
    server: AppServer<M>,
    state: Arc<Mutex<RuntimeManagementState>>,
}

impl<M> Clone for RuntimeManagementService<M> {
    fn clone(&self) -> Self {
        Self {
            server: self.server.clone(),
            state: self.state.clone(),
        }
    }
}

struct RuntimeManagementState {
    session: Option<OpenedSession>,
    thread_id: ThreadId,
    world: WorldState,
    enabled_mcp_servers: Vec<String>,
    mcp_tool_count: usize,
    retry_mcp_servers: Vec<McpServerConfig>,
    approval: ApprovalController,
}

impl<M: Model + Send + 'static> RuntimeManagementService<M> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server: AppServer<M>,
        session: Option<OpenedSession>,
        thread_id: ThreadId,
        world: WorldState,
        enabled_mcp_servers: Vec<String>,
        mcp_tool_count: usize,
        retry_mcp_servers: Vec<McpServerConfig>,
        approval: ApprovalController,
    ) -> Self {
        Self {
            server,
            state: Arc::new(Mutex::new(RuntimeManagementState {
                session,
                thread_id,
                world,
                enabled_mcp_servers,
                mcp_tool_count,
                retry_mcp_servers,
                approval,
            })),
        }
    }

    pub fn session_info(&self) -> Option<RuntimeSessionInfo> {
        let state = self.state.lock().unwrap();
        state.session.as_ref().map(|opened| RuntimeSessionInfo {
            session_id: opened.store.session_id().to_string(),
            thread_id: opened.store.thread_id().to_string(),
            path: opened.store.path().display().to_string(),
            resumed: opened.resumed,
        })
    }

    pub fn checkpoint_seq(&self) -> Option<u64> {
        self.state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(|opened| opened.store.checkpoint_seq())
    }

    pub fn thread_id(&self) -> ThreadId {
        self.state.lock().unwrap().thread_id.clone()
    }

    pub fn world(&self) -> WorldState {
        self.state.lock().unwrap().world.clone()
    }

    pub fn enabled_mcp_servers(&self) -> Vec<String> {
        self.state.lock().unwrap().enabled_mcp_servers.clone()
    }

    pub fn mcp_tool_count(&self) -> usize {
        self.state.lock().unwrap().mcp_tool_count
    }

    pub fn retry_mcp_servers(&self) -> Vec<McpServerConfig> {
        self.state.lock().unwrap().retry_mcp_servers.clone()
    }

    pub async fn refresh_world(&self) -> Result<bool, String> {
        let world = self.world();
        let refreshed = WorldState::detect(
            world.workspace(),
            world.approval(),
            world.copilot(),
            world.sandbox(),
        );
        self.update_world(refreshed).await
    }

    pub async fn set_execution(
        &self,
        approval: ApprovalMode,
        copilot: bool,
    ) -> Result<bool, String> {
        let world = self.world();
        self.update_world(world.with_execution(approval, copilot, world.sandbox()))
            .await
    }

    pub async fn update_world(&self, updated: WorldState) -> Result<bool, String> {
        if updated == self.world() {
            return Ok(false);
        }
        let context = updated.model_context()?;
        self.update_thread(ThreadUpdate::AppendContext(context))
            .await?;
        let checkpoint = self.read_checkpoint().await?;
        self.record_context(&checkpoint)?;
        self.state.lock().unwrap().world = updated;
        Ok(true)
    }

    pub fn mcp_status(&self) -> (Vec<String>, Vec<String>, usize) {
        let state = self.state.lock().unwrap();
        let inactive = state
            .retry_mcp_servers
            .iter()
            .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
            .collect();
        (
            state.enabled_mcp_servers.clone(),
            inactive,
            state.mcp_tool_count,
        )
    }

    pub async fn retry_mcp(&self) -> Result<McpRetryResult, String> {
        let servers = self.retry_mcp_servers();
        if servers.is_empty() {
            return Ok(McpRetryResult {
                enabled_servers: Vec::new(),
                inactive_servers: Vec::new(),
                diagnostics: Vec::new(),
                tool_count: 0,
            });
        }
        let approval = self.state.lock().unwrap().approval.clone();
        let McpLoadResult {
            tools,
            loaded_servers,
            diagnostics,
        } = load_mcp(&servers, approval);
        let inactive_servers = servers
            .iter()
            .filter(|server| {
                !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
            })
            .map(|server| format!("{}/{}", server.plugin_name, server.server_name))
            .collect::<Vec<_>>();
        let enabled_servers = loaded_servers.iter().cloned().collect::<Vec<_>>();
        let tool_count = tools.len();
        self.update_thread(ThreadUpdate::ExtendTools(classify_tools(tools)))
            .await?;
        let mut state = self.state.lock().unwrap();
        state.retry_mcp_servers.retain(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        });
        state
            .enabled_mcp_servers
            .extend(enabled_servers.iter().cloned());
        state.mcp_tool_count += tool_count;
        Ok(McpRetryResult {
            enabled_servers,
            inactive_servers,
            diagnostics,
            tool_count,
        })
    }

    pub async fn update_thread(&self, update: ThreadUpdate) -> Result<(), String> {
        let thread_id = self.thread_id();
        self.server
            .thread_update_for(thread_id, update)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn read_checkpoint(&self) -> Result<ThreadCheckpoint, String> {
        self.server
            .thread_read_for(self.thread_id())
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn start_new_thread(&self) -> Result<(), String> {
        let (old_thread_id, new_thread_id) = {
            let mut state = self.state.lock().unwrap();
            let old_thread_id = state.thread_id.clone();
            let Some(session) = state.session.as_mut() else {
                return Err("session persistence is disabled".to_string());
            };
            session.store.start_thread()?;
            (
                old_thread_id,
                ThreadId::new(session.store.thread_id().to_string()),
            )
        };
        self.server
            .thread_reset(old_thread_id, new_thread_id.clone(), 1)
            .await
            .map_err(|error| error.to_string())?;
        self.state.lock().unwrap().thread_id = new_thread_id;
        Ok(())
    }

    pub fn record_context(&self, checkpoint: &ThreadCheckpoint) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let Some(session) = state.session.as_mut() else {
            return Ok(());
        };
        let context = checkpoint
            .session
            .messages()
            .iter()
            .rev()
            .find(|message| matches!(message, mini_agent_protocol::Message::Context { .. }))
            .ok_or_else(|| "no context item is available to persist".to_string())?;
        session
            .store
            .record_context(context, checkpoint.session.messages())
    }

    pub fn record_turn(
        &self,
        started_at_ms: u64,
        prompt: &str,
        result: &crate::runtime::RuntimeTurnResult,
    ) -> Result<(), String> {
        self.record_turn_with_messages(
            started_at_ms,
            prompt,
            result,
            &result.messages,
            &result.messages,
        )
    }

    pub fn record_turn_with_messages(
        &self,
        started_at_ms: u64,
        prompt: &str,
        result: &crate::runtime::RuntimeTurnResult,
        messages: &[mini_agent_protocol::Message],
        checkpoint: &[mini_agent_protocol::Message],
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let Some(session) = state.session.as_mut() else {
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
}
