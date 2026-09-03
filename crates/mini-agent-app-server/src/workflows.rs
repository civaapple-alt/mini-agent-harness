//! Host workflow operations exposed through the App Server runtime.
//!
//! Goal and Plan persistence stays implemented by the Host workflow module.
//! The store is moved into the App Server worker and this type only sends
//! workflow commands through that worker.

use crate::AppServerError;
use crate::action::ActionFailure;
use crate::action::ActionResponse;
use crate::action::ActionResult;
use crate::runtime_actor::RuntimeCommand;
use crate::runtime_actor::RuntimeRequest;
use crate::worker::Command;
use mini_agent_app_server_protocol::ThreadGoalStatus;
pub(crate) use mini_agent_host::GoalLimits;
pub(crate) use mini_agent_host::GoalState;
use mini_agent_host::HostWorkflowStore;
use mini_agent_host::RuntimeConfig;
pub(crate) use mini_agent_host::VerifierVerdict;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub(crate) use mini_agent_host::parse_verifier_verdict;

pub(crate) type WorkflowStateSnapshot = (bool, Option<GoalState>, Vec<String>);

/// App Server bound workflow service for one durable session directory.
#[derive(Clone)]
pub struct WorkflowService {
    store: Option<HostWorkflowStore>,
    commands: Option<mpsc::Sender<Command>>,
    revision: Option<Arc<AtomicU64>>,
    stable_system_prompt: Option<String>,
    verifier_config: Option<RuntimeConfig>,
}

impl WorkflowService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            store: Some(HostWorkflowStore::new(session_dir, goal_limits)),
            commands: None,
            revision: None,
            stable_system_prompt: None,
            verifier_config: None,
        }
    }

    /// Associates the prompt assembled by Host with this runtime so Plan Mode
    /// can switch the settled Thread without exposing raw prompt replacement
    /// through the App Server protocol.
    pub fn with_stable_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.stable_system_prompt = Some(prompt.into());
        self
    }

    /// Enables the separate tool-free provider used for automatic Goal
    /// verification and continuation.
    pub fn with_verifier_config(mut self, config: RuntimeConfig) -> Self {
        self.verifier_config = Some(config);
        self
    }

    pub(crate) fn stable_system_prompt(&self) -> Option<&str> {
        self.stable_system_prompt.as_deref()
    }

    pub(crate) fn verifier_config(&self) -> Option<RuntimeConfig> {
        self.verifier_config.clone()
    }

    pub(crate) fn into_store(mut self) -> io::Result<HostWorkflowStore> {
        self.store
            .take()
            .ok_or_else(|| io::Error::other("workflow store is already bound"))
    }

    pub(crate) fn bound(
        commands: mpsc::Sender<Command>,
        revision: Arc<AtomicU64>,
        stable_system_prompt: Option<String>,
        verifier_config: Option<RuntimeConfig>,
    ) -> Self {
        Self {
            store: None,
            commands: Some(commands),
            revision: Some(revision),
            stable_system_prompt,
            verifier_config,
        }
    }

    pub(crate) async fn state_action(
        &self,
    ) -> Result<ActionResponse<WorkflowStateSnapshot>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowState { reply })
            .await
    }

    pub(crate) async fn set_collaboration_mode_action(
        &self,
        active: bool,
        builtin_tools: Option<mini_agent_host::BuiltinToolSelection>,
    ) -> Result<ActionResponse<Vec<String>>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::SetCollaborationMode {
            active,
            builtin_tools,
            reply,
        })
        .await
    }

    pub(crate) async fn set_goal_action(
        &self,
        objective: Option<String>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> Result<ActionResponse<GoalState>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::GoalSet {
            objective,
            status,
            token_budget,
            reply,
        })
        .await
    }

    pub(crate) async fn get_goal_action(
        &self,
    ) -> Result<ActionResponse<Option<GoalState>>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::GoalGet { reply })
            .await
    }

    pub(crate) async fn clear_goal_action(&self) -> Result<ActionResponse<bool>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::GoalClear { reply })
            .await
    }

    async fn request_action<T, F>(&self, build: F) -> Result<ActionResponse<T>, ActionFailure>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> RuntimeCommand,
    {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| ActionFailure::without_receipt(AppServerError::RuntimeUnavailable))?;
        let revision = self
            .revision
            .as_ref()
            .ok_or_else(|| ActionFailure::without_receipt(AppServerError::RuntimeUnavailable))?;
        let (reply, response) = oneshot::channel();
        commands
            .send(Command::Runtime(RuntimeRequest {
                expected_revision: revision.load(Ordering::SeqCst).into(),
                command: build(reply),
            }))
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?;
        response
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?
    }
}
