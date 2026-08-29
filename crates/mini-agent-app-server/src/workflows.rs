//! Host workflow operations exposed through the App Server runtime.
//!
//! Goal and Plan persistence stays implemented by the Host workflow module,
//! while this service owns the session-directory binding used by frontends.

use mini_agent_host::goal;
use mini_agent_host::goal::GoalLimits;
use std::io;
use std::path::Path;
use std::path::PathBuf;

pub use goal::GoalState;
pub use goal::GoalStatus;
pub use goal::PlanModeState;
pub use goal::PlanSlash;
pub use goal::VerdictOutcome;
pub use goal::VerifierVerdict;
pub use goal::goal_turn_prompt;
pub use goal::parse_plan_slash;
pub use goal::planning_turn_prompt;
pub use goal::with_plan_mode_overlay;

/// App Server bound workflow service for one durable session directory.
#[derive(Clone, Debug)]
pub struct WorkflowService {
    session_dir: PathBuf,
    goal_limits: GoalLimits,
}

impl WorkflowService {
    pub fn new(session_dir: impl Into<PathBuf>, goal_limits: GoalLimits) -> Self {
        Self {
            session_dir: session_dir.into(),
            goal_limits,
        }
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn goal_limits(&self) -> GoalLimits {
        self.goal_limits
    }

    pub fn init_plan_mode(&self, prompt: Option<&str>) -> io::Result<PathBuf> {
        goal::init_plan_mode_with_prompt(&self.session_dir, prompt)
    }

    pub fn disable_plan_mode(&self) -> io::Result<()> {
        goal::disable_plan_mode(&self.session_dir)
    }

    pub fn plan_active(&self) -> bool {
        goal::is_plan_mode_active(&self.session_dir)
    }

    pub fn init_goal(&self, objective: &str) -> io::Result<GoalState> {
        goal::init_goal_workspace_with_limits(&self.session_dir, objective, self.goal_limits)
    }

    pub fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        goal::load_goal_state(&self.session_dir)
    }

    pub fn verification_criteria(&self) -> io::Result<String> {
        goal::goal_verification_criteria(&self.session_dir)
    }

    pub fn record_verifier_verdict(&self, checkpoint_seq: u64, output: &str) -> io::Result<()> {
        goal::record_verifier_verdict(&self.session_dir, checkpoint_seq, output)
    }

    pub fn advance_goal(&self, verdict: Option<VerifierVerdict>) -> io::Result<GoalState> {
        goal::advance_goal_milestone(&self.session_dir, verdict)
    }

    pub fn pause_goal(&self) -> io::Result<()> {
        goal::pause_goal(&self.session_dir)
    }

    pub fn fail_goal(&self) -> io::Result<GoalState> {
        goal::fail_goal(&self.session_dir)
    }
}
