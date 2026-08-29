//! Concrete capability providers used to assemble a mini-agent runtime.
//!
//! This crate owns provider implementations and their local resources. Host
//! selects providers through bounded identifiers and remains responsible for
//! profile resolution and runtime orchestration.

mod image;
mod mcp;
mod model;
mod openai;
mod path_policy;
mod persona;
mod processes;
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

// The implementation modules stay private. This root facade is the stable
// capability boundary used by Host, App Server, and embedding applications.
pub use image::DeepSeekFiles;
pub use image::FileUploader;
pub use image::ImageStore;
pub use image::ProjectedImage;
pub use image::StoredImage;
pub use image::declared_media_type;
pub use image::detect_image;
pub use image::format_envelope;
pub use image::parse_envelope;
pub use image::project_images;
pub use image::uses_deepseek_files;
pub use image::vision_model_for;
pub use image::wire_image_block;
pub use mcp::LoadResult as McpLoadResult;
pub use mcp::load as load_mcp;
pub use model::ModelProviderSettings;
pub use model::build_model;
pub use openai::OpenAiError;
pub use openai::OpenAiModel;
pub use path_policy::goal_relative_rest;
pub use path_policy::is_plan_md_alias;
pub use path_policy::is_under_dir;
pub use path_policy::normalize_path;
pub use persona::AgentPromptKind;
pub use persona::PersonaPromptKind;
pub use registry::CapabilityDescriptor;
pub use registry::CapabilityKind;
pub use registry::CapabilityRegistry;
pub use registry::ToolBuildRequest;
pub use registry::ToolProvider;
pub use result_store::ReadToolResult;
pub use result_store::ResultStore;
pub use result_store::StoredResult;
pub use sandbox::ProcessSandbox;
pub use sandbox::SandboxKind;
pub use security::SecurityDecision;
pub use security::SecurityPolicy;
pub use security::SecurityPreset;
pub use session::OpenedSession;
pub use session::SessionRequest;
pub use session::SessionStore;
pub use session::TurnCommit;
pub use session::TurnStatus;
pub use session::timestamp_ms;
pub use skills::Discovery;
pub use skills::McpServerConfig;
pub use skills::McpTransportConfig;
pub use skills::discover;
pub use workspace::ApprovalController;
pub use workspace::ApprovalMode;
pub use workspace::CommandOutput;
pub use workspace::Workspace;
pub use workspace::run_sandboxed_command;
pub use workspace::shell_command;
pub use workspace::string_arg;
pub use workspace::workspace_tools_with_read_roots_and_results;

/// Stable identifier for the built-in OpenAI-compatible model provider.
pub const OPENAI_MODEL_PROVIDER: &str = "openai";

/// Stable identifier for the built-in tool provider.
pub const BUILTIN_TOOL_PROVIDER: &str = "builtin";

/// Stable identifier for the built-in extension provider.
pub const BUILTIN_EXTENSION_PROVIDER: &str = "builtin";

/// Stable identifier for the built-in policy provider.
pub const BUILTIN_POLICY_PROVIDER: &str = "builtin";
