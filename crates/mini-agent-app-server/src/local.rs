//! Local embedded-client bootstrap helpers.
//!
//! These helpers keep runtime composition and construction behind the App
//! Server boundary. A CLI or another local frontend supplies only its
//! input/output and approval adapter.

use crate::AppServerRuntime;
use crate::RuntimeStartOptions;
use crate::SessionRequest;
use crate::frontend::ApprovalController;
use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPreset;
use mini_agent_core::HarnessConfig;
use mini_agent_core::RunControl;
use mini_agent_host::RuntimeComposition;
use mini_agent_host::RuntimeConfig;
use std::sync::Arc;

/// Inputs used by an embedded local frontend to resolve one runtime.
pub struct LocalRuntimeRequest {
    pub no_tools: bool,
    pub security_preset: SecurityPreset,
    pub security_preset_explicit: bool,
    pub sandbox_kind: SandboxKind,
    pub sandbox_kind_explicit: bool,
    pub web_search_override: Option<bool>,
    pub session_request: SessionRequest,
}

/// Fully resolved local runtime inputs, before the frontend approval callback
/// is attached and the App Server starts its Thread.
pub struct LocalRuntimeLaunch {
    runtime_config: RuntimeConfig,
    harness_config: HarnessConfig,
    composition: RuntimeComposition,
    session_request: SessionRequest,
}

impl LocalRuntimeLaunch {
    pub fn runtime_config(&self) -> RuntimeConfig {
        self.runtime_config.clone()
    }

    pub fn security_preset(&self) -> SecurityPreset {
        self.composition.security
    }
}

/// Resolves configuration and the bounded runtime composition for a local
/// embedded client. No provider resources or Thread are created here.
pub fn prepare(request: LocalRuntimeRequest) -> Result<LocalRuntimeLaunch, String> {
    let mut runtime_config = RuntimeConfig::load()?;
    if let Some(enabled) = request.web_search_override {
        runtime_config = runtime_config.with_web_search(enabled);
    }
    let mut composition = RuntimeComposition::default();
    if request.no_tools {
        composition = composition.without_tools();
    }
    if request.sandbox_kind_explicit {
        composition = composition.with_sandbox(request.sandbox_kind);
    }
    if request.security_preset_explicit {
        composition = composition.with_security(request.security_preset);
    }
    let harness_config = HarnessConfig::default();
    Ok(LocalRuntimeLaunch {
        runtime_config,
        harness_config,
        composition,
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
            composition: self.composition,
            registry: mini_agent_capabilities::CapabilityRegistry::builtin(),
        })
        .await
    }
}
