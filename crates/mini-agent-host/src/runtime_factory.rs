//! Host-owned runtime construction seam.
//!
//! App Server and frontends select a bounded [`RuntimeProfile`], while this
//! factory remains the only place that turns the selection into concrete
//! provider, tool, extension, policy, and session-bound artifacts.

use crate::HostRuntime;
use crate::RuntimeBuilder;
use crate::RuntimeConfig;
use crate::RuntimeProfile;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::result_store::ResultStore;
use mini_agent_capabilities::workspace::ApprovalController;
use mini_agent_core::HarnessConfig;

/// Builds a concrete host runtime for an App Server service boundary.
///
/// The factory carries edge configuration and the frontend approval callback,
/// while the profile selects the concrete policy provider and sandbox. Profile
/// selection remains explicit so each frontend can choose an allowlisted
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
        profile: RuntimeProfile,
        results: ResultStore,
    ) -> Result<HostRuntime, String> {
        self.approval
            .set_read_only_agent(profile.agent.is_read_only());
        RuntimeBuilder::new(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
        )
        .with_profile(profile)
        .with_registry(self.registry.clone())
        .build_with_result_store(results)
    }
}
