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
pub(crate) use mini_agent_host::GoalLimits;
pub(crate) use mini_agent_host::GoalState;
pub(crate) use mini_agent_host::GoalStatus;
use mini_agent_host::HostWorkflowStore;
pub(crate) use mini_agent_host::VerdictOutcome;
pub(crate) use mini_agent_host::VerifierVerdict;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub(crate) use mini_agent_host::parse_verifier_verdict;

/// App Server bound workflow service for one durable session directory.
#[derive(Clone)]
pub struct WorkflowService {
    store: Option<HostWorkflowStore>,
    commands: Option<mpsc::Sender<Command>>,
    revision: Option<Arc<AtomicU64>>,
    stable_system_prompt: Option<String>,
}

impl WorkflowService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            store: Some(HostWorkflowStore::new(session_dir, goal_limits)),
            commands: None,
            revision: None,
            stable_system_prompt: None,
        }
    }

    /// Associates the prompt assembled by Host with this runtime so Plan Mode
    /// can switch the settled Thread without exposing raw prompt replacement
    /// through the App Server protocol.
    pub fn with_stable_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.stable_system_prompt = Some(prompt.into());
        self
    }

    pub(crate) fn stable_system_prompt(&self) -> Option<&str> {
        self.stable_system_prompt.as_deref()
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
    ) -> Self {
        Self {
            store: None,
            commands: Some(commands),
            revision: Some(revision),
            stable_system_prompt,
        }
    }

    pub(crate) async fn state_action(
        &self,
    ) -> Result<ActionResponse<(bool, Option<GoalState>)>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowState { reply })
            .await
    }

    pub(crate) async fn set_collaboration_mode_action(
        &self,
        active: bool,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::SetCollaborationMode { active, reply })
            .await
    }

    pub(crate) async fn init_goal_action(
        &self,
        objective: &str,
    ) -> Result<ActionResponse<GoalState>, ActionFailure> {
        let objective = objective.to_string();
        self.request_action(|reply| RuntimeCommand::WorkflowInitGoal { objective, reply })
            .await
    }

    pub(crate) async fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        self.request(|reply| RuntimeCommand::WorkflowLoadGoal { reply })
            .await
    }

    pub(crate) async fn verification_criteria_action(
        &self,
    ) -> Result<ActionResponse<String>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowCriteria { reply })
            .await
    }

    pub(crate) async fn record_verifier_verdict_action(
        &self,
        checkpoint_seq: u64,
        output: &str,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        let output = output.to_string();
        self.request_action(|reply| RuntimeCommand::WorkflowRecordVerdict {
            checkpoint_seq,
            output,
            reply,
        })
        .await
    }

    pub(crate) async fn advance_goal_action(
        &self,
        verdict: Option<VerifierVerdict>,
    ) -> Result<ActionResponse<GoalState>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowAdvance { verdict, reply })
            .await
    }

    pub(crate) async fn pause_goal_action(&self) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowPause { reply })
            .await
    }

    pub(crate) async fn fail_goal_action(
        &self,
    ) -> Result<ActionResponse<GoalState>, ActionFailure> {
        self.request_action(|reply| RuntimeCommand::WorkflowFail { reply })
            .await
    }

    async fn request<T, F>(&self, build: F) -> io::Result<T>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> RuntimeCommand,
    {
        self.request_action(build)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
            .map_err(|error| io::Error::other(error.to_string()))
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
