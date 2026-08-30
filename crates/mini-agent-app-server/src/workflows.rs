//! Host workflow operations exposed through the App Server runtime.
//!
//! Goal and Plan persistence stays implemented by the Host workflow module,
//! while this service owns the session-directory binding used by frontends.

use mini_agent_host::HostWorkflowStore;
use std::io;
use std::path::PathBuf;

pub(crate) use mini_agent_host::GoalLimits;
pub(crate) use mini_agent_host::GoalState;
pub(crate) use mini_agent_host::GoalStatus;
pub(crate) use mini_agent_host::VerdictOutcome;
pub(crate) use mini_agent_host::VerifierVerdict;
pub(crate) use mini_agent_host::parse_verifier_verdict;

/// App Server bound workflow service for one durable session directory.
#[derive(Clone, Debug)]
pub struct WorkflowService {
    store: HostWorkflowStore,
}

impl WorkflowService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            store: HostWorkflowStore::new(session_dir, goal_limits),
        }
    }

    pub(crate) fn enable_plan_mode(&self, prompt: Option<&str>) -> io::Result<()> {
        self.store.init_plan_mode(prompt).map(|_| ())
    }

    pub(crate) fn disable_plan_mode(&self) -> io::Result<()> {
        self.store.disable_plan_mode()
    }

    pub(crate) fn plan_active(&self) -> bool {
        self.store.plan_active()
    }

    pub(crate) fn init_goal(&self, objective: &str) -> io::Result<GoalState> {
        self.store.init_goal(objective)
    }

    pub(crate) fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        self.store.load_goal_state()
    }

    pub(crate) fn verification_criteria(&self) -> io::Result<String> {
        self.store.verification_criteria()
    }

    pub(crate) fn record_verifier_verdict(
        &self,
        checkpoint_seq: u64,
        output: &str,
    ) -> io::Result<()> {
        self.store.record_verifier_verdict(checkpoint_seq, output)
    }

    pub(crate) fn advance_goal(&self, verdict: Option<VerifierVerdict>) -> io::Result<GoalState> {
        self.store.advance_goal(verdict)
    }

    pub(crate) fn pause_goal(&self) -> io::Result<()> {
        self.store.pause_goal()
    }

    pub(crate) fn fail_goal(&self) -> io::Result<GoalState> {
        self.store.fail_goal()
    }
}
