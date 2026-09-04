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
use mini_agent_capabilities::ApprovalScope;
use mini_agent_capabilities::ApprovalStore;
use mini_agent_capabilities::OpenedSession;
use mini_agent_capabilities::SecurityPolicy;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_capabilities::SessionRequest;
use mini_agent_capabilities::SessionStore;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Thread;
use mini_agent_host::HostRuntimeFactory;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::ToolApprovalRequest;
use std::error::Error;
use tokio::io::BufReader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let runtime_config = RuntimeConfig::load().map_err(std::io::Error::other)?;
    let broker = ApprovalBroker::new();
    let approval_store = ApprovalStore::new();
    let startup_approval = approval_for(
        broker.clone(),
        &runtime_config,
        approval_store.clone(),
        None,
        SecurityPreset::Default,
        ApprovalScope::PerAction,
    );
    let base_profile = RuntimeProfile::interactive_default();
    let startup_config = runtime_config.clone();
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_startup_and_services(broker.clone(), stdin, stdout, move |params| {
        let mut profile = base_profile.clone();
        apply_provider_selection(&mut profile, params.providers.as_ref())?;
        let factory_profile = profile.clone();
        let session = open_session(&startup_config)?;
        let goal_workspace = session
            .as_ref()
            .and_then(|opened| opened.store.path().parent().map(std::path::PathBuf::from))
            .unwrap_or_else(|| startup_config.workspace().to_path_buf());
        let runtime_approval = startup_approval.clone();
        if let Some(opened) = &session {
            runtime_approval.bind_session_file(opened.store.path());
            runtime_approval.bind_approval_context(
                Some(startup_config.project_id()),
                Some(startup_config.workspace().display().to_string()),
                Some(startup_config.workspace_revision()),
                Some(opened.store.session_id().to_string()),
            );
        }
        let management_approval = runtime_approval.clone();
        let results = session
            .as_ref()
            .map(|opened| opened.store.result_store())
            .unwrap_or_default();
        let runtime = HostRuntimeFactory::new(
            &startup_config,
            runtime_approval.clone(),
            HarnessConfig::default(),
        )
        .build(profile, results)?;
        let mini_agent_host::HarnessBuild {
            harness,
            images,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            stable_system_prompt,
            capability_manifest,
            ..
        } = runtime;
        let mut harness = harness;
        if let Some(opened) = &session {
            images.bind_session_file(opened.store.path());
            if opened.resumed {
                harness
                    .restore_session(opened.state.clone())
                    .map_err(|error| error.to_string())?;
            }
        }
        let capability_manifest = capability_manifest_to_protocol(&capability_manifest);
        let thread_id = session
            .as_ref()
            .map(|opened| ThreadId::new(opened.store.thread_id().to_string()))
            .unwrap_or_else(|| ThreadId::new("default"));
        let mut thread = Thread::new(thread_id.clone(), harness);
        if let Some(opened) = &session {
            thread.set_next_turn_number(opened.store.thread_turn_count() as u64 + 1);
        }
        let factory_broker = broker.clone();
        let factory_store = approval_store.clone();
        let factory_config = startup_config.clone();
        let server = mini_agent_app_server::AppServer::with_thread_factory(
            ThreadStart::new(thread_id.clone()),
            vec![thread],
            move |thread_id: ThreadId| {
                let (access, selected_approval) = factory_broker.execution_scope();
                let security = security_preset(access);
                let approval = approval_for(
                    factory_broker.clone(),
                    &factory_config,
                    factory_store.clone(),
                    Some(thread_id.as_str().to_string()),
                    security,
                    approval_scope(selected_approval),
                );
                let mut thread_profile = factory_profile.clone();
                thread_profile.security = security;
                let runtime =
                    HostRuntimeFactory::new(&factory_config, approval, HarnessConfig::default())
                        .build(thread_profile, Default::default())
                        .map_err(AppServerError::Checkpoint)?;
                Ok(Thread::new(thread_id, runtime.harness))
            },
        );
        let management = RuntimeManagementService::new(
            server.clone(),
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            management_approval,
        );
        let thread_settings = mini_agent_app_server::ThreadSettingsService::new()
            .with_stable_system_prompt(stable_system_prompt);
        let goals = mini_agent_app_server::ThreadGoalRequestProcessor::new(
            goal_workspace,
            startup_config.goal_limits(),
        )
        .with_verifier_config(startup_config.clone());
        Ok((
            server,
            capability_manifest,
            StartupServices {
                runtime: Some(RuntimeServices::new(management, thread_settings, goals)?),
            },
        ))
    })
    .await?;
    Ok(())
}

fn open_session(runtime_config: &RuntimeConfig) -> Result<Option<OpenedSession>, String> {
    let mode = std::env::var("MINI_AGENT_SESSION_MODE").unwrap_or_else(|_| "disabled".to_string());
    let request = match mode.as_str() {
        "disabled" => return Ok(None),
        "new" => SessionRequest::New,
        "named" => SessionRequest::Named(
            std::env::var("MINI_AGENT_SESSION_ID")
                .map_err(|_| "MINI_AGENT_SESSION_ID is required for named sessions".to_string())?,
        ),
        "resume" => {
            SessionRequest::Resume(std::env::var("MINI_AGENT_SESSION_ID").map_err(|_| {
                "MINI_AGENT_SESSION_ID is required for resumed sessions".to_string()
            })?)
        }
        other => return Err(format!("unknown MINI_AGENT_SESSION_MODE: {other}")),
    };
    SessionStore::open(&runtime_config.workspace(), request).map(Some)
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

fn approval_for(
    broker: ApprovalBroker,
    runtime_config: &RuntimeConfig,
    store: ApprovalStore,
    session_id: Option<String>,
    security: SecurityPreset,
    scope: ApprovalScope,
) -> ApprovalController {
    let approval = ApprovalController::with_policy_and_context_callback(
        ApprovalMode::Interactive,
        SecurityPolicy::for_preset(security),
        move |request: &ToolApprovalRequest| {
            broker
                .request_with_context(request)
                .map_err(mini_agent_protocol::ToolError)
        },
    )
    .with_approval_store(store);
    approval.bind_approval_context(
        Some(runtime_config.project_id()),
        Some(runtime_config.workspace().display().to_string()),
        Some(runtime_config.workspace_revision()),
        session_id,
    );
    approval.set_approval_scope(scope);
    approval
}

fn security_preset(access: mini_agent_app_server_protocol::AccessScope) -> SecurityPreset {
    match access {
        mini_agent_app_server_protocol::AccessScope::Project => SecurityPreset::Default,
        mini_agent_app_server_protocol::AccessScope::FullMachine => SecurityPreset::FullMachine,
    }
}

fn approval_scope(approval: mini_agent_app_server_protocol::ApprovalMode) -> ApprovalScope {
    match approval {
        mini_agent_app_server_protocol::ApprovalMode::PerAction => ApprovalScope::PerAction,
        mini_agent_app_server_protocol::ApprovalMode::CurrentSession => {
            ApprovalScope::CurrentSession
        }
        mini_agent_app_server_protocol::ApprovalMode::CurrentProject => {
            ApprovalScope::CurrentProject
        }
    }
}
