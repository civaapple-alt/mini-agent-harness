use mini_agent_app_server::AppServerError;
use mini_agent_app_server::ApprovalBroker;
use mini_agent_app_server::RuntimeManagementService;
use mini_agent_app_server::RuntimeServices;
use mini_agent_app_server::StartupServices;
use mini_agent_app_server::capability_manifest_to_protocol;
use mini_agent_app_server::serve_stdio_with_startup_and_services;
use mini_agent_app_server_protocol::CapabilityProviderSelection;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_capabilities::SecurityPolicy;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_host::HostRuntimeFactory;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_host::load_workspace_profile;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::ToolApprovalRequest;
use std::env;
use std::error::Error;
use tokio::io::BufReader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let runtime_config = RuntimeConfig::load().map_err(std::io::Error::other)?;
    let broker = ApprovalBroker::new();
    let base_profile = match env::var("MINI_AGENT_PROFILE") {
        Ok(name) => RuntimeProfile::builtin(&name)
            .ok_or_else(|| std::io::Error::other(format!("unknown MINI_AGENT_PROFILE `{name}`")))?,
        Err(env::VarError::NotPresent) => RuntimeProfile::interactive_default(),
        Err(error) => return Err(Box::new(error) as Box<dyn Error>),
    };
    let startup_config = runtime_config.clone();
    let startup_broker = broker.clone();
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_startup_and_services(broker, stdin, stdout, move |params| {
        let base_profile = match params.profile {
            Some(name) => RuntimeProfile::builtin(&name)
                .ok_or_else(|| format!("unknown startup profile `{name}`"))?,
            None => base_profile.clone(),
        };
        let mut profile = load_workspace_profile(&startup_config.workspace(), base_profile)?;
        apply_provider_selection(&mut profile, params.providers.as_ref())?;
        let factory_profile = profile.clone();
        let approval = approval_for(startup_broker.clone());
        let management_approval = approval.clone();
        let runtime = HostRuntimeFactory::new(&startup_config, approval, HarnessConfig::default())
            .build(profile, Default::default())?;
        let mini_agent_host::HarnessBuild {
            harness,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            capability_manifest,
            ..
        } = runtime;
        let capability_manifest = capability_manifest_to_protocol(&capability_manifest);
        let thread_id = ThreadId::new("default");
        let thread = Thread::new(thread_id.clone(), harness);
        let factory_broker = startup_broker.clone();
        let factory_config = startup_config.clone();
        let server = mini_agent_app_server::AppServer::with_thread_factory(
            ThreadStart::new(thread_id.clone()),
            vec![thread],
            move |thread_id| {
                let approval = approval_for(factory_broker.clone());
                let runtime =
                    HostRuntimeFactory::new(&factory_config, approval, HarnessConfig::default())
                        .build(factory_profile.clone(), Default::default())
                        .map_err(AppServerError::Checkpoint)?;
                Ok(Thread::new(thread_id, runtime.harness))
            },
        );
        let management = RuntimeManagementService::new(
            server.clone(),
            None,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            management_approval,
        );
        let workflows = mini_agent_app_server::WorkflowService::new(
            startup_config.workspace(),
            startup_config.goal_limits(),
        );
        Ok((
            server,
            capability_manifest,
            StartupServices {
                runtime: Some(RuntimeServices::new(management, workflows)?),
            },
        ))
    })
    .await?;
    Ok(())
}

fn apply_provider_selection(
    profile: &mut RuntimeProfile,
    providers: Option<&CapabilityProviderSelection>,
) -> Result<(), String> {
    let Some(providers) = providers else {
        return Ok(());
    };
    if let Some(provider) = providers.model.as_deref() {
        profile.model_provider = provider.to_string();
    }
    if let Some(provider) = providers.tools.as_deref() {
        profile.tool_provider = provider.to_string();
    }
    if let Some(provider) = providers.extensions.as_deref() {
        profile.extension_provider = provider.to_string();
    }
    if let Some(provider) = providers.policy.as_deref() {
        profile.policy_provider = provider.to_string();
    }
    Ok(())
}

fn approval_for(broker: ApprovalBroker) -> ApprovalController {
    ApprovalController::with_policy_and_context_callback(
        ApprovalMode::Interactive,
        SecurityPolicy::for_preset(SecurityPreset::Default),
        move |request: &ToolApprovalRequest| {
            broker
                .request_with_context(request)
                .map_err(mini_agent_protocol::ToolError)
        },
    )
}
