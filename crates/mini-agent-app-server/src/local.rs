//! Local embedded-client bootstrap helpers.
//!
//! These helpers keep profile resolution and runtime construction behind the
//! App Server boundary. A CLI or another local frontend supplies only its
//! input/output and approval adapter.

use crate::AppServerRuntime;
use crate::RuntimeStartOptions;
use crate::SessionRequest;
use crate::frontend::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_core::HarnessConfig;
use mini_agent_core::RunControl;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_host::WorldState;
use mini_agent_host::harness_config;
use mini_agent_host::harness_config_auto;
use std::sync::Arc;

/// Inputs used by an embedded local frontend to resolve one runtime.
pub struct LocalRuntimeRequest {
    pub automatic: bool,
    pub no_tools: bool,
    pub security_preset: SecurityPreset,
    pub security_preset_explicit: bool,
    pub sandbox_kind: SandboxKind,
    pub sandbox_kind_explicit: bool,
    pub web_search_override: Option<bool>,
    pub session_request: SessionRequest,
    pub max_steps: Option<usize>,
}

/// Returns the bounded startup summary shown by local frontends before the
/// runtime worker is ready.
pub fn world_summary(
    workspace: &std::path::Path,
    approval: ApprovalMode,
    copilot: bool,
    sandbox: SandboxKind,
) -> String {
    WorldState::detect(workspace, approval, copilot, sandbox).summary()
}

/// Fully resolved local runtime inputs, before the frontend approval callback
/// is attached and the App Server starts its Thread.
pub struct LocalRuntimeLaunch {
    runtime_config: RuntimeConfig,
    harness_config: HarnessConfig,
    profile: RuntimeProfile,
    session_request: SessionRequest,
}

impl LocalRuntimeLaunch {
    pub fn runtime_config(&self) -> RuntimeConfig {
        self.runtime_config.clone()
    }

    pub fn copilot_max_steps(&self) -> usize {
        self.runtime_config.copilot_max_steps()
    }

    pub fn web_search_enabled(&self) -> bool {
        self.runtime_config.web_search()
    }

    pub fn workflow_scope(&self) -> crate::frontend::WorkflowScope {
        self.profile.workflows
    }

    pub fn security_preset(&self) -> SecurityPreset {
        self.profile.security
    }
}

/// Resolves configuration and the bounded workspace profile for a local
/// embedded client. No provider resources or Thread are created here.
pub fn prepare(request: LocalRuntimeRequest) -> Result<LocalRuntimeLaunch, String> {
    let mut runtime_config = RuntimeConfig::load()?;
    if let Some(enabled) = request.web_search_override {
        runtime_config = runtime_config.with_web_search(enabled);
    }
    let profile = if request.automatic {
        RuntimeProfile::auto_default()
    } else {
        RuntimeProfile::ask_default()
    };
    let mut profile =
        mini_agent_host::load_workspace_profile(&runtime_config.workspace(), profile)?;
    if request.no_tools {
        profile = profile.without_tools();
    }
    if request.sandbox_kind_explicit {
        profile = profile.with_sandbox(request.sandbox_kind);
    }
    if request.security_preset_explicit {
        profile = profile.with_security(request.security_preset);
    }
    let harness_config = match (request.automatic, request.max_steps) {
        (true, steps) => harness_config_auto(
            true,
            steps.unwrap_or_else(|| runtime_config.copilot_max_steps()),
        ),
        (false, Some(steps)) => harness_config_auto(true, steps),
        (false, None) => harness_config(false),
    };
    Ok(LocalRuntimeLaunch {
        runtime_config,
        harness_config,
        profile,
        session_request: request.session_request,
    })
}

impl LocalRuntimeLaunch {
    /// Starts the local App Server with a frontend-owned approval adapter.
    pub async fn start(self, approval: ApprovalController) -> Result<AppServerRuntime, String> {
        self.start_with_control(approval, Arc::new(RunControl::new()))
            .await
    }

    /// Starts the local App Server while sharing run control with an input
    /// worker, as the interactive REPL does.
    pub async fn start_with_control(
        self,
        approval: ApprovalController,
        control: Arc<RunControl>,
    ) -> Result<AppServerRuntime, String> {
        AppServerRuntime::start(RuntimeStartOptions {
            runtime_config: self.runtime_config,
            approval: approval.into_capability(),
            harness_config: self.harness_config,
            session_request: self.session_request,
            control,
            profile: self.profile,
            registry: mini_agent_capabilities::CapabilityRegistry::builtin(),
        })
        .await
    }
}
