//! Application-host capabilities and runtime composition for mini-agent.
//!
//! This crate owns concrete providers, tools, policy, extension discovery,
//! persistence, and product workflows. It deliberately does not own terminal
//! input or command-line dispatch; those belong to `mini-agent-cli`.

pub mod config;
pub mod env_file;
pub mod goal;
pub mod harness_builder;
pub mod image;
pub mod marketplaces;
pub mod mcp;
pub mod observer;
pub mod openai;
pub mod persona;
pub mod processes;
pub mod project_context;
pub mod result_store;
pub mod sandbox;
pub mod security;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tool_outcome;
pub mod web;
pub mod workspace;
pub mod world;

/// Build metadata used by host diagnostics and persisted status output.
pub fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("unknown")
}

pub use config::RuntimeConfig;
pub use harness_builder::HarnessBuild;
pub use harness_builder::HostRuntime;
pub use harness_builder::RuntimeBuilder;
pub use harness_builder::harness_config;
pub use harness_builder::harness_config_auto;
pub use harness_builder::prepare_openai_harness;
pub use harness_builder::print_auto_warning;
pub use image::ImageStore;
pub use observer::RunObserver;
pub use openai::OpenAiModel;
pub use sandbox::SandboxKind;
pub use security::SecurityPreset;
pub use session::OpenedSession;
pub use session::SessionRequest;
pub use session::SessionStore;
pub use session::TurnCommit;
pub use session::TurnStatus;
pub use workspace::ApprovalController;
pub use workspace::ApprovalMode;
pub use workspace::workspace_tools_with_read_roots_and_results;
pub use world::WorldState;
