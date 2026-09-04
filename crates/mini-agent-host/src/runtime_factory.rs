//! Host-owned runtime construction seam.
//!
//! App Server and frontends select a bounded [`RuntimeComposition`], while this
//! factory remains the only place that turns the selection into concrete
//! provider, tool, extension, policy, and session-bound artifacts.

use crate::HostRuntime;
use crate::RuntimeComposition;
use crate::RuntimeConfig;
use crate::harness_builder::prepare_harness_with_model_factory;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::ModelProviderSettings;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_capabilities::ResultStore;
use mini_agent_capabilities::build_model;
use mini_agent_core::HarnessConfig;

fn openai_model_factory(
    provider_id: &str,
    settings: ModelProviderSettings,
    images: ImageStore,
) -> Result<OpenAiModel, String> {
    build_model(provider_id, settings, images).map_err(|error| error.to_string())
}

/// Builds a concrete host runtime for an App Server service boundary.
///
/// The factory carries edge configuration and the frontend approval callback,
/// while the composition selects the concrete policy provider and sandbox.
/// Composition selection remains explicit so each frontend can choose an allowlisted
/// capability scope without creating a second execution loop.
pub struct HostRuntimeFactory<'a> {
    runtime_config: &'a RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    registry: CapabilityRegistry,
}

impl<'a> HostRuntimeFactory<'a> {
    pub fn new(
        runtime_config: &'a RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
    ) -> Self {
        Self {
            runtime_config,
            approval,
            config,
            registry: CapabilityRegistry::builtin(),
        }
    }

    /// Uses providers registered by the embedding application for new runs.
    pub fn with_registry(mut self, registry: CapabilityRegistry) -> Self {
        self.registry = registry;
        self
    }

    pub fn build(
        &self,
        composition: RuntimeComposition,
        results: ResultStore,
    ) -> Result<HostRuntime, String> {
        self.approval
            .set_read_only_agent(composition.agent.is_read_only());
        prepare_harness_with_model_factory(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
            composition,
            results,
            self.registry.clone(),
            openai_model_factory,
        )
    }
}
