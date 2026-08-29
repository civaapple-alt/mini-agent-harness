//! Minimal model-provider seam example.
//!
//! This example is compile-checked with the crate and intentionally does not
//! contact a provider. An embedding application can call `start` after
//! resolving its own configuration and credentials.

#![allow(dead_code)]

use mini_agent_app_server::AppServerRuntime;
use mini_agent_app_server::SessionRequest;
use mini_agent_capabilities::CapabilityDescriptor;
use mini_agent_capabilities::CapabilityKind;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::ModelProviderSettings;
use mini_agent_capabilities::workspace::ApprovalController;
use mini_agent_capabilities::workspace::ApprovalMode;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use std::error::Error;

struct EchoModel;

#[derive(Debug)]
struct EchoError;

impl std::fmt::Display for EchoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("example model error")
    }
}

impl Error for EchoError {}

impl Model for EchoModel {
    type Error = EchoError;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let _ = request;
        Ok(ModelResponse {
            reasoning: String::new(),
            text: "response from an external model provider".to_string(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

fn echo_factory(
    provider_id: &str,
    _settings: ModelProviderSettings,
    _images: ImageStore,
) -> Result<EchoModel, String> {
    (provider_id == "example-model")
        .then_some(EchoModel)
        .ok_or_else(|| format!("unsupported provider: {provider_id}"))
}

/// The host still assembles tools and policy; this function supplies only the
/// model implementation and a registry entry for its stable profile ID.
async fn start(runtime_config: RuntimeConfig) -> Result<AppServerRuntime<EchoModel>, String> {
    let profile = RuntimeProfile::interactive_default().with_model_provider("example-model");
    let registry = CapabilityRegistry::builtin().with_model_provider(CapabilityDescriptor {
        id: "example-model",
        kind: CapabilityKind::Model,
        description: "Example in-process model provider",
    });
    AppServerRuntime::<EchoModel>::start_with_model_factory(
        runtime_config,
        ApprovalController::new(ApprovalMode::Automatic),
        HarnessConfig::default(),
        SessionRequest::Disabled,
        std::sync::Arc::new(mini_agent_core::RunControl::new()),
        profile,
        registry,
        echo_factory,
    )
    .await
}

fn main() {
    println!("external provider seam: AppServerRuntime::<EchoModel>::start_with_model_factory");
}
