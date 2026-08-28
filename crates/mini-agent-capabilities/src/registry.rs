use crate::ImageStore;
use crate::SandboxKind;
use crate::result_store::ResultStore;
use crate::skills;
use crate::workspace::ApprovalController;
use mini_agent_core::Tool;
use mini_agent_core::ToolError;
use std::path::Path;
use std::path::PathBuf;

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

const BUILTIN_DESCRIPTORS: [CapabilityDescriptor; 4] = [
    CapabilityDescriptor {
        id: crate::OPENAI_MODEL_PROVIDER,
        kind: CapabilityKind::Model,
        description: "OpenAI-compatible Responses model provider",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_TOOL_PROVIDER,
        kind: CapabilityKind::Tool,
        description: "Built-in workspace, process, web, image, and subagent tools",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_EXTENSION_PROVIDER,
        kind: CapabilityKind::Extension,
        description: "Built-in skill, plugin, marketplace, and MCP extensions",
    },
    CapabilityDescriptor {
        id: crate::BUILTIN_POLICY_PROVIDER,
        kind: CapabilityKind::Policy,
        description: "Built-in sandbox, security, and approval policy",
    },
];

/// Registry of concrete providers available to a local Host.
///
/// The registry is intentionally data-only at the App Server boundary. A
/// profile selects stable IDs; provider construction and secrets stay local to
/// the capabilities crate.
#[derive(Clone, Copy, Debug, Default)]
pub struct CapabilityRegistry;

impl CapabilityRegistry {
    pub fn builtin() -> Self {
        Self
    }

    pub fn descriptors(self) -> &'static [CapabilityDescriptor] {
        &BUILTIN_DESCRIPTORS
    }

    pub fn contains_model(self, provider_id: &str) -> bool {
        self.contains(CapabilityKind::Model, provider_id)
    }

    /// Returns whether a stable provider ID is registered for a capability
    /// category.
    pub fn contains(self, kind: CapabilityKind, provider_id: &str) -> bool {
        self.descriptors()
            .iter()
            .any(|descriptor| descriptor.kind == kind && descriptor.id == provider_id)
    }

    /// Validates a provider selection before any local resources are opened.
    pub fn validate(self, kind: CapabilityKind, provider_id: &str) -> Result<(), String> {
        if self.contains(kind, provider_id) {
            Ok(())
        } else {
            Err(format!("unknown {:?} provider `{provider_id}`", kind))
        }
    }

    /// Builds the selected built-in tool provider without exposing its
    /// concrete workspace, process, web, or subagent implementations to Host.
    pub fn build_tools(self, request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        if let Err(error) = self.validate(CapabilityKind::Tool, &request.provider_id) {
            return Err(ToolError(error));
        }
        crate::workspace::workspace_tools_with_read_roots_and_results(
            request.workspace,
            request.approval,
            request.extra_read_roots,
            request.sandbox,
            request.images,
            request.results,
        )
    }

    /// Discovers the selected extension provider inputs once for a runtime.
    pub fn discover_extensions(
        self,
        provider_id: &str,
        workspace: &Path,
    ) -> Result<skills::Discovery, String> {
        self.validate(CapabilityKind::Extension, provider_id)?;
        Ok(skills::discover(workspace))
    }

    /// Starts selected MCP provider entries after Host policy has resolved the
    /// approval controller.
    pub fn load_mcp(
        self,
        provider_id: &str,
        servers: &[skills::McpServerConfig],
        approval: ApprovalController,
    ) -> Result<crate::mcp::LoadResult, String> {
        self.validate(CapabilityKind::Extension, provider_id)?;
        Ok(crate::mcp::load(servers, approval))
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
