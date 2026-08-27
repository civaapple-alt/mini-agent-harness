use mini_agent_core::ContextLimitBehavior;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::ToolRegistry;

use crate::config::RuntimeConfig;
use crate::mcp;
use crate::openai::OpenAiModel;
use crate::project_context;
use crate::skills;
use crate::workspace::ApprovalController;
use crate::workspace::workspace_tools_with_read_roots;
use crate::world::WorldState;

pub(crate) const AUTO_MAX_STEPS: usize = 0;

pub(crate) struct HarnessBuild {
    pub(crate) harness: Harness<OpenAiModel>,
    pub(crate) stable_system_prompt: String,
    pub(crate) world: WorldState,
    pub(crate) enabled_mcp_servers: Vec<String>,
    pub(crate) mcp_tool_count: usize,
    pub(crate) retry_mcp_servers: Vec<skills::McpServerConfig>,
}

pub(crate) fn prepare_openai_harness(
    runtime_config: &RuntimeConfig,
    approval: ApprovalController,
    mut config: HarnessConfig,
) -> Result<HarnessBuild, String> {
    let provider = runtime_config.provider_settings()?;
    let copilot = config.context_limit_behavior == ContextLimitBehavior::Compact;
    let model = match OpenAiModel::new(
        provider.api_key,
        provider.model,
        provider.base_url,
        provider.web_search,
    ) {
        Ok(model) => model,
        Err(error) => return Err(error.to_string()),
    };
    let workspace = runtime_config.workspace();
    let project_instructions = project_context::load_agents_md(&workspace)?;
    if let Some(warning) = project_instructions.truncation_warning() {
        eprintln!("warning: {warning}");
    }
    config.system_prompt = project_instructions.augment(&config.system_prompt);
    let skill_discovery = skills::discover(&workspace);
    for diagnostic in skill_discovery.diagnostics() {
        eprintln!("warning: {diagnostic}");
    }
    config.system_prompt = skill_discovery.augment_system_prompt(&config.system_prompt)?;
    let mut tools = match workspace_tools_with_read_roots(
        workspace.clone(),
        approval.clone(),
        skill_discovery.extra_read_roots().to_vec(),
    ) {
        Ok(tools) => tools,
        Err(error) => return Err(error.to_string()),
    };
    let configured_mcp_servers = skill_discovery.mcp_servers().to_vec();
    let approval_mode = approval.mode();
    let mcp::LoadResult {
        tools: mcp_tools,
        loaded_servers,
        diagnostics,
    } = mcp::load(&configured_mcp_servers, approval);
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
    let world = WorldState::detect(&workspace, approval_mode, copilot);
    let world_context = world.model_context()?;
    let mut harness = Harness::new(model, ToolRegistry::new(tools), config);
    harness
        .append_context(world_context)
        .map_err(|error| error.to_string())?;
    Ok(HarnessBuild {
        harness,
        stable_system_prompt,
        world,
        enabled_mcp_servers,
        mcp_tool_count,
        retry_mcp_servers,
    })
}

#[allow(dead_code)]
pub(crate) fn harness_config(copilot: bool) -> HarnessConfig {
    harness_config_auto(copilot, AUTO_MAX_STEPS)
}

pub(crate) fn harness_config_auto(copilot: bool, auto_max_steps: usize) -> HarnessConfig {
    if copilot {
        HarnessConfig {
            max_steps: auto_max_steps,
            context_limit_behavior: ContextLimitBehavior::Compact,
            ..HarnessConfig::default()
        }
    } else {
        HarnessConfig::default()
    }
}

pub(crate) fn print_auto_warning() {
    eprintln!(
        "warning: auto mode runs workspace writes, MCP servers, and unsandboxed shell commands without approval"
    );
}
