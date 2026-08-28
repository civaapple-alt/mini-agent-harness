use mini_agent_app_server::AppServerError;
use mini_agent_app_server::ApprovalBroker;
use mini_agent_app_server::serve_stdio_with_approval;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_host::ApprovalController;
use mini_agent_host::ApprovalMode;
use mini_agent_host::RuntimeBuilder;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::SandboxKind;
use mini_agent_host::SecurityPreset;
use std::error::Error;
use tokio::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let runtime_config = RuntimeConfig::load().map_err(std::io::Error::other)?;
    let broker = ApprovalBroker::new();
    let approval_broker = broker.clone();
    let approval = ApprovalController::with_policy_and_callback(
        ApprovalMode::Interactive,
        mini_agent_host::security::SecurityPolicy::for_preset(SecurityPreset::Default),
        move |action| {
            approval_broker
                .request(action)
                .map_err(mini_agent_core::ToolError)
        },
    );
    let runtime = RuntimeBuilder::new(
        &runtime_config,
        approval,
        HarnessConfig::default(),
        SandboxKind::Native,
    )
    .build()
    .map_err(std::io::Error::other)?;
    let thread_id = ThreadId::new("default");
    let thread = Thread::new(thread_id.clone(), runtime.harness);
    let factory_broker = broker.clone();
    let server = mini_agent_app_server::AppServer::with_thread_factory(
        ThreadStart::new(thread_id),
        vec![thread],
        move |thread_id| {
            let runtime_config = RuntimeConfig::load()
                .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
            let callback_broker = factory_broker.clone();
            let approval = ApprovalController::with_policy_and_callback(
                ApprovalMode::Interactive,
                mini_agent_host::security::SecurityPolicy::for_preset(SecurityPreset::Default),
                move |action| {
                    callback_broker
                        .request(action)
                        .map_err(mini_agent_core::ToolError)
                },
            );
            let runtime = RuntimeBuilder::new(
                &runtime_config,
                approval,
                HarnessConfig::default(),
                SandboxKind::Native,
            )
            .build()
            .map_err(AppServerError::Checkpoint)?;
            Ok(Thread::new(thread_id, runtime.harness))
        },
    );
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_approval(server, broker, stdin, stdout).await?;
    Ok(())
}
