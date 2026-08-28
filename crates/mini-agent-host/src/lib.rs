//! Application-host capabilities and runtime composition for mini-agent.
//!
//! This crate owns concrete providers, tools, policy, extension discovery,
//! persistence, and product workflows. It deliberately does not own terminal
//! input or command-line dispatch; those belong to `mini-agent-cli`.

pub mod config;
pub mod env_file;
pub mod goal;
pub mod harness_builder;
pub mod mcp;
pub mod observer;
pub mod persona;
pub mod processes;
pub mod profile;
pub mod project_context;
pub mod result_store;
pub mod runtime_factory;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tool_outcome;
pub mod web;
pub mod workspace;
pub mod world;

/// Concrete model and image providers live in `mini-agent-capabilities`.
/// These re-exports preserve the current Host API while callers migrate to
/// profile-selected provider construction.
pub use mini_agent_capabilities::image;
pub use mini_agent_capabilities::marketplaces;
pub use mini_agent_capabilities::openai;
pub use mini_agent_capabilities::sandbox;
pub use mini_agent_capabilities::security;

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
pub use harness_builder::prepare_openai_harness_with_profile;
pub use harness_builder::prepare_openai_harness_with_profile_and_result_store;
pub use harness_builder::print_auto_warning;
pub use mini_agent_capabilities::ImageStore;
pub use mini_agent_capabilities::OpenAiError;
pub use mini_agent_capabilities::OpenAiModel;
pub use observer::RunObserver;
pub use profile::AgentKind;
pub use profile::CapabilityManifest;
pub use profile::ContextLimits;
pub use profile::ExtensionLoadDepth;
pub use profile::ExtensionSelection;
pub use profile::PersonaKind;
pub use profile::PromptRulePolicy;
pub use profile::PromptSources;
pub use profile::RegularAgentConfig;
pub use profile::RulePolicy;
pub use profile::RuleSourceState;
pub use profile::RuleSourceStatus;
pub use profile::RuleSources;
pub use profile::RuntimeProfile;
pub use profile::SourceFingerprint;
pub use profile::ToolScope;
pub use profile::WorkflowScope;
pub use profile::load_workspace_profile;
pub use runtime_factory::HostRuntimeFactory;
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
