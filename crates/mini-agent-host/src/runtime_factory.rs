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
use std::sync::Arc;

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
    user_questions: Option<Arc<dyn crate::UserQuestionHandler>>,
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
            user_questions: None,
        }
    }

    /// Uses providers registered by the embedding application for new runs.
    pub fn with_registry(mut self, registry: CapabilityRegistry) -> Self {
        self.registry = registry;
        self
    }

    pub fn with_user_questions(mut self, handler: Arc<dyn crate::UserQuestionHandler>) -> Self {
        self.user_questions = Some(handler);
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
        let model_approval = self.approval.clone();
        let registry = selected_search.map_or_else(
            || self.registry.clone(),
            |config| self.registry.clone().with_web_search(config),
        );
        let user_questions_enabled =
            self.user_questions.is_some() && composition.tools == ToolScope::All;
        let mut runtime = prepare_harness_with_model_factory(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
            composition,
            results,
            registry,
            move |_provider_id: &str, _settings: ModelProviderSettings, images: ImageStore| {
                Ok(
                    HostResponsesModel::new(catalog.clone(), project_id.clone(), images)
                        .with_plan_mode_approval(model_approval.clone()),
                )
            },
        )?;
        if user_questions_enabled {
            let tool: Box<dyn mini_agent_protocol::Tool> = Box::new(crate::AskUserTool::new(
                self.user_questions
                    .as_ref()
                    .expect("enabled questions have an interactive handler")
                    .clone(),
            ));
            runtime.harness.extend_tools(vec![tool]);
        }
        runtime.builtin_tools = builtin_tools_for_features(search_enabled, user_questions_enabled);
        runtime
            .harness
            .set_hidden_tools(runtime.builtin_tools.hidden_names());
        Ok(runtime)
    }
}

fn builtin_tools_for_features(
    search_enabled: bool,
    user_questions_enabled: bool,
) -> BuiltinToolSelection {
    let mut names = BuiltinToolSelection::default().names().to_vec();
    if search_enabled {
        names.push("web_fetch".to_string());
    }
    if user_questions_enabled {
        names.push("ask_user".to_string());
    }
    BuiltinToolSelection::from_names(names).expect("feature tools are in the builtin catalog")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_search_includes_fetch_in_the_initial_builtin_selection() {
        let with_search = builtin_tools_for_features(true, false);
        assert!(with_search.names().iter().any(|name| name == "web_fetch"));
        assert_eq!(with_search.hidden_names(), ["ask_user"]);

        let without_search = builtin_tools_for_features(false, false);
        assert!(
            !without_search
                .names()
                .iter()
                .any(|name| name == "web_fetch")
        );
        assert_eq!(without_search.hidden_names(), ["web_fetch", "ask_user"]);
    }

    #[test]
    fn user_question_tool_is_selected_only_for_interactive_clients() {
        assert!(
            builtin_tools_for_features(false, true)
                .names()
                .contains(&"ask_user".to_string())
        );
        assert!(
            !builtin_tools_for_features(false, false)
                .names()
                .contains(&"ask_user".to_string())
        );
    }
}
