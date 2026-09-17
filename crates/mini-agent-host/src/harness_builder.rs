use mini_agent_capabilities::{
    ApprovalController, CapabilityRegistry, ImageStore, McpLoadResult, McpServerConfig,
    ModelProviderSettings, OpenAiModel, ResultStore,
};
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRouter;
use mini_agent_protocol::Model;
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::project_context;
use crate::tool_catalog::BuiltinToolSelection;
use crate::tool_orchestrator::ToolOrchestrator;
use crate::world::WorldState;
use crate::{
    CapabilityManifest, ExtensionLoadDepth, ExtensionSelection, RuntimeComposition,
    SourceFingerprint, ToolScope,
};
pub struct HarnessBuild<M: Model> {
    pub harness: Harness<M>,
    pub images: ImageStore,
    pub stable_system_prompt: String,
    pub world: WorldState,
    pub enabled_mcp_servers: Vec<String>,
    pub mcp_tool_count: usize,
    pub retry_mcp_servers: Vec<McpServerConfig>,
    pub capability_manifest: CapabilityManifest,
    pub skill_discovery: Option<mini_agent_capabilities::Discovery>,
}

/// The fully assembled application-host runtime handed to a frontend or
/// service boundary. It owns the concrete provider-backed Harness together
/// with host state needed by persistence, extensions, and workflow adapters.
pub type HostRuntime = HarnessBuild<OpenAiModel>;

const PROVIDER_WEB_SEARCH_PROMPT: &str = "## Provider web search\nThe model provider has enabled its server-side `web_search` tool for current web research. Use `web_search` for current or broad web research. It is separate from the Host `web_fetch` tool, which is only for reading an exact URL. Do not claim that `web_search` is unavailable merely because it is not listed with Host function tools; only report it unavailable after the provider returns a tool error.";

fn append_provider_web_search_prompt(prompt: &mut String, enabled: bool) {
    if enabled {
        prompt.push_str("\n\n");
        prompt.push_str(PROVIDER_WEB_SEARCH_PROMPT);
    }
}

/// Provider seam used by the Host composition root to construct a model
/// without coupling the runtime assembly to one concrete HTTP provider.
pub trait ModelProviderFactory<M>: Send + Sync {
    fn build(
        &self,
        provider_id: &str,
        settings: ModelProviderSettings,
        images: ImageStore,
    ) -> Result<M, String>;
}

impl<M, F> ModelProviderFactory<M> for F
where
    F: Fn(&str, ModelProviderSettings, ImageStore) -> Result<M, String> + Send + Sync,
{
    fn build(
        &self,
        provider_id: &str,
        settings: ModelProviderSettings,
        images: ImageStore,
    ) -> Result<M, String> {
        self(provider_id, settings, images)
    }
}

/// Builds a Host runtime with an embedding application's model provider.
///
/// Tool, policy, extension, world, and prompt assembly remain identical to
/// the built-in path; only model construction crosses this explicit seam.
pub fn prepare_harness_with_model_factory<M, F>(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    composition: RuntimeComposition,
    results: ResultStore,
    registry: CapabilityRegistry,
    model_factory: F,
) -> Result<HarnessBuild<M>, String>
where
    M: Model,
    F: ModelProviderFactory<M>,
{
    let mut config = config;
    let policy = registry.build_policy(&composition.policy_provider, composition.security)?;
    approval.set_policy(policy);
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Model,
        &composition.model_provider,
    )?;
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Tool,
        &composition.tool_provider,
    )?;
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Extension,
        &composition.extension_provider,
    )?;
    let provider = runtime_config.provider_settings()?;
    append_provider_web_search_prompt(&mut config.system_prompt, provider.web_search);
    let images = ImageStore::for_provider(provider.api_key.clone(), &provider.base_url);
    let model = model_factory.build(
        &composition.model_provider,
        ModelProviderSettings {
            api_key: provider.api_key,
            model: provider.model,
            base_url: provider.base_url,
            web_search: provider.web_search,
        },
        images.clone(),
    )?;
    let workspace = runtime_config.workspace();
    let mut capability_manifest = composition.manifest_with_config(&config);
    let composition_overlay = composition.prompt_overlay();
    if !composition_overlay.is_empty() {
        config.system_prompt = format!("{composition_overlay}\n\n{}", config.system_prompt);
    }
    let project_fingerprint =
        if composition.regular_agent.prompts.project || composition.regular_agent.rules.project {
            let project_instructions = project_context::load_agents_md(&workspace)?;
            if let Some(warning) = project_instructions.truncation_warning() {
                eprintln!("warning: {warning}");
            }
            let fingerprint = project_instructions.fingerprint();
            if composition.regular_agent.prompts.project {
                config.system_prompt = project_instructions.augment(&config.system_prompt);
            }
            fingerprint
        } else {
            None
        };
    let mut skill_discovery = (composition.extensions != ExtensionLoadDepth::None
        && (composition.regular_agent.prompts.extensions
            || composition.regular_agent.rules.extensions))
        .then(|| {
            registry.discover_extensions_with_builtin_groups(
                &composition.extension_provider,
                &workspace,
                &composition.builtin_skill_groups,
            )
        })
        .transpose()?;
    if let Some(discovery) = &mut skill_discovery {
        if let ExtensionSelection::Named(names) = &composition.extension_selection {
            discovery.retain_selected(names);
        }
        for diagnostic in discovery.diagnostics() {
            eprintln!("warning: {diagnostic}");
        }
        capability_manifest.available_skills = discovery.skill_catalog();
        let extension_fingerprint = discovery.prompt_fingerprint()?;
        if composition.regular_agent.prompts.extensions {
            config.system_prompt = discovery.augment_system_prompt(&config.system_prompt)?;
        }
        if let Some(fingerprint) = extension_fingerprint {
            if composition.regular_agent.prompts.extensions {
                capability_manifest
                    .prompt_source_fingerprints
                    .push(SourceFingerprint {
                        source: "extensions".to_string(),
                        fingerprint: fingerprint.clone(),
                    });
            }
            if composition.regular_agent.rules.extensions {
                capability_manifest
                    .rule_source_fingerprints
                    .push(SourceFingerprint {
                        source: "extensions".to_string(),
                        fingerprint,
                    });
            }
        }
    }
    if let Some(fingerprint) = project_fingerprint {
        if composition.regular_agent.prompts.project {
            capability_manifest
                .prompt_source_fingerprints
                .push(SourceFingerprint {
                    source: "project".to_string(),
                    fingerprint: fingerprint.clone(),
                });
        }
        if composition.regular_agent.rules.project {
            capability_manifest
                .rule_source_fingerprints
                .push(SourceFingerprint {
                    source: "project".to_string(),
                    fingerprint,
                });
        }
    }
    let mut tools = if composition.tools == ToolScope::All {
        match registry.build_tools(mini_agent_capabilities::ToolBuildRequest {
            provider_id: composition.tool_provider.clone(),
            workspace: workspace.clone(),
            approval: approval.clone(),
            extra_read_roots: runtime_config.extra_read_roots(),
            skill_read_roots: skill_discovery
                .as_ref()
                .map_or_else(Vec::new, |discovery| discovery.skill_read_roots()),
            extra_write_roots: runtime_config.extra_write_roots(),
            sandbox: composition.sandbox,
            images: images.clone(),
            results,
        }) {
            Ok(tools) => tools,
            Err(error) => return Err(error.to_string()),
        }
    } else {
        Vec::new()
    };
    let configured_mcp_servers = if composition.extensions == ExtensionLoadDepth::Enabled
        && composition.tools == ToolScope::All
    {
        skill_discovery
            .as_ref()
            .map_or_else(Vec::new, |discovery| discovery.mcp_servers().to_vec())
    } else {
        Vec::new()
    };
    let McpLoadResult {
        tools: mcp_tools,
        loaded_servers,
        diagnostics,
    } = if composition.extensions == ExtensionLoadDepth::Enabled
        && composition.tools == ToolScope::All
    {
        registry
            .load_mcp(
                &composition.extension_provider,
                &configured_mcp_servers,
                approval.clone(),
            )
            .map_err(|error| error.to_string())?
    } else {
        McpLoadResult {
            tools: Vec::new(),
            loaded_servers: Default::default(),
            diagnostics: Vec::new(),
        }
    };
    for diagnostic in diagnostics {
        eprintln!("warning: {diagnostic}");
    }
    let enabled_mcp_servers = loaded_servers.iter().cloned().collect();
    let mcp_tool_count = mcp_tools.len();
    tools.extend(mcp_tools);
    let retry_mcp_servers = configured_mcp_servers
        .into_iter()
        .filter(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        })
        .collect();
    let stable_system_prompt = config.system_prompt.clone();
    let mut extra_roots = runtime_config.extra_write_roots();
    extra_roots.extend(runtime_config.extra_read_roots());
    extra_roots.sort();
    extra_roots.dedup();
    let world = WorldState::detect_with_roots(
        &workspace,
        extra_roots,
        composition.security,
        approval.approval_policy(),
        composition.sandbox,
    );
    let world_context = world.model_context()?;
    let tool_executor = Arc::new(ToolOrchestrator::new(approval.clone()));
    let tool_registry = ToolRouter::with_executor(tools, tool_executor);
    let mut harness = Harness::new(model, tool_registry, config);
    harness.set_hidden_tools(BuiltinToolSelection::default().hidden_names());
    harness
        .append_context(world_context)
        .map_err(|error| error.to_string())?;
    Ok(HarnessBuild {
        harness,
        images,
        stable_system_prompt,
        world,
        enabled_mcp_servers,
        mcp_tool_count,
        retry_mcp_servers,
        capability_manifest,
        skill_discovery,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_search_prompt_is_only_added_when_enabled() {
        let mut prompt = "base prompt".to_string();
        append_provider_web_search_prompt(&mut prompt, false);
        assert_eq!(prompt, "base prompt");

        append_provider_web_search_prompt(&mut prompt, true);
        assert!(prompt.contains("`web_search`"));
        assert!(prompt.contains("`web_fetch`"));
    }
}
