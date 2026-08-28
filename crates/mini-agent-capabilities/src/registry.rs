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

const BUILTIN_DESCRIPTORS: [CapabilityDescriptor; 1] = [CapabilityDescriptor {
    id: crate::OPENAI_MODEL_PROVIDER,
    kind: CapabilityKind::Model,
    description: "OpenAI-compatible Responses model provider",
}];

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
        self.descriptors().iter().any(|descriptor| {
            descriptor.kind == CapabilityKind::Model && descriptor.id == provider_id
        })
    }

    /// Builds the selected built-in tool provider without exposing its
    /// concrete workspace, process, web, or subagent implementations to Host.
    pub fn build_tools(
        self,
        workspace: PathBuf,
        approval: ApprovalController,
        extra_read_roots: Vec<PathBuf>,
        sandbox: SandboxKind,
        images: ImageStore,
        results: ResultStore,
    ) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        crate::workspace::workspace_tools_with_read_roots_and_results(
            workspace,
            approval,
            extra_read_roots,
            sandbox,
            images,
            results,
        )
    }

    /// Discovers the selected extension provider inputs once for a runtime.
    pub fn discover_extensions(self, workspace: &Path) -> skills::Discovery {
        skills::discover(workspace)
    }

    /// Starts selected MCP provider entries after Host policy has resolved the
    /// approval controller.
    pub fn load_mcp(
        self,
        servers: &[skills::McpServerConfig],
        approval: ApprovalController,
    ) -> crate::mcp::LoadResult {
        crate::mcp::load(servers, approval)
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
