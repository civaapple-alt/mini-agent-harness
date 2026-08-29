//! Stable frontend-facing adapters for the CLI and other local clients.
//!
//! Concrete providers remain implemented by Host and Capabilities, but
//! frontends import their launch, approval, observation, and presentation
//! contracts from this module. This keeps those implementation crates out of
//! the frontend's direct dependency graph.

pub use mini_agent_app_server_protocol::WorkflowGoalAdvanceParams;
pub use mini_agent_app_server_protocol::WorkflowGoalStatus;
pub use mini_agent_app_server_protocol::WorkflowState;
pub use mini_agent_app_server_protocol::WorkflowVerdictOutcome;
pub use mini_agent_app_server_protocol::WorkflowVerifierVerdict;
pub use mini_agent_capabilities::sandbox::SandboxKind;
pub use mini_agent_capabilities::security::SecurityPolicy;
pub use mini_agent_capabilities::security::SecurityPreset;
use mini_agent_capabilities::workspace::ApprovalController as CapabilityApprovalController;
pub use mini_agent_capabilities::workspace::ApprovalMode;
pub use mini_agent_core::DEFAULT_MAX_PENDING_INPUTS;
pub use mini_agent_core::EventEnvelope;
pub use mini_agent_core::EventSink;
pub use mini_agent_core::InputQueueError;
pub use mini_agent_core::RunControl;
pub use mini_agent_core::StopReason;
pub use mini_agent_core::ToolError;
pub use mini_agent_core::TurnInput;
pub use mini_agent_core::TurnInputMode;
pub use mini_agent_core::TurnStatus;
pub use mini_agent_host::RuntimeConfig;
pub use mini_agent_host::RuntimeProfile;
pub use mini_agent_host::WorkflowScope;
pub use mini_agent_host::harness_config;
pub use mini_agent_host::harness_config_auto;
pub use mini_agent_host::load_workspace_profile;
pub use mini_agent_host::print_auto_warning;

/// App Server owned frontend handle for approval policy and interaction.
///
/// The underlying capability controller remains private to App Server and
/// Host composition. Frontends can configure and inspect approval state, but
/// cannot depend on the capability crate's concrete type in their API.
#[derive(Clone)]
pub struct ApprovalController(CapabilityApprovalController);

impl ApprovalController {
    pub fn new(mode: ApprovalMode) -> Self {
        Self(CapabilityApprovalController::new(mode))
    }

    pub fn with_preset(mode: ApprovalMode, preset: SecurityPreset) -> Self {
        Self(CapabilityApprovalController::with_preset(mode, preset))
    }

    pub fn with_policy_and_callback(
        mode: ApprovalMode,
        policy: SecurityPolicy,
        callback: impl Fn(&str) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self(CapabilityApprovalController::with_policy_and_callback(
            mode, policy, callback,
        ))
    }

    pub(crate) fn into_capability(self) -> CapabilityApprovalController {
        self.0
    }

    pub fn mode(&self) -> ApprovalMode {
        self.0.mode()
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        self.0.set_mode(mode);
    }

    pub fn set_living_plan(&self, path: Option<std::path::PathBuf>) {
        self.0.set_living_plan(path);
    }

    pub fn set_goal_dir(&self, path: Option<std::path::PathBuf>) {
        self.0.set_goal_dir(path);
    }
}

pub mod observer {
    pub use mini_agent_host::observer::RunObserver;
    pub use mini_agent_host::observer::ScriptFormat;
    pub use mini_agent_host::observer::print_final_answer;
}

pub mod skills {
    pub use mini_agent_capabilities::skills::Discovery;
    pub use mini_agent_capabilities::skills::discover;
}
