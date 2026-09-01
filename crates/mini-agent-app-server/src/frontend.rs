//! Stable frontend-facing adapters for the CLI and other local clients.
//!
//! Concrete providers remain implemented by Host and Capabilities, but
//! frontends import their launch, approval, observation, and presentation
//! contracts from this module. This keeps those implementation crates out of
//! the frontend's direct dependency graph.

pub use mini_agent_app_server_protocol::CapabilityManifest;
pub use mini_agent_app_server_protocol::CollaborationMode;
pub use mini_agent_app_server_protocol::CollaborationModeKind;
pub use mini_agent_app_server_protocol::ThreadSettingsUpdateResult;
pub use mini_agent_app_server_protocol::WorkflowGoalAdvanceParams;
pub use mini_agent_app_server_protocol::WorkflowGoalStatus;
pub use mini_agent_app_server_protocol::WorkflowState;
pub use mini_agent_app_server_protocol::WorkflowVerdictOutcome;
pub use mini_agent_app_server_protocol::WorkflowVerifierVerdict;
pub use mini_agent_capabilities::ApprovalController as CapabilityApprovalController;
pub use mini_agent_capabilities::ApprovalMode;
pub use mini_agent_capabilities::SandboxKind;
pub use mini_agent_capabilities::SecurityPolicy;
pub use mini_agent_capabilities::SecurityPreset;
pub use mini_agent_core::DEFAULT_MAX_PENDING_INPUTS;
pub use mini_agent_core::InputQueueError;
pub use mini_agent_core::RunControl;
pub use mini_agent_host::WorkflowScope;
pub use mini_agent_host::harness_config_auto;
pub use mini_agent_host::print_auto_warning;
pub use mini_agent_protocol::EventEnvelope;
pub use mini_agent_protocol::EventSink;
pub use mini_agent_protocol::Message;
pub use mini_agent_protocol::StopReason;
pub use mini_agent_protocol::ToolError;
pub use mini_agent_protocol::TurnInput;
pub use mini_agent_protocol::TurnInputMode;
pub use mini_agent_protocol::TurnStatus;

/// App Server owned profile selector used by local frontend startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProfile(mini_agent_host::RuntimeProfile);

impl RuntimeProfile {
    pub fn interactive_default() -> Self {
        Self(mini_agent_host::RuntimeProfile::interactive_default())
    }

    pub fn auto_default() -> Self {
        Self(mini_agent_host::RuntimeProfile::auto_default())
    }

    pub fn without_tools(self) -> Self {
        Self(self.0.without_tools())
    }

    pub fn with_sandbox(self, sandbox: SandboxKind) -> Self {
        Self(self.0.with_sandbox(sandbox))
    }

    pub fn with_security(self, security: SecurityPreset) -> Self {
        Self(self.0.with_security(security))
    }

    pub fn sandbox(&self) -> SandboxKind {
        self.0.sandbox
    }

    pub fn manifest(&self) -> CapabilityManifest {
        crate::capability_manifest_to_protocol(
            &self
                .0
                .manifest_with_config(&mini_agent_core::HarnessConfig::default()),
        )
    }

    pub(crate) fn into_host(self) -> mini_agent_host::RuntimeProfile {
        self.0
    }
}

/// Loads a bounded workspace profile through the App Server launch boundary.
pub fn load_workspace_profile(
    workspace: &std::path::Path,
    base: RuntimeProfile,
) -> Result<RuntimeProfile, String> {
    mini_agent_host::load_workspace_profile(workspace, base.into_host()).map(RuntimeProfile)
}

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
    use super::EventEnvelope;
    use super::EventSink;
    use serde_json::Value;

    /// App Server owned output observer used by local frontends.
    pub struct RunObserver(mini_agent_host::observer::RunObserver);

    #[derive(Clone, Copy)]
    pub enum ScriptFormat {
        Text,
        Json,
    }

    impl RunObserver {
        pub fn new() -> Self {
            Self(mini_agent_host::observer::RunObserver::new())
        }

        pub fn for_script(format: ScriptFormat) -> Self {
            let format = match format {
                ScriptFormat::Text => mini_agent_host::observer::ScriptFormat::Text,
                ScriptFormat::Json => mini_agent_host::observer::ScriptFormat::Json,
            };
            Self(mini_agent_host::observer::RunObserver::for_script(format))
        }

        pub fn finish(&mut self) {
            self.0.finish();
        }

        pub fn stats_json(&self) -> Value {
            self.0.stats_json()
        }

        pub fn tool_calls_json(&self) -> &[Value] {
            self.0.tool_calls_json()
        }

        pub fn assistant_displayed(&self) -> bool {
            self.0.assistant_displayed()
        }
    }

    impl Default for RunObserver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl EventSink for RunObserver {
        fn emit(&mut self, event: EventEnvelope) {
            self.0.emit(event);
        }
    }

    pub fn print_final_answer(text: &str) {
        mini_agent_host::observer::print_final_answer(text);
    }
}

pub mod skills {
    pub use mini_agent_capabilities::Discovery;
    pub use mini_agent_capabilities::discover;
}

/// Workflow commands and prompt shaping exposed to local frontends.
pub mod workflow {
    pub use mini_agent_host::PlanSlash;
    pub use mini_agent_host::VerdictOutcome;
    pub use mini_agent_host::VerifierVerdict;
    pub use mini_agent_host::goal_turn_prompt;
    pub use mini_agent_host::parse_plan_slash;
    pub use mini_agent_host::planning_turn_prompt;
    pub use mini_agent_host::with_plan_mode_overlay;
}
