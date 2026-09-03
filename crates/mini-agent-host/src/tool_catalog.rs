use mini_agent_protocol::Tool;

/// Identifies the owner that supplied a model-callable tool.
// The complete origin vocabulary is staged before external providers are
// enabled by a later catalog batch.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOrigin {
    Builtin,
    Host,
    Mcp,
    Dynamic,
}

/// Controls whether a catalog entry can appear in a model request.
// Hidden and Disabled become active when per-Thread selection is introduced.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolExposure {
    Visible,
    Hidden,
    Disabled,
}

/// Coarse admission class used for catalog and diagnostics metadata.
///
/// The concrete `ToolHandler` remains authoritative for call-specific
/// validation and conditional approval requirements.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolAdmissionClass {
    ReadOnly,
    ApprovalRequired,
    Forbidden,
}

/// Stable Host-owned metadata for one tool name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCatalogEntry {
    pub name: &'static str,
    pub provider: &'static str,
    pub origin: ToolOrigin,
    pub exposure: ToolExposure,
    pub admission: ToolAdmissionClass,
}

/// A bounded, Thread-local selection of the model-visible Builtin tools.
///
/// The selection changes exposure only. Provider construction, approval, and
/// execution continue to use their existing Host/Core boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinToolSelection {
    names: Vec<String>,
}

const DEFAULT_BUILTIN_CATALOG: [ToolCatalogEntry; 6] = [
    ToolCatalogEntry {
        name: "read_file",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ReadOnly,
    },
    ToolCatalogEntry {
        name: "edit_file",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ApprovalRequired,
    },
    ToolCatalogEntry {
        name: "write_file",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ApprovalRequired,
    },
    ToolCatalogEntry {
        name: "shell",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ApprovalRequired,
    },
    ToolCatalogEntry {
        name: "web_fetch",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ReadOnly,
    },
    ToolCatalogEntry {
        name: "read_image",
        provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER,
        origin: ToolOrigin::Builtin,
        exposure: ToolExposure::Visible,
        admission: ToolAdmissionClass::ReadOnly,
    },
];

pub(crate) fn default_builtin_catalog() -> &'static [ToolCatalogEntry] {
    &DEFAULT_BUILTIN_CATALOG
}

impl BuiltinToolSelection {
    pub fn all() -> Self {
        Self {
            names: default_builtin_catalog()
                .iter()
                .filter(|entry| entry.exposure == ToolExposure::Visible)
                .map(|entry| entry.name.to_string())
                .collect(),
        }
    }

    pub fn from_names(names: Vec<String>) -> Result<Self, String> {
        if names.len() > default_builtin_catalog().len() {
            return Err(format!(
                "builtin tool selection has too many entries: {}",
                names.len()
            ));
        }
        for name in &names {
            if !default_builtin_catalog()
                .iter()
                .any(|entry| entry.name == name && entry.exposure == ToolExposure::Visible)
            {
                return Err(format!("unknown or unavailable builtin tool: {name}"));
            }
        }
        if names
            .iter()
            .any(|name| names.iter().filter(|candidate| *candidate == name).count() > 1)
        {
            return Err("builtin tool selection contains duplicates".to_string());
        }
        Ok(Self { names })
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn hidden_names(&self) -> Vec<String> {
        default_builtin_catalog()
            .iter()
            .filter(|entry| {
                entry.exposure == ToolExposure::Visible
                    && !self.names.iter().any(|name| name == entry.name)
            })
            .map(|entry| entry.name.to_string())
            .collect()
    }
}

impl Default for BuiltinToolSelection {
    fn default() -> Self {
        Self::all()
    }
}

pub(crate) fn retain_default_builtin_tools(tools: Vec<Box<dyn Tool>>) -> Vec<Box<dyn Tool>> {
    tools
        .into_iter()
        .filter(|tool| {
            let name = tool.spec().name;
            default_builtin_catalog()
                .iter()
                .any(|entry| entry.exposure == ToolExposure::Visible && entry.name == name)
        })
        .collect()
}
