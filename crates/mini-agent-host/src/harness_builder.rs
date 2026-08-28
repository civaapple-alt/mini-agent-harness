use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRegistry;

use crate::config::RuntimeConfig;
use crate::image::ImageStore;
use crate::mcp;
use crate::openai::OpenAiModel;
use crate::profile::{
    CapabilityManifest, ExtensionLoadDepth, ExtensionSelection, RuntimeProfile, SourceFingerprint,
    ToolScope,
};
use crate::project_context;
use crate::result_store::ResultStore;
use crate::sandbox::SandboxKind;
use crate::skills;
use crate::tool_outcome::classify_tools;
use crate::workspace::ApprovalController;
use crate::workspace::workspace_tools_with_read_roots_and_results;
use crate::world::WorldState;

pub const AUTO_MAX_STEPS: usize = 0;

pub struct HarnessBuild {
    pub harness: Harness<OpenAiModel>,
    pub images: ImageStore,
    pub stable_system_prompt: String,
    pub world: WorldState,
    pub enabled_mcp_servers: Vec<String>,
    pub mcp_tool_count: usize,
    pub retry_mcp_servers: Vec<skills::McpServerConfig>,
    pub capability_manifest: CapabilityManifest,
}

/// The fully assembled application-host runtime handed to a frontend or
/// service boundary. It owns the concrete provider-backed Harness together
/// with host state needed by persistence, extensions, and workflow adapters.
pub type HostRuntime = HarnessBuild;

/// Composes provider, tools, policy, extensions, and world context outside the
/// CLI. Frontends should depend on this builder instead of importing concrete
/// host modules to assemble a Harness themselves.
pub struct RuntimeBuilder<'a> {
    runtime_config: &'a RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    sandbox: SandboxKind,
    profile: RuntimeProfile,
}

impl<'a> RuntimeBuilder<'a> {
    pub fn new(
        runtime_config: &'a RuntimeConfig,
        approval: ApprovalController,
        config: HarnessConfig,
        sandbox: SandboxKind,
    ) -> Self {
        Self {
            runtime_config,
            approval,
            config,
            sandbox,
            profile: RuntimeProfile::default(),
        }
    }

    pub fn with_profile(mut self, profile: RuntimeProfile) -> Self {
        self.profile = profile;
        self
    }

    pub fn build(&self) -> Result<HostRuntime, String> {
        self.approval
            .set_read_only_agent(self.profile.agent.is_read_only());
        prepare_openai_harness_with_profile(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
            self.sandbox,
            self.profile.clone(),
        )
    }

    pub fn build_with_result_store(&self, results: ResultStore) -> Result<HostRuntime, String> {
        self.approval
            .set_read_only_agent(self.profile.agent.is_read_only());
        prepare_openai_harness_with_profile_and_result_store(
            self.runtime_config,
            self.approval.clone(),
            self.config.clone(),
            self.sandbox,
            self.profile.clone(),
            results,
        )
    }
}

pub fn prepare_openai_harness(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    sandbox: SandboxKind,
) -> Result<HarnessBuild, String> {
    prepare_openai_harness_with_profile_and_result_store(
        runtime_config,
        approval,
        config,
        sandbox,
        RuntimeProfile::default(),
        ResultStore::default(),
    )
}

pub fn prepare_openai_harness_with_profile(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    sandbox: SandboxKind,
    profile: RuntimeProfile,
) -> Result<HarnessBuild, String> {
    prepare_openai_harness_with_profile_and_result_store(
        runtime_config,
        approval,
        config,
        sandbox,
        profile,
        ResultStore::default(),
    )
}

pub fn prepare_openai_harness_with_result_store(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    config: HarnessConfig,
    sandbox: SandboxKind,
    results: ResultStore,
) -> Result<HarnessBuild, String> {
    prepare_openai_harness_with_profile_and_result_store(
        runtime_config,
        approval,
        config,
        sandbox,
        RuntimeProfile::default(),
        results,
    )
}

pub fn prepare_openai_harness_with_profile_and_result_store(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
    sandbox: SandboxKind,
    profile: RuntimeProfile,
    results: ResultStore,
) -> Result<HarnessBuild, String> {
    let provider = runtime_config.provider_settings()?;
    let copilot = config.context_limit_behavior == ContextLimitBehavior::Compact;
    let images = ImageStore::for_provider(provider.api_key.clone(), &provider.base_url);
    let model = match OpenAiModel::new(
        provider.api_key,
        provider.model,
        provider.base_url,
        provider.web_search,
        images.clone(),
    ) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
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
        .then(|| skills::discover(&workspace));
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
        let extra_read_roots = skill_discovery
            .as_ref()
            .map_or_else(Vec::new, |discovery| discovery.extra_read_roots().to_vec());
        match workspace_tools_with_read_roots_and_results(
            workspace.clone(),
            approval.clone(),
            extra_read_roots,
            sandbox,
            images.clone(),
            results,
        ) {
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
    let mcp::LoadResult {
        tools: mcp_tools,
        loaded_servers,
        diagnostics,
    } = if profile.extensions == ExtensionLoadDepth::Enabled && profile.tools == ToolScope::All {
        mcp::load(&configured_mcp_servers, approval)
    } else {
        mcp::LoadResult {
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
    let tools = classify_tools(tools);
    let retry_mcp_servers = configured_mcp_servers
        .into_iter()
        .filter(|server| {
            !loaded_servers.contains(&format!("{}/{}", server.plugin_name, server.server_name))
        })
        .collect();
    let stable_system_prompt = config.system_prompt.clone();
    let world = WorldState::detect(&workspace, approval_mode, copilot, sandbox);
    let world_context = world.model_context()?;
    let mut harness = Harness::new(model, ToolRegistry::new(tools), config);
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

#[allow(dead_code)]
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

pub fn print_auto_warning() {
    eprintln!(
        "warning: auto mode runs workspace writes, MCP servers, and unsandboxed shell commands without approval"
    );
}
