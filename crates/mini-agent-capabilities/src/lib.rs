//! Concrete capability providers used to assemble a mini-agent runtime.
//!
//! This crate owns provider implementations and their local resources. Host
//! selects providers through bounded identifiers and remains responsible for
//! runtime composition and orchestration.

mod blocking;
mod child_tasks;
mod image;
mod mcp;
mod model;
mod notebook;
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
pub use security::ActionGrantScope;
pub use security::ApprovalStore;
pub use security::SecurityDecision;
pub use security::SecurityPolicy;
pub use security::SecurityPreset;
pub use security::action_grant_key;
pub use session::OpenedSession;
pub use session::SessionForkConflict;
pub use session::SessionForkError;
pub use session::SessionForkInfo;
pub use session::SessionForkMetadata;
pub use session::SessionItem;
pub use session::SessionOperation;
pub use session::SessionRequest;
pub use session::SessionStore;
pub use session::THREAD_SETTINGS_FILE_NAME;
pub use session::TurnCommit;
pub use session::TurnStatus;
pub use session::resolve_session_file;

// Host/App Server composition and embedding seams. These exports assemble
// concrete providers without exposing their internal wire or process logic.
pub use child_tasks::child_task_tools;
pub use image::FileUploader;
pub use image::ImageStore;
pub use mcp::LoadResult as McpLoadResult;
pub use mcp::load as load_mcp;
pub use mini_agent_protocol::ApprovalPolicy;
pub use model::ModelProviderSettings;
pub use model::build_model;
pub use notebook::{
    MAX_NOTEBOOK_BYTES, MAX_NOTEBOOK_ENTRIES, NOTEBOOK_FILE_NAME, NotebookEntry,
    NotebookImportance, NotebookSnapshot, forget_notebook, notebook_tools, read_notebook,
    read_notebook_scope, upsert_notebook, upsert_notebook_with_importance,
};
pub use openai::OpenAiError;
pub use openai::OpenAiModel;
pub use path_policy::normalize_path;
pub use skills::Discovery;
pub use skills::LoadedSkill;
pub use skills::MAX_ACTIVATED_SKILL_BYTES;
pub use skills::MAX_SELECTED_SKILLS;
pub use skills::McpServerConfig;
pub use skills::McpTransportConfig;
pub use skills::SkillActivation;
pub use skills::SkillCatalogEntry;
pub use skills::SkillDependency;
pub use skills::SkillPathRecord;
pub use skills::builtin_skill_root;
pub use skills::discover;
pub use skills::discover_with_builtin_groups;
pub use workspace::ApprovalController;
pub use workspace::ApprovalFailure;
pub use workspace::MAX_SKILL_READ_BYTES;
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
