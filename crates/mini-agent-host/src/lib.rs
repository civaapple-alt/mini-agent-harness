//! Application-host runtime composition for mini-agent.
//!
//! Concrete model, policy, and persistence providers live in
//! `mini-agent-capabilities`. This crate owns runtime composition and product
//! workflows. It deliberately does not own terminal input or command-line
//! dispatch; those belong to `mini-agent-cli`.

pub mod config;
pub mod env_file;
mod goal;
mod harness_builder;
pub mod project_context;
#[path = "profile.rs"]
mod runtime_composition;
pub mod runtime_factory;
mod tool_catalog;
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
pub use goal::VerdictOutcome;
pub use goal::VerifierVerdict;
pub use goal::goal_turn_prompt;
pub use goal::parse_verifier_verdict;
pub use goal::with_plan_mode_overlay;
pub use harness_builder::HarnessBuild;
pub use harness_builder::HostRuntime;
pub use harness_builder::ModelProviderFactory;
pub use harness_builder::prepare_harness_with_model_factory;
pub use runtime_composition::AgentKind;
pub use runtime_composition::CapabilityManifest;
pub use runtime_composition::ContextLimits;
pub use runtime_composition::ExtensionLoadDepth;
pub use runtime_composition::ExtensionSelection;
pub use runtime_composition::PersonaKind;
pub use runtime_composition::PromptSources;
pub use runtime_composition::RegularAgentConfig;
pub use runtime_composition::RulePolicy;
pub use runtime_composition::RuleSourceState;
pub use runtime_composition::RuleSourceStatus;
pub use runtime_composition::RuleSources;
pub use runtime_composition::RuntimeComposition;
pub use runtime_composition::SourceFingerprint;
pub use runtime_composition::ToolScope;
pub use runtime_composition::WorkflowScope;
pub use runtime_factory::HostRuntimeFactory;
pub use tool_catalog::BuiltinToolSelection;
pub use tool_orchestrator::ToolOrchestrator;
pub use world::WorldState;
