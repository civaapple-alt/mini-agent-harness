//! Stable frontend-facing adapters for the CLI and other local clients.
//!
//! Concrete providers remain implemented by Host and Capabilities, but
//! frontends import their launch, approval, observation, and presentation
//! contracts from this module. This keeps those implementation crates out of
//! the frontend's direct dependency graph.

pub use mini_agent_capabilities::sandbox::SandboxKind;
pub use mini_agent_capabilities::security::SecurityPolicy;
pub use mini_agent_capabilities::security::SecurityPreset;
pub use mini_agent_capabilities::workspace::ApprovalController;
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

pub mod observer {
    pub use mini_agent_host::observer::RunObserver;
    pub use mini_agent_host::observer::ScriptFormat;
    pub use mini_agent_host::observer::print_final_answer;
}

pub mod skills {
    pub use mini_agent_capabilities::skills::Discovery;
    pub use mini_agent_capabilities::skills::discover;
}
