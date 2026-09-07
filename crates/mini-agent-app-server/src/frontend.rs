//! Stable frontend-facing adapters for local clients.
//!
//! Concrete providers remain implemented by Host and Capabilities, but
//! frontends import launch, approval, and protocol contracts from this module.
//! Terminal presentation remains an edge concern of the experimental CLI.

pub use mini_agent_app_server_protocol::CapabilityManifest;
pub use mini_agent_app_server_protocol::CollaborationMode;
pub use mini_agent_app_server_protocol::CollaborationModeKind;
pub use mini_agent_app_server_protocol::ThreadSettingsUpdateResult;
pub use mini_agent_capabilities::ApprovalController;
pub use mini_agent_capabilities::ApprovalController as CapabilityApprovalController;
pub use mini_agent_capabilities::ApprovalPolicy;
pub use mini_agent_capabilities::SandboxKind;
pub use mini_agent_capabilities::SecurityPolicy;
pub use mini_agent_capabilities::SecurityPreset;
pub use mini_agent_core::DEFAULT_MAX_PENDING_INPUTS;
pub use mini_agent_core::InputQueueError;
pub use mini_agent_core::RunControl;
pub use mini_agent_protocol::EventEnvelope;
pub use mini_agent_protocol::EventSink;
pub use mini_agent_protocol::Message;
pub use mini_agent_protocol::StopReason;
pub use mini_agent_protocol::ToolError;
pub use mini_agent_protocol::TurnInput;
pub use mini_agent_protocol::TurnInputMode;
pub use mini_agent_protocol::TurnStatus;

pub mod skills {
    pub use mini_agent_capabilities::Discovery;
    pub use mini_agent_capabilities::discover;
}

/// Workflow commands and prompt shaping exposed to local frontends.
pub mod workflow {
    pub use mini_agent_host::VerdictOutcome;
    pub use mini_agent_host::VerifierVerdict;
    pub use mini_agent_host::goal_turn_prompt;
    pub use mini_agent_host::with_plan_mode_overlay;
}
