use mini_agent_app_server::AppServerError;
use mini_agent_app_server::ApprovalBroker;
use mini_agent_app_server::capability_manifest_to_protocol;
use mini_agent_app_server::serve_stdio_with_startup;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_host::ApprovalController;
use mini_agent_host::ApprovalMode;
use mini_agent_host::HostRuntimeFactory;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_host::SandboxKind;
use mini_agent_host::SecurityPreset;
use mini_agent_host::load_workspace_profile;
use std::env;
use std::error::Error;
use tokio::io::BufReader;

#[tokio::main(flavor = "current_thread")]
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
    serve_stdio_with_startup(broker, stdin, stdout, move |params| {
        let base_profile = match params.profile {
            Some(name) => RuntimeProfile::builtin(&name)
                .ok_or_else(|| format!("unknown startup profile `{name}`"))?,
            None => base_profile.clone(),
        };
        let profile = load_workspace_profile(&startup_config.workspace(), base_profile)?;
        let factory_profile = profile.clone();
        let approval = approval_for(startup_broker.clone());
        let runtime = HostRuntimeFactory::new(
            &startup_config,
            approval,
            HarnessConfig::default(),
            SandboxKind::Native,
        )
        .build(profile, Default::default())?;
        let capability_manifest = capability_manifest_to_protocol(&runtime.capability_manifest);
        let thread_id = ThreadId::new("default");
        let thread = Thread::new(thread_id.clone(), runtime.harness);
        let factory_broker = startup_broker.clone();
        let factory_config = startup_config.clone();
        let server = mini_agent_app_server::AppServer::with_thread_factory(
            ThreadStart::new(thread_id),
            vec![thread],
            move |thread_id| {
                let approval = approval_for(factory_broker.clone());
                let runtime = HostRuntimeFactory::new(
                    &factory_config,
                    approval,
                    HarnessConfig::default(),
                    SandboxKind::Native,
                )
                .build(factory_profile.clone(), Default::default())
                .map_err(AppServerError::Checkpoint)?;
                Ok(Thread::new(thread_id, runtime.harness))
            },
        );
        Ok((server, capability_manifest))
    })
    .await?;
    Ok(())
}

fn approval_for(broker: ApprovalBroker) -> ApprovalController {
    ApprovalController::with_policy_and_callback(
        ApprovalMode::Interactive,
        mini_agent_host::security::SecurityPolicy::for_preset(SecurityPreset::Default),
        move |action| broker.request(action).map_err(mini_agent_core::ToolError),
    )
}
