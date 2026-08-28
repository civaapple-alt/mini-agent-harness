//! Concrete capability providers used to assemble a mini-agent runtime.
//!
//! This crate owns provider implementations and their local resources. Host
//! selects providers through bounded identifiers and remains responsible for
//! profile resolution and runtime orchestration.

pub mod image;
pub mod marketplaces;
pub mod mcp;
pub mod model;
pub mod openai;
pub mod path_policy;
pub mod persona;
pub mod processes;
pub mod registry;
pub mod result_store;
pub mod sandbox;
pub mod security;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod web;
pub mod workspace;

pub use image::DeepSeekFiles;
pub use image::FileUploader;
pub use image::ImageStore;
pub use image::ProjectedImage;
pub use image::StoredImage;
pub use mcp::LoadResult as McpLoadResult;
pub use model::ModelProviderSettings;
pub use model::build_model;
pub use openai::OpenAiError;
pub use openai::OpenAiModel;
pub use registry::CapabilityDescriptor;
pub use registry::CapabilityKind;
pub use registry::CapabilityRegistry;
pub use sandbox::SandboxKind;
pub use security::SecurityPreset;
pub use session::OpenedSession;
pub use session::SessionRequest;
pub use session::SessionStore;
pub use session::TurnCommit;
pub use session::TurnStatus;
pub use skills::McpServerConfig;

/// Stable identifier for the built-in OpenAI-compatible model provider.
pub const OPENAI_MODEL_PROVIDER: &str = "openai";

/// Lists the concrete model providers currently available to a host registry.
pub fn model_provider_ids() -> &'static [&'static str] {
    &[OPENAI_MODEL_PROVIDER]
}
