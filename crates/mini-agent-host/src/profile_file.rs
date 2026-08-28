use serde::Deserialize;
use std::fs;
use std::path::Path;

use super::{
    AgentKind, ExtensionLoadDepth, ExtensionSelection, PersonaKind, PromptSources, RuleSources,
    RuntimeProfile, ToolScope, WorkflowScope,
};
use crate::sandbox::SandboxKind;
use crate::security::SecurityPreset;

const PROFILE_FILE: &str = ".agents/profile.json";
const MAX_PROFILE_FILE_BYTES: usize = 16 * 1024;
const MAX_PROFILE_NAME_BYTES: usize = 64;
const MAX_SELECTED_EXTENSIONS: usize = 32;
const MAX_EXTENSION_NAME_BYTES: usize = 128;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProfileFile {
    name: Option<String>,
    model_provider: Option<String>,
    tools: Option<ToolScope>,
    extension_depth: Option<ExtensionLoadDepth>,
    selected_extensions: Option<Vec<String>>,
    agent: Option<AgentKind>,
    persona: Option<PersonaKind>,
    workflows: Option<WorkflowScope>,
    prompt_sources: Option<PromptSources>,
    rule_sources: Option<RuleSources>,
    sandbox: Option<String>,
    security: Option<String>,
}

/// Loads an optional, bounded workspace profile over a caller-provided base.
///
/// The file contains only allowlisted provider/enum selections, source
/// switches, and extension names. It cannot provide credentials, arbitrary
/// prompt text, commands, or filesystem paths. Callers apply explicit
/// command-line deny overrides after this function returns.
pub fn load_workspace_profile(
    workspace: &Path,
    mut base: RuntimeProfile,
) -> Result<RuntimeProfile, String> {
    let path = workspace.join(PROFILE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(base),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if bytes.len() > MAX_PROFILE_FILE_BYTES {
        return Err(format!(
            "{} exceeds {MAX_PROFILE_FILE_BYTES} bytes",
            path.display()
        ));
    }
    let file: ProfileFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    if let Some(name) = file.name {
        validate_profile_name(&name)?;
        base.name = name;
    }
    if let Some(provider) = file.model_provider {
        validate_model_provider(&provider)?;
        base.model_provider = provider;
    }
    if let Some(tools) = file.tools {
        base.tools = tools;
    }
    let has_extension_depth = file.extension_depth.is_some();
    if let Some(extension_depth) = file.extension_depth {
        base.extensions = extension_depth;
    }
    if let Some(names) = file.selected_extensions {
        validate_extension_names(&names)?;
        base.extension_selection = ExtensionSelection::Named(names);
        if !has_extension_depth {
            base.extensions = ExtensionLoadDepth::Selected;
        }
    }
    if let Some(agent) = file.agent {
        base.agent = agent;
    }
    if let Some(persona) = file.persona {
        base.persona = persona;
    }
    if let Some(workflows) = file.workflows {
        base.workflows = workflows;
    }
    if let Some(prompts) = file.prompt_sources {
        base.regular_agent.prompts = prompts;
    }
    if let Some(rules) = file.rule_sources {
        base.regular_agent.rules = rules;
    }
    if let Some(sandbox) = file.sandbox {
        base.sandbox = SandboxKind::parse(&sandbox)?;
    }
    if let Some(security) = file.security {
        base.security = SecurityPreset::parse(&security)?;
    }
    Ok(base)
}

fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_PROFILE_NAME_BYTES {
        return Err(format!(
            "profile name must be 1..={MAX_PROFILE_NAME_BYTES} bytes"
        ));
    }
    if name.contains('/') || name.contains('\\') || name.chars().any(char::is_control) {
        return Err("profile name contains an unsafe character".to_string());
    }
    Ok(())
}

fn validate_extension_names(names: &[String]) -> Result<(), String> {
    if names.len() > MAX_SELECTED_EXTENSIONS {
        return Err(format!(
            "selectedExtensions exceeds {MAX_SELECTED_EXTENSIONS} entries"
        ));
    }
    for name in names {
        if name.is_empty() || name.len() > MAX_EXTENSION_NAME_BYTES {
            return Err(format!(
                "extension name must be 1..={MAX_EXTENSION_NAME_BYTES} bytes"
            ));
        }
        if name.contains('\\') || name.split('/').any(|part| part == "..") {
            return Err(format!(
                "extension name {name:?} contains an unsafe path component"
            ));
        }
    }
    Ok(())
}

fn validate_model_provider(provider: &str) -> Result<(), String> {
    if mini_agent_capabilities::CapabilityRegistry::builtin().contains_model(provider) {
        Ok(())
    } else {
        Err(format!("unknown model provider {provider:?}"))
    }
}
