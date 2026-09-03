use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ModelProviderSettings;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRegistry;
use mini_agent_protocol::Model;
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::profile::{
    CapabilityManifest, ExtensionLoadDepth, ExtensionSelection, RuntimeProfile, SourceFingerprint,
    ToolScope,
};
use crate::project_context;
use crate::tool_catalog::BuiltinToolSelection;
use crate::tool_orchestrator::ToolOrchestrator;
use crate::world::WorldState;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::McpLoadResult;
use mini_agent_capabilities::McpServerConfig;
use mini_agent_capabilities::ResultStore;

pub const AUTO_MAX_STEPS: usize = 0;

pub struct HarnessBuild<M: Model> {
    pub harness: Harness<M>,
    pub images: ImageStore,
    pub stable_system_prompt: String,
    pub world: WorldState,
    pub enabled_mcp_servers: Vec<String>,
    pub mcp_tool_count: usize,
    pub retry_mcp_servers: Vec<McpServerConfig>,
    pub capability_manifest: CapabilityManifest,
}

/// The fully assembled application-host runtime handed to a frontend or
/// service boundary. It owns the concrete provider-backed Harness together
/// with host state needed by persistence, extensions, and workflow adapters.
pub type HostRuntime = HarnessBuild<OpenAiModel>;

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
    profile: RuntimeProfile,
    results: ResultStore,
    registry: CapabilityRegistry,
    model_factory: F,
) -> Result<HarnessBuild<M>, String>
where
    M: Model,
    F: ModelProviderFactory<M>,
{
    prepare_harness_with_profile_and_result_store_and_registry(
        runtime_config,
        approval,
        config,
        profile,
        results,
        registry,
        model_factory,
    )
}

fn prepare_harness_with_profile_and_result_store_and_registry<M, F>(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
    profile: RuntimeProfile,
    results: ResultStore,
    registry: CapabilityRegistry,
    model_factory: F,
) -> Result<HarnessBuild<M>, String>
where
    M: Model,
    F: ModelProviderFactory<M>,
{
    let policy = registry.build_policy(&profile.policy_provider, profile.security)?;
    approval.set_policy(policy);
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Model,
        &profile.model_provider,
    )?;
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Tool,
        &profile.tool_provider,
    )?;
    registry.validate(
        mini_agent_capabilities::CapabilityKind::Extension,
        &profile.extension_provider,
    )?;
    let provider = runtime_config.provider_settings()?;
    let copilot = config.context_limit_behavior == ContextLimitBehavior::Compact;
    let images = ImageStore::for_provider(provider.api_key.clone(), &provider.base_url);
    let model = model_factory.build(
        &profile.model_provider,
        ModelProviderSettings {
            api_key: provider.api_key,
            model: provider.model,
            base_url: provider.base_url,
            web_search: provider.web_search,
        },
        images.clone(),
    )?;
    let workspace = runtime_config.workspace();
    let mut capability_manifest = profile.manifest_with_config(&config);
    let profile_overlay = profile.prompt_overlay();
    if !profile_overlay.is_empty() {
        config.system_prompt = format!("{profile_overlay}\n\n{}", config.system_prompt);
    }
    let project_fingerprint =
        if profile.regular_agent.prompts.project || profile.regular_agent.rules.project {
            let project_instructions = project_context::load_agents_md(&workspace)?;
            if let Some(warning) = project_instructions.truncation_warning() {
                eprintln!("warning: {warning}");
            }
            let fingerprint = project_instructions.fingerprint();
            if profile.regular_agent.prompts.project {
                config.system_prompt = project_instructions.augment(&config.system_prompt);
            }
            fingerprint
        } else {
            None
        };
    let mut skill_discovery = (profile.extensions != ExtensionLoadDepth::None
        && (profile.regular_agent.prompts.extensions || profile.regular_agent.rules.extensions))
        .then(|| registry.discover_extensions(&profile.extension_provider, &workspace))
        .transpose()?;
    if let Some(discovery) = &mut skill_discovery {
        if let ExtensionSelection::Named(names) = &profile.extension_selection {
            discovery.retain_selected(names);
        }
        for diagnostic in discovery.diagnostics() {
            eprintln!("warning: {diagnostic}");
        }
        let extension_fingerprint = discovery.prompt_fingerprint()?;
        if profile.regular_agent.prompts.extensions {
            config.system_prompt = discovery.augment_system_prompt(&config.system_prompt)?;
        }
        if let Some(fingerprint) = extension_fingerprint {
            if profile.regular_agent.prompts.extensions {
                capability_manifest
                    .prompt_source_fingerprints
                    .push(SourceFingerprint {
                        source: "extensions".to_string(),
                        fingerprint: fingerprint.clone(),
                    });
            }
            if profile.regular_agent.rules.extensions {
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
        if profile.regular_agent.prompts.project {
            capability_manifest
                .prompt_source_fingerprints
                .push(SourceFingerprint {
                    source: "project".to_string(),
                    fingerprint: fingerprint.clone(),
                });
        }
        if profile.regular_agent.rules.project {
            capability_manifest
                .rule_source_fingerprints
                .push(SourceFingerprint {
                    source: "project".to_string(),
                    fingerprint,
                });
        }
    }
    let mut tools = if profile.tools == ToolScope::All {
        match registry.build_tools(mini_agent_capabilities::ToolBuildRequest {
            provider_id: profile.tool_provider.clone(),
            workspace: workspace.clone(),
            approval: approval.clone(),
            extra_read_roots: Vec::new(),
            sandbox: profile.sandbox,
            images: images.clone(),
            results,
        }) {
            Ok(tools) => tools,
            Err(error) => return Err(error.to_string()),
        }
    } else {
        Vec::new()
    };
    let configured_mcp_servers =
        if profile.extensions == ExtensionLoadDepth::Enabled && profile.tools == ToolScope::All {
            skill_discovery
                .as_ref()
                .map_or_else(Vec::new, |discovery| discovery.mcp_servers().to_vec())
        } else {
            Vec::new()
        };
    let approval_mode = approval.mode();
    let McpLoadResult {
        tools: mcp_tools,
        loaded_servers,
        diagnostics,
    } = if profile.extensions == ExtensionLoadDepth::Enabled && profile.tools == ToolScope::All {
        registry
            .load_mcp(
                &profile.extension_provider,
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
    let world = WorldState::detect(&workspace, approval_mode, copilot, profile.sandbox);
    let world_context = world.model_context()?;
    let tool_executor = Arc::new(ToolOrchestrator::new(approval.clone()));
    let tool_registry = ToolRegistry::with_executor(tools, tool_executor);
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
    })
}

pub fn harness_config(copilot: bool) -> HarnessConfig {
    harness_config_auto(copilot, AUTO_MAX_STEPS)
}

pub fn harness_config_auto(copilot: bool, auto_max_steps: usize) -> HarnessConfig {
    if copilot {
        HarnessConfig {
            max_steps: if auto_max_steps == 0 {
                usize::MAX
            } else {
                auto_max_steps
            },
            context_limit_behavior: ContextLimitBehavior::Compact,
            ..HarnessConfig::default()
        }
    } else {
        HarnessConfig::default()
    }
}
