//! Application-host profile resolution and runtime composition for mini-agent.
//!
//! Concrete model, policy, and persistence providers live in
//! `mini-agent-capabilities`. This crate owns profile resolution, runtime
//! composition, and product workflows. It deliberately does not own terminal
//! input or command-line dispatch; those belong to `mini-agent-cli`.

pub mod config;
pub mod env_file;
mod goal;
mod harness_builder;
pub mod observer;
pub mod profile;
pub mod project_context;
pub mod runtime_factory;
mod tool_orchestrator;
pub mod world;

#[cfg(test)]
#[path = "test_support_tests.rs"]
pub(crate) mod test_support;

/// Build metadata used by host diagnostics and persisted status output.
pub fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("unknown")
}

pub use config::RuntimeConfig;
pub use goal::GoalLimits;
pub use goal::GoalState;
pub use goal::GoalStatus;
pub use goal::HostWorkflowStore;
pub use goal::PlanSlash;
pub use goal::VerdictOutcome;
pub use goal::VerifierVerdict;
pub use goal::goal_turn_prompt;
pub use goal::parse_plan_slash;
pub use goal::parse_verifier_verdict;
pub use goal::planning_turn_prompt;
pub use goal::with_plan_mode_overlay;
pub use harness_builder::HarnessBuild;
pub use harness_builder::HostRuntime;
pub use harness_builder::ModelProviderFactory;
pub use harness_builder::harness_config;
pub use harness_builder::harness_config_auto;
pub use harness_builder::prepare_harness_with_model_factory;
pub use harness_builder::print_auto_warning;
pub use observer::RunObserver;
pub use profile::AgentKind;
pub use profile::CapabilityManifest;
pub use profile::ContextLimits;
pub use profile::ExtensionLoadDepth;
pub use profile::ExtensionSelection;
pub use profile::PersonaKind;
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
