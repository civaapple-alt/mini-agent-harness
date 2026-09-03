//! Concrete capability providers used to assemble a mini-agent runtime.
//!
//! This crate owns provider implementations and their local resources. Host
//! selects providers through bounded identifiers and remains responsible for
//! profile resolution and runtime orchestration.

mod blocking;
mod image;
mod mcp;
mod model;
mod openai;
mod path_policy;
mod persona;
mod registry;
mod result_store;
mod sandbox;
mod security;
mod session;
mod skills;
mod web;
mod workspace;

#[cfg(test)]
pub(crate) mod test_support;

// The implementation modules stay private. This root facade is the capability
// boundary used by Host, App Server, and embedding applications. Keep exports
// grouped by role so implementation details do not become accidental API.

// Stable capability contracts.
pub use persona::AgentPromptKind;
pub use persona::PersonaPromptKind;
pub use registry::CapabilityDescriptor;
pub use registry::CapabilityKind;
pub use registry::CapabilityRegistry;
pub use registry::ToolBuildRequest;
pub use registry::ToolProvider;
pub use result_store::ResultStore;
pub use result_store::StoredResult;
pub use sandbox::SandboxKind;
pub use security::ApprovalScope;
pub use security::ApprovalStore;
pub use security::SecurityDecision;
pub use security::SecurityPolicy;
pub use security::SecurityPreset;
pub use session::OpenedSession;
pub use session::SessionItem;
pub use session::SessionRequest;
pub use session::SessionStore;
pub use session::TurnCommit;
pub use session::TurnStatus;

// Host/App Server composition and embedding seams. These exports assemble
// concrete providers without exposing their internal wire or process logic.
pub use image::FileUploader;
pub use image::ImageStore;
pub use mcp::LoadResult as McpLoadResult;
pub use mcp::load as load_mcp;
pub use model::ModelProviderSettings;
pub use model::build_model;
pub use openai::OpenAiError;
pub use openai::OpenAiModel;
pub use path_policy::normalize_path;
pub use skills::Discovery;
pub use skills::McpServerConfig;
pub use skills::McpTransportConfig;
pub use skills::SkillActivation;
pub use skills::SkillDependency;
pub use skills::discover;
pub use workspace::ApprovalController;
pub use workspace::ApprovalMode;
pub use workspace::workspace_tools_with_read_roots_and_results;

/// Stable identifier for the built-in OpenAI-compatible model provider.
pub const OPENAI_MODEL_PROVIDER: &str = "openai";

/// Stable identifier for the built-in tool provider.
pub const BUILTIN_TOOL_PROVIDER: &str = "builtin";

/// Stable identifier for the built-in extension provider.
pub const BUILTIN_EXTENSION_PROVIDER: &str = "builtin";

/// Stable identifier for the built-in policy provider.
pub const BUILTIN_POLICY_PROVIDER: &str = "builtin";

fn into_tool_outcome(
    result: Result<String, mini_agent_protocol::ToolError>,
) -> mini_agent_protocol::ToolExecutionOutcome {
    result.map_or_else(
        |error| mini_agent_protocol::ToolExecutionOutcome::failed(error.to_string()),
        mini_agent_protocol::ToolExecutionOutcome::completed,
    )
}
