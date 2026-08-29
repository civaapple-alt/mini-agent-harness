//! Application-host profile resolution and runtime composition for mini-agent.
//!
//! Concrete model, policy, marketplace, and persistence providers live in
//! `mini-agent-capabilities`. This crate owns profile resolution, runtime
//! composition, and product workflows. It deliberately does not own terminal
//! input or command-line dispatch; those belong to `mini-agent-cli`.

pub mod config;
pub mod env_file;
pub mod goal;
pub mod harness_builder;
pub mod observer;
pub mod profile;
pub mod project_context;
pub mod runtime_factory;
pub mod tool_outcome;
pub mod world;

/// Build metadata used by host diagnostics and persisted status output.
pub fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("unknown")
}

pub use config::RuntimeConfig;
pub use harness_builder::HarnessBuild;
pub use harness_builder::HostRuntime;
pub use harness_builder::ModelProviderFactory;
pub use harness_builder::RuntimeBuilder;
pub use harness_builder::harness_config;
pub use harness_builder::harness_config_auto;
pub use harness_builder::prepare_harness_with_model_factory;
pub use harness_builder::prepare_openai_harness;
pub use harness_builder::prepare_openai_harness_with_profile;
pub use harness_builder::prepare_openai_harness_with_profile_and_result_store;
pub use harness_builder::print_auto_warning;
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
pub use world::WorldState;
