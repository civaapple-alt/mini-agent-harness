//! Host-owned runtime construction seam.
//!
//! App Server and frontends select a bounded [`RuntimeComposition`], while this
//! factory remains the only place that turns the selection into concrete
//! provider, tool, extension, policy, and session-bound artifacts.

use crate::BuiltinToolSelection;
use crate::HostRuntime;
use crate::RuntimeComposition;
use crate::RuntimeConfig;
use crate::ToolScope;
use crate::WebSearchSettingsStore;
use crate::harness_builder::prepare_harness_with_model_factory;
use crate::models::{HostResponsesModel, ModelCatalogStore};
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::ModelProviderSettings;
use mini_agent_capabilities::ResultStore;
use mini_agent_core::HarnessConfig;

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
        let catalog = ModelCatalogStore::machine_default()?;
        let project_id = self.runtime_config.project_id();
        let selected_search =
            if composition.tools == ToolScope::All && self.runtime_config.web_search() {
                WebSearchSettingsStore::machine_default()?.runtime_config()?
            } else {
                None
            };
        let search_enabled = selected_search.is_some();
        let registry = selected_search.map_or_else(
            || self.registry.clone(),
            |config| self.registry.clone().with_web_search(config),
        );
        let mut runtime = prepare_harness_with_model_factory(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
            composition,
            results,
            registry,
            move |_provider_id: &str, _settings: ModelProviderSettings, images: ImageStore| {
                Ok(HostResponsesModel::new(
                    catalog.clone(),
                    project_id.clone(),
                    images,
                ))
            },
        )?;
        runtime.builtin_tools = builtin_tools_for_search(search_enabled);
        runtime
            .harness
            .set_hidden_tools(runtime.builtin_tools.hidden_names());
        Ok(runtime)
    }
}

fn builtin_tools_for_search(search_enabled: bool) -> BuiltinToolSelection {
    if search_enabled {
        BuiltinToolSelection::all()
    } else {
        BuiltinToolSelection::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_search_includes_fetch_in_the_initial_builtin_selection() {
        let with_search = builtin_tools_for_search(true);
        assert!(with_search.names().iter().any(|name| name == "web_fetch"));
        assert!(with_search.hidden_names().is_empty());

        let without_search = builtin_tools_for_search(false);
        assert!(
            !without_search
                .names()
                .iter()
                .any(|name| name == "web_fetch")
        );
        assert_eq!(without_search.hidden_names(), ["web_fetch"]);
    }
}
