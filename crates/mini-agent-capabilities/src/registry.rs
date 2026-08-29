use crate::ImageStore;
use crate::SandboxKind;
use crate::result_store::ResultStore;
use crate::security::{SecurityPolicy, SecurityPreset};
use crate::skills;
use crate::workspace::ApprovalController;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolError;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) fn stable_fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3_u64);
    }
    format!("{hash:016x}")
}

/// The category of a concrete capability provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    Model,
    Tool,
    Extension,
    Policy,
}

/// Bounded metadata exposed to Host and service clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: &'static str,
    pub kind: CapabilityKind,
    pub description: &'static str,
}

/// Inputs required to assemble a selected tool provider.
pub struct ToolBuildRequest {
    pub provider_id: String,
    pub workspace: PathBuf,
    pub approval: ApprovalController,
    pub extra_read_roots: Vec<PathBuf>,
    pub sandbox: SandboxKind,
    pub images: ImageStore,
    pub results: ResultStore,
}

/// A host-embedded tool provider.
///
/// Implementations own their concrete tool constructors and may capture
/// application resources in the provider value. The host only sees the
/// bounded descriptor and passes runtime-scoped inputs through
/// [`ToolBuildRequest`]. Providers must not execute tools while building them.
pub trait ToolProvider: Send + Sync {
    fn descriptor(&self) -> CapabilityDescriptor;

    fn build_tools(&self, request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError>;
}

const BUILTIN_DESCRIPTORS: [CapabilityDescriptor; 4] = [
    CapabilityDescriptor {
        id: crate::OPENAI_MODEL_PROVIDER,
        kind: CapabilityKind::Model,
        description: "OpenAI-compatible Responses model provider",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_TOOL_PROVIDER,
        kind: CapabilityKind::Tool,
        description: "Built-in workspace, process, web, and image tools",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_EXTENSION_PROVIDER,
        kind: CapabilityKind::Extension,
        description: "Built-in skill, plugin, and MCP extensions",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_POLICY_PROVIDER,
        kind: CapabilityKind::Policy,
        description: "Built-in sandbox, security, and approval policy",
    },
];

struct BuiltinToolProvider;

impl ToolProvider for BuiltinToolProvider {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: crate::BUILTIN_TOOL_PROVIDER,
            kind: CapabilityKind::Tool,
            description: "Built-in workspace, process, web, and image tools",
        }
    }

    fn build_tools(&self, request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        crate::workspace::workspace_tools_with_read_roots_and_results(
            request.workspace,
            request.approval,
            request.extra_read_roots,
            request.sandbox,
            request.images,
            request.results,
        )
    }
}

/// Registry of concrete providers available to a local Host.
///
/// The registry is intentionally data-only at the App Server boundary. A
/// profile selects stable IDs; provider construction and secrets stay local to
/// the capabilities crate. External providers are registered by an embedding
/// application rather than discovered from untrusted profile data.
#[derive(Clone)]
pub struct CapabilityRegistry {
    tool_providers: Arc<Vec<Arc<dyn ToolProvider>>>,
    model_providers: Arc<Vec<CapabilityDescriptor>>,
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl CapabilityRegistry {
    pub fn builtin() -> Self {
        Self {
            tool_providers: Arc::new(vec![Arc::new(BuiltinToolProvider)]),
            model_providers: Arc::new(Vec::new()),
        }
    }

    /// Returns a registry with an additional host-embedded tool provider.
    ///
    /// The provider ID is still validated against the profile selection before
    /// any workspace resources are opened.
    pub fn with_tool_provider(self, provider: Arc<dyn ToolProvider>) -> Self {
        let mut providers = (*self.tool_providers).clone();
        providers.push(provider);
        Self {
            tool_providers: Arc::new(providers),
            model_providers: self.model_providers,
        }
    }

    /// Returns a registry with an embedding application's model descriptor.
    ///
    /// Construction is supplied separately through the Host model factory;
    /// the registry only makes the stable provider ID selectable by a profile.
    pub fn with_model_provider(mut self, descriptor: CapabilityDescriptor) -> Self {
        assert_eq!(descriptor.kind, CapabilityKind::Model);
        let mut providers = (*self.model_providers).clone();
        if !providers
            .iter()
            .any(|registered| registered.id == descriptor.id)
        {
            providers.push(descriptor);
        }
        self.model_providers = Arc::new(providers);
        self
    }

    pub fn descriptors(&self) -> Vec<CapabilityDescriptor> {
        let mut descriptors = BUILTIN_DESCRIPTORS.to_vec();
        descriptors.extend(self.model_providers.iter().copied());
        for descriptor in self
            .tool_providers
            .iter()
            .map(|provider| provider.descriptor())
        {
            if descriptor.kind != CapabilityKind::Tool {
                continue;
            }
            if !descriptors.iter().any(|registered| {
                registered.kind == descriptor.kind && registered.id == descriptor.id
            }) {
                descriptors.push(descriptor);
            }
        }
        descriptors
    }

    /// Returns whether a stable provider ID is registered for a capability
    /// category.
    pub fn contains(&self, kind: CapabilityKind, provider_id: &str) -> bool {
        self.descriptors()
            .iter()
            .any(|descriptor| descriptor.kind == kind && descriptor.id == provider_id)
    }

    /// Validates a provider selection before any local resources are opened.
    pub fn validate(&self, kind: CapabilityKind, provider_id: &str) -> Result<(), String> {
        if self.contains(kind, provider_id) {
            Ok(())
        } else {
            Err(format!("unknown {:?} provider `{provider_id}`", kind))
        }
    }

    /// Builds the selected built-in tool provider without exposing its
    /// concrete workspace, process, web, or image implementations to Host.
    pub fn build_tools(&self, request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        if let Err(error) = self.validate(CapabilityKind::Tool, &request.provider_id) {
            return Err(ToolError(error));
        }
        self.tool_providers
            .iter()
            .find(|provider| {
                let descriptor = provider.descriptor();
                descriptor.kind == CapabilityKind::Tool && descriptor.id == request.provider_id
            })
            .expect("validated tool provider must be registered")
            .build_tools(request)
    }

    /// Builds the selected policy provider without owning the frontend's
    /// approval callback or transport-specific interaction.
    pub fn build_policy(
        &self,
        provider_id: &str,
        preset: SecurityPreset,
    ) -> Result<SecurityPolicy, String> {
        self.validate(CapabilityKind::Policy, provider_id)?;
        Ok(SecurityPolicy::for_preset(preset))
    }

    /// Discovers the selected extension provider inputs once for a runtime.
    pub fn discover_extensions(
        &self,
        provider_id: &str,
        workspace: &Path,
    ) -> Result<skills::Discovery, String> {
        self.validate(CapabilityKind::Extension, provider_id)?;
        Ok(skills::discover(workspace))
    }

    /// Starts selected MCP provider entries after Host policy has resolved the
    /// approval controller.
    pub fn load_mcp(
        &self,
        provider_id: &str,
        servers: &[skills::McpServerConfig],
        approval: ApprovalController,
    ) -> Result<crate::mcp::LoadResult, String> {
        self.validate(CapabilityKind::Extension, provider_id)?;
        Ok(crate::mcp::load(servers, approval))
    }
}
