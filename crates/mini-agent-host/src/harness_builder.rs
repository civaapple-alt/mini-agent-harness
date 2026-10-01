use mini_agent_capabilities::{
    ApprovalController, CapabilityRegistry, ImageStore, McpLoadResult, McpServerConfig,
    ModelProviderSettings, ResultStore,
};
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRouter;
use mini_agent_protocol::Model;
use std::path::PathBuf;
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
    /// Builtin selection already applied to the model-visible tool catalog.
    pub builtin_tools: BuiltinToolSelection,
    pub images: ImageStore,
    pub stable_system_prompt: String,
    pub world: WorldState,
    pub enabled_mcp_servers: Vec<String>,
    pub mcp_tool_count: usize,
    pub retry_mcp_servers: Vec<McpServerConfig>,
    pub capability_manifest: CapabilityManifest,
    pub skill_discovery: Option<mini_agent_capabilities::Discovery>,
    pub skill_discovery_refresh: Option<SkillDiscoveryRefresh>,
    pub skill_read_roots: mini_agent_capabilities::SkillReadRoots,
    pub background_shells: mini_agent_capabilities::BackgroundShellManager,
    pub scheduled_tasks: mini_agent_capabilities::ScheduledTaskManager,
}

/// Host-owned inputs needed to refresh the effective Skills catalog without
/// rebuilding the Thread runtime or its stable prompt and tool set.
#[derive(Clone)]
pub struct SkillDiscoveryRefresh {
    registry: CapabilityRegistry,
    provider_id: String,
    workspace: PathBuf,
    enabled_groups: Vec<String>,
    selection: ExtensionSelection,
    prompt_enabled: bool,
}

impl SkillDiscoveryRefresh {
    pub fn new(
        registry: CapabilityRegistry,
        provider_id: impl Into<String>,
        workspace: PathBuf,
        enabled_groups: Vec<String>,
        selection: ExtensionSelection,
        prompt_enabled: bool,
    ) -> Self {
        Self {
            registry,
            provider_id: provider_id.into(),
            workspace,
            enabled_groups,
            selection,
            prompt_enabled,
        }
    }

    fn discover(&self) -> Result<mini_agent_capabilities::Discovery, String> {
        let mut discovery = self.registry.discover_extensions_with_builtin_groups(
            &self.provider_id,
            &self.workspace,
            &self.enabled_groups,
        )?;
        if let ExtensionSelection::Named(names) = &self.selection {
            discovery.retain_selected(names);
        }
        Ok(discovery)
    }

    pub fn refresh(&self) -> Result<mini_agent_capabilities::Discovery, String> {
        self.discover()
    }

    pub fn prompt_enabled(&self) -> bool {
        self.prompt_enabled
    }
}

/// The fully assembled application-host runtime handed to a frontend or
/// service boundary. It owns the concrete provider-backed Harness together
/// with host state needed by persistence, extensions, and workflow adapters.
pub type HostRuntime = HarnessBuild<crate::models::HostResponsesModel>;

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
    let images = ImageStore::for_provider(provider.api_key.clone(), &provider.base_url);
    let model = model_factory.build(
        &composition.model_provider,
        ModelProviderSettings {
            api_key: provider.api_key,
            model: provider.model,
            base_url: provider.base_url,
        },
        images.clone(),
    )?;
    let workspace = runtime_config.workspace();
    let background_shells = mini_agent_capabilities::BackgroundShellManager::new();
    let scheduled_tasks = mini_agent_capabilities::ScheduledTaskManager::new();
    let mut capability_manifest = composition.manifest_with_config(&config);
    let composition_overlay = composition.prompt_overlay();
    if !composition_overlay.is_empty() {
        config.system_prompt = format!("{composition_overlay}\n\n{}", config.system_prompt);
    }
    let project_instruction_loader = if composition.regular_agent.prompts.project {
        project_context::ProjectInstructionLoader::from_configured_roots(
            &workspace,
            &runtime_config.extra_read_roots(),
        )
    } else {
        project_context::ProjectInstructionLoader::default()
    };
    let startup_injections = project_instruction_loader.startup_injections()?;
    for injection in &startup_injections {
        if let Some(warning) = &injection.warning {
            eprintln!("warning: {warning}");
        }
    }
    if composition.regular_agent.prompts.project {
        config.max_context_item_bytes = config
            .max_context_item_bytes
            .max(project_context::MAX_PROJECT_INSTRUCTIONS_BYTES + 1024);
    }
    let project_fingerprint =
        if composition.regular_agent.prompts.project || composition.regular_agent.rules.project {
            let fingerprints = startup_injections
                .iter()
                .map(|injection| {
                    format!(
                        "{}:{}:{}",
                        injection.record.workspace.as_deref().unwrap_or(""),
                        injection.record.path.as_deref().unwrap_or(""),
                        injection.record.fingerprint
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!fingerprints.is_empty())
                .then(|| crate::runtime_composition::stable_fingerprint(fingerprints.as_bytes()))
        } else {
            None
        };
    let skill_discovery_refresh = (composition.extensions != ExtensionLoadDepth::None
        && (composition.regular_agent.prompts.extensions
            || composition.regular_agent.rules.extensions))
        .then(|| {
            SkillDiscoveryRefresh::new(
                registry.clone(),
                composition.extension_provider.clone(),
                workspace.clone(),
                composition.builtin_skill_groups.clone(),
                composition.extension_selection.clone(),
                composition.regular_agent.prompts.extensions,
            )
        });
    let mut skill_discovery = skill_discovery_refresh
        .as_ref()
        .map(SkillDiscoveryRefresh::discover)
        .transpose()?;
    if let Some(discovery) = &mut skill_discovery {
        for diagnostic in discovery.diagnostics() {
            eprintln!("warning: {diagnostic}");
        }
        capability_manifest.available_skills = discovery.skill_catalog();
        let extension_fingerprint = discovery.prompt_fingerprint()?;
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
    let skill_read_roots = mini_agent_capabilities::SkillReadRoots::from_paths(
        skill_discovery
            .as_ref()
            .map_or_else(Vec::new, |discovery| discovery.skill_read_roots()),
    );
    if skill_discovery_refresh
        .as_ref()
        .is_some_and(SkillDiscoveryRefresh::prompt_enabled)
    {
        config.max_context_item_bytes = config
            .max_context_item_bytes
            .max(mini_agent_capabilities::MAX_SKILL_CONTEXT_BYTES)
            .max(mini_agent_capabilities::MAX_ACTIVATED_SKILL_CONTEXT_BYTES);
    } else if skill_discovery_refresh.is_some() {
        config.max_context_item_bytes = config
            .max_context_item_bytes
            .max(mini_agent_capabilities::MAX_ACTIVATED_SKILL_CONTEXT_BYTES);
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
            session_read_roots: runtime_config.session_read_roots(),
            skill_read_roots: skill_read_roots.clone(),
            extra_write_roots: runtime_config.extra_write_roots(),
            sandbox: composition.sandbox,
            images: images.clone(),
            results: results.clone(),
            background_shells: background_shells.clone(),
            scheduled_tasks: scheduled_tasks.clone(),
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
    let world = WorldState::detect_with_root_sets(
        &workspace,
        runtime_config.extra_read_roots(),
        runtime_config.extra_write_roots(),
        runtime_config.session_read_roots(),
        composition.security,
        approval.approval_policy(),
        composition.sandbox,
    );
    let world_context = world.model_context()?;
    let tool_executor = Arc::new(
        ToolOrchestrator::new(approval.clone())
            .with_project_instructions(project_instruction_loader)
            .with_result_store(results.clone()),
    );
    let tool_registry = ToolRouter::with_executor(tools, tool_executor);
    let mut harness = Harness::new(model, tool_registry, config);
    harness.set_hidden_tools(BuiltinToolSelection::default().hidden_names());
    for injection in startup_injections {
        let _ = harness
            .append_context_injection(injection.message, injection.record)
            .map_err(|error| error.to_string())?;
    }
    let (world_message, world_record) = host_context_message(
        "world_state",
        mini_agent_protocol::ContextInjectionKind::WorkspaceState,
        "工作区状态",
        "主工作区",
        "当前运行环境和工作区状态",
        &world_context,
    );
    let _ = harness
        .append_context_injection(world_message, world_record)
        .map_err(|error| error.to_string())?;
    if approval.session_dir().is_some() {
        let capabilities = crate::world::session_capabilities_context();
        let (message, record) = host_context_message(
            "session_capabilities",
            mini_agent_protocol::ContextInjectionKind::Other,
            "会话能力",
            "会话",
            "当前会话允许使用的附加能力",
            capabilities,
        );
        let _ = harness
            .append_context_injection(message, record)
            .map_err(|error| error.to_string())?;
    }
    Ok(HarnessBuild {
        harness,
        builtin_tools: BuiltinToolSelection::default(),
        images,
        stable_system_prompt,
        world,
        enabled_mcp_servers,
        mcp_tool_count,
        retry_mcp_servers,
        capability_manifest,
        skill_discovery,
        skill_discovery_refresh,
        skill_read_roots,
        background_shells,
        scheduled_tasks,
    })
}

fn host_context_message(
    id: &str,
    kind: mini_agent_protocol::ContextInjectionKind,
    source: &str,
    workspace: &str,
    scope: &str,
    context: &str,
) -> (String, mini_agent_protocol::ContextInjectionRecord) {
    let opening = format!("<{id}>");
    let closing = format!("</{id}>");
    let body = context
        .strip_prefix(&opening)
        .and_then(|value| value.strip_suffix(&closing))
        .unwrap_or(context);
    let fingerprint = mini_agent_protocol::stable_digest(context.as_bytes());
    let record = mini_agent_protocol::ContextInjectionRecord {
        id: id.to_string(),
        kind,
        source: source.to_string(),
        workspace: Some(workspace.to_string()),
        path: None,
        scope: scope.to_string(),
        bytes: context.len() as u64,
        fingerprint,
        supersedes: None,
        reused: false,
    };
    (record.context_message(body), record)
}
