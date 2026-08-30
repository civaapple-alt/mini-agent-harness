//! Host workflow operations exposed through the App Server runtime.
//!
//! Goal and Plan persistence stays implemented by the Host workflow module.
//! The store is moved into the App Server worker and this type only sends
//! workflow commands through that worker.

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
}

impl WorkflowService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            store: Some(HostWorkflowStore::new(session_dir, goal_limits)),
            commands: None,
            revision: None,
        }
    }

    pub(crate) fn into_store(mut self) -> io::Result<HostWorkflowStore> {
        self.store
            .take()
            .ok_or_else(|| io::Error::other("workflow store is already bound"))
    }

    pub(crate) fn bound(commands: mpsc::Sender<Command>, revision: Arc<AtomicU64>) -> Self {
        Self {
            store: None,
            commands: Some(commands),
            revision: Some(revision),
        }
    }

    pub(crate) async fn state(&self) -> io::Result<(bool, Option<GoalState>)> {
        self.request(|reply| RuntimeCommand::WorkflowState { reply })
            .await
    }

    pub(crate) async fn enable_plan_mode(&self, prompt: Option<&str>) -> io::Result<()> {
        self.request(|reply| RuntimeCommand::WorkflowSetPlan {
            active: true,
            prompt: prompt.map(str::to_string),
            reply,
        })
        .await
    }

    pub(crate) async fn disable_plan_mode(&self) -> io::Result<()> {
        self.request(|reply| RuntimeCommand::WorkflowSetPlan {
            active: false,
            prompt: None,
            reply,
        })
        .await
    }

    pub(crate) async fn init_goal(&self, objective: &str) -> io::Result<GoalState> {
        let objective = objective.to_string();
        self.request(|reply| RuntimeCommand::WorkflowInitGoal { objective, reply })
            .await
    }

    pub(crate) async fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        self.request(|reply| RuntimeCommand::WorkflowLoadGoal { reply })
            .await
    }

    pub(crate) async fn verification_criteria(&self) -> io::Result<String> {
        self.request(|reply| RuntimeCommand::WorkflowCriteria { reply })
            .await
    }

    pub(crate) async fn record_verifier_verdict(
        &self,
        checkpoint_seq: u64,
        output: &str,
    ) -> io::Result<()> {
        let output = output.to_string();
        self.request(|reply| RuntimeCommand::WorkflowRecordVerdict {
            checkpoint_seq,
            output,
            reply,
        })
        .await
    }

    pub(crate) async fn advance_goal(
        &self,
        verdict: Option<VerifierVerdict>,
    ) -> io::Result<GoalState> {
        self.request(|reply| RuntimeCommand::WorkflowAdvance { verdict, reply })
            .await
    }

    pub(crate) async fn pause_goal(&self) -> io::Result<()> {
        self.request(|reply| RuntimeCommand::WorkflowPause { reply })
            .await
    }

    pub(crate) async fn fail_goal(&self) -> io::Result<GoalState> {
        self.request(|reply| RuntimeCommand::WorkflowFail { reply })
            .await
    }

    async fn request<T, F>(&self, build: F) -> io::Result<T>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> RuntimeCommand,
    {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| io::Error::other("workflow service is not bound"))?;
        let revision = self
            .revision
            .as_ref()
            .ok_or_else(|| io::Error::other("workflow service is not bound"))?;
        let (reply, response) = oneshot::channel();
        commands
            .send(Command::Runtime(RuntimeRequest {
                expected_revision: revision.load(Ordering::SeqCst).into(),
                command: build(reply),
            }))
            .await
            .map_err(|_| io::Error::other("runtime actor is unavailable"))?;
        response
            .await
            .map_err(|_| io::Error::other("runtime actor dropped the response"))?
            .map(ActionResponse::into_value)
            .map_err(|error| io::Error::other(error.to_string()))
    }
}
