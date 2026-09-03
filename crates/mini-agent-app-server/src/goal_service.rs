//! Thread Goal request boundary and Goal startup inputs.

use crate::action::{ActionFailure, ActionResponse};
use crate::runtime_command::{RuntimeCommand, RuntimeCommandClient};
use mini_agent_app_server_protocol::ThreadGoalStatus;
pub(crate) use mini_agent_host::GoalLimits;
pub(crate) use mini_agent_host::GoalState;
use mini_agent_host::HostWorkflowStore;
use mini_agent_host::RuntimeConfig;
use std::io;
use std::path::PathBuf;

pub(crate) use mini_agent_host::VerifierVerdict;
pub(crate) use mini_agent_host::parse_verifier_verdict;

/// App Server Goal boundary for one Thread runtime.
#[derive(Clone)]
pub struct GoalService {
    store: Option<HostWorkflowStore>,
    client: Option<RuntimeCommandClient>,
    verifier_config: Option<RuntimeConfig>,
}

impl GoalService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            store: Some(HostWorkflowStore::new(session_dir, goal_limits)),
            client: None,
            verifier_config: None,
        }
    }

    /// Enables the separate tool-free provider used for automatic Goal
    /// verification and continuation.
    pub fn with_verifier_config(mut self, config: RuntimeConfig) -> Self {
        self.verifier_config = Some(config);
        self
    }

    pub(crate) fn verifier_config(&self) -> Option<RuntimeConfig> {
        self.verifier_config.clone()
    }

    pub(crate) fn into_store(mut self) -> io::Result<HostWorkflowStore> {
        self.store
            .take()
            .ok_or_else(|| io::Error::other("Goal store is already bound"))
    }

    pub(crate) fn bound(
        client: RuntimeCommandClient,
        verifier_config: Option<RuntimeConfig>,
    ) -> Self {
        Self {
            store: None,
            client: Some(client),
            verifier_config,
        }
    }

    pub(crate) async fn set_thread_goal_action(
        &self,
        objective: Option<String>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> Result<ActionResponse<GoalState>, ActionFailure> {
        let client = self.client()?;
        client
            .request_action(|reply| RuntimeCommand::ThreadGoalSet {
                objective,
                status,
                token_budget,
                reply,
            })
            .await
    }

    pub(crate) async fn get_thread_goal_action(
        &self,
    ) -> Result<ActionResponse<Option<GoalState>>, ActionFailure> {
        let client = self.client()?;
        client
            .request_action(|reply| RuntimeCommand::ThreadGoalGet { reply })
            .await
    }

    pub(crate) async fn clear_thread_goal_action(
        &self,
    ) -> Result<ActionResponse<bool>, ActionFailure> {
        let client = self.client()?;
        client
            .request_action(|reply| RuntimeCommand::ThreadGoalClear { reply })
            .await
    }

    fn client(&self) -> Result<&RuntimeCommandClient, ActionFailure> {
        self.client.as_ref().ok_or_else(|| {
            ActionFailure::without_receipt(crate::AppServerError::RuntimeUnavailable)
        })
    }
}
