//! Host-backed runtime used by local clients.
//!
//! `AppServerRuntime` is the composition root for an in-process service. The
//! host creates the provider-backed Thread and persistence state here, then
//! all turn execution goes through the same protocol client used by clients and
//! the external JSON-RPC transport.

use crate::AppServer;
use crate::AppServerConnection;
use crate::LocalAppServerClient;
use crate::RuntimeManagementService;
use crate::RuntimeServices;
use crate::workflows::WorkflowService;
use mini_agent_app_server_protocol::CapabilityManifest as ProtocolCapabilityManifest;
use mini_agent_app_server_protocol::ContextLimits as ProtocolContextLimits;
use mini_agent_app_server_protocol::DisabledCapability;
use mini_agent_app_server_protocol::RulePolicy as ProtocolRulePolicy;
use mini_agent_app_server_protocol::RuleSourceStatus as ProtocolRuleSourceStatus;
use mini_agent_app_server_protocol::SourceFingerprint as ProtocolSourceFingerprint;
use mini_agent_app_server_protocol::TurnReadResult;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::CapabilityRegistry;
use mini_agent_capabilities::ImageStore;
use mini_agent_capabilities::ModelProviderSettings;
use mini_agent_capabilities::OpenAiModel;
use mini_agent_capabilities::SessionStore;
use mini_agent_capabilities::build_model;
use mini_agent_core::HarnessConfig;
use mini_agent_core::RunControl;
use mini_agent_core::Thread;
use mini_agent_host::CapabilityManifest;
use mini_agent_host::ModelProviderFactory;
use mini_agent_host::RuntimeConfig;
use mini_agent_host::RuntimeProfile;
use mini_agent_host::prepare_harness_with_model_factory;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use serde::Serialize;
use std::path::PathBuf;

fn openai_model_factory(
    provider_id: &str,
    settings: ModelProviderSettings,
    images: ImageStore,
) -> Result<OpenAiModel, String> {
    build_model(provider_id, settings, images).map_err(|error| error.to_string())
}

/// The settled result projected by the local App Server runtime.
pub type RuntimeTurnResult = TurnReadResult;

/// All turns settled while one start request was being serviced. A steer or
/// follow-up may cause the App Server to settle more than one turn before the
/// service becomes idle.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTurnBatch {
    pub turns: Vec<RuntimeTurnResult>,
}

/// Session selection requested by a local or remote App Server client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    Disabled,
    New,
    Named(String),
    Resume(String),
    Fork(String),
}

/// Stable session metadata exposed to local and remote management clients.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSessionInfo {
    pub session_id: String,
    pub thread_id: String,
    pub path: String,
    pub resumed: bool,
}

/// Result of retrying MCP servers that were deferred during startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpRetryResult {
    pub enabled_servers: Vec<String>,
    pub inactive_servers: Vec<String>,
    pub diagnostics: Vec<String>,
    pub tool_count: usize,
}

/// A provider-backed App Server plus the host state needed by local clients.
pub struct AppServerRuntime<M: Model = OpenAiModel> {
    client: LocalAppServerClient<M>,
    images: ImageStore,
    model_name: String,
    stable_system_prompt: String,
    capability_manifest: CapabilityManifest,
}

/// Explicit inputs for constructing one host-backed App Server runtime.
///
/// The model factory remains a separate argument because it is the only
/// application-defined implementation seam; all runtime policy and identity
/// inputs travel together in this value.
pub struct RuntimeStartOptions {
    pub runtime_config: RuntimeConfig,
    pub approval: ApprovalController,
    pub harness_config: HarnessConfig,
    pub session_request: SessionRequest,
    pub control: std::sync::Arc<RunControl>,
    pub profile: RuntimeProfile,
    pub registry: CapabilityRegistry,
}

impl AppServerRuntime<OpenAiModel> {
    pub async fn start(options: RuntimeStartOptions) -> Result<Self, String> {
        Self::start_with_model_factory(options, openai_model_factory).await
    }
}

impl<M: Model + Send + 'static> AppServerRuntime<M> {
    /// Builds a runtime with an embedding application's model provider and
    /// capability registry. The Host still owns tool, policy, extension, and
    /// world assembly; only model construction crosses this seam.
    pub async fn start_with_model_factory<F>(
        options: RuntimeStartOptions,
        model_factory: F,
    ) -> Result<Self, String>
    where
        F: ModelProviderFactory<M>,
    {
        let RuntimeStartOptions {
            runtime_config,
            approval,
            harness_config,
            session_request,
            control,
            profile,
            registry,
        } = options;
        let workspace = runtime_config.workspace();
        let goal_limits = runtime_config.goal_limits();
        let model_name = runtime_config.model().unwrap_or_default().to_string();
        let session = match session_request {
            SessionRequest::Disabled => None,
            other => {
                let request = match other {
                    SessionRequest::New => mini_agent_capabilities::SessionRequest::New,
                    SessionRequest::Named(id) => mini_agent_capabilities::SessionRequest::Named(id),
                    SessionRequest::Resume(id) => {
                        mini_agent_capabilities::SessionRequest::Resume(id)
                    }
                    SessionRequest::Fork(id) => mini_agent_capabilities::SessionRequest::Fork(id),
                    SessionRequest::Disabled => unreachable!("disabled session handled above"),
                };
                let opened = SessionStore::open(&workspace, request)
                    .map_err(|error| format!("cannot open session: {error}"))?;
                approval.bind_session_file(opened.store.path());
                Some(opened)
            }
        };
        let results = session
            .as_ref()
            .map(|opened| opened.store.result_store())
            .unwrap_or_default();
        let mini_agent_host::HarnessBuild {
            harness,
            images,
            stable_system_prompt,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            capability_manifest,
        } = prepare_harness_with_model_factory(
            &runtime_config,
            approval.clone(),
            harness_config,
            profile,
            results,
            registry,
            model_factory,
        )?;
        let mut harness = harness;
        if let Some(opened) = &session {
            images.bind_session_file(opened.store.path());
            if opened.resumed {
                harness
                    .restore_session(opened.state.clone())
                    .map_err(|error| format!("cannot restore session: {error}"))?;
            }
        }
        let thread_id = session
            .as_ref()
            .map(|opened| ThreadId::new(opened.store.thread_id().to_string()))
            .unwrap_or_else(|| ThreadId::new("default"));
        let mut thread = Thread::new(thread_id.clone(), harness);
        if let Some(opened) = &session {
            thread.set_next_turn_number(opened.store.thread_turn_count() as u64 + 1);
        }
        let server = AppServer::new_with_control(
            ThreadStart::new(thread_id.clone()),
            thread,
            control.clone(),
        );
        let workflow_service = WorkflowService::new(
            session
                .as_ref()
                .and_then(|opened| opened.store.path().parent().map(PathBuf::from))
                .unwrap_or_else(|| world.workspace().to_path_buf()),
            goal_limits,
        );
        let management = RuntimeManagementService::new(
            server.clone(),
            session,
            world,
            enabled_mcp_servers,
            mcp_tool_count,
            retry_mcp_servers,
            approval.clone(),
        );
        let services = RuntimeServices::new(management, workflow_service)
            .map_err(|error| format!("cannot bind runtime services: {error}"))?;
        let connection = AppServerConnection::with_capability_manifest(
            server.clone(),
            capability_manifest_to_protocol(&capability_manifest),
        )
        .with_runtime_services(services);
        let mut client = LocalAppServerClient::with_control(connection, control.clone());
        client
            .initialize_with_profile(
                "mini-agent-cli",
                env!("CARGO_PKG_VERSION"),
                Some(capability_manifest.profile.clone()),
            )
            .await
            .map_err(|error| format!("cannot initialize app server: {}", error.message))?;
        Ok(AppServerRuntime {
            client,
            images,
            model_name,
            stable_system_prompt,
            capability_manifest,
        })
    }
}

impl<M: Model + Send + 'static> AppServerRuntime<M> {
    pub fn client_mut(&mut self) -> &mut LocalAppServerClient<M> {
        &mut self.client
    }

    pub fn into_server(self) -> AppServer<M> {
        self.client.into_server()
    }

    pub fn into_connection(self) -> AppServerConnection<M> {
        self.client.into_connection()
    }

    pub fn images(&self) -> &ImageStore {
        &self.images
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub async fn thread_id(&self) -> ThreadId {
        self.client.thread_id().await
    }

    pub fn stable_system_prompt(&self) -> &str {
        &self.stable_system_prompt
    }

    pub fn capability_manifest(&self) -> &CapabilityManifest {
        &self.capability_manifest
    }

    pub async fn checkpoint_seq(&self) -> Option<u64> {
        self.client.checkpoint_seq().await
    }
}

pub fn capability_manifest_to_protocol(
    manifest: &mini_agent_host::CapabilityManifest,
) -> ProtocolCapabilityManifest {
    ProtocolCapabilityManifest {
        profile: manifest.profile.clone(),
        model_provider: manifest.model_provider.clone(),
        tool_provider: manifest.tool_provider.clone(),
        extension_provider: manifest.extension_provider.clone(),
        policy_provider: manifest.policy_provider.clone(),
        enabled: manifest.enabled.clone(),
        disabled: manifest
            .disabled
            .iter()
            .map(|(name, reason)| DisabledCapability {
                name: name.clone(),
                reason: reason.clone(),
            })
            .collect(),
        extension_depth: serialized_name(manifest.extension_depth),
        selected_extensions: manifest.selected_extensions.clone(),
        prompt_sources: manifest.prompt_sources.clone(),
        rule_sources: manifest.rule_sources.clone(),
        rule_source_status: manifest
            .rule_source_status
            .iter()
            .map(|status| ProtocolRuleSourceStatus {
                source: status.source.clone(),
                state: serialized_name(status.state),
                reason: status.reason.clone(),
            })
            .collect(),
        prompt_source_fingerprints: manifest
            .prompt_source_fingerprints
            .iter()
            .map(|source| ProtocolSourceFingerprint {
                source: source.source.clone(),
                fingerprint: source.fingerprint.clone(),
            })
            .collect(),
        rule_source_fingerprints: manifest
            .rule_source_fingerprints
            .iter()
            .map(|source| ProtocolSourceFingerprint {
                source: source.source.clone(),
                fingerprint: source.fingerprint.clone(),
            })
            .collect(),
        prompt_rule_precedence: manifest.prompt_rule_precedence.clone(),
        rule_resolution: manifest.rule_resolution.clone(),
        rule_conflicts: manifest.rule_conflicts.clone(),
        rule_policy: ProtocolRulePolicy {
            workspace_write: manifest.rule_policy.workspace_write,
            shell_execution: manifest.rule_policy.shell_execution,
            process_execution: manifest.rule_policy.process_execution,
            workflow_scope: serialized_name(manifest.rule_policy.workflow_scope),
        },
        context_limits: ProtocolContextLimits {
            max_context_bytes: manifest.context_limits.max_context_bytes,
            max_context_item_bytes: manifest.context_limits.max_context_item_bytes,
            max_user_input_bytes: manifest.context_limits.max_user_input_bytes,
            max_model_response_bytes: manifest.context_limits.max_model_response_bytes,
            max_tool_output_bytes: manifest.context_limits.max_tool_output_bytes,
            max_tool_calls_per_step: manifest.context_limits.max_tool_calls_per_step,
        },
        sandbox: manifest.sandbox.clone(),
        security: manifest.security.clone(),
    }
}

fn serialized_name<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .expect("enum value is serializable")
        .as_str()
        .unwrap_or("unknown")
        .to_string()
}
