//! Declarative runtime composition used to assemble a host runtime.

use serde::Deserialize;
use serde::Serialize;

use mini_agent_capabilities::SandboxKind;
use mini_agent_capabilities::SecurityPreset;

#[path = "profile_manifest.rs"]
mod profile_manifest;

pub use profile_manifest::CapabilityManifest;
pub use profile_manifest::ContextLimits;
pub use profile_manifest::RulePolicy;
pub use profile_manifest::RuleSourceState;
pub use profile_manifest::RuleSourceStatus;
pub use profile_manifest::SourceFingerprint;
pub(crate) use profile_manifest::stable_fingerprint;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolScope {
    All,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionLoadDepth {
    None,
    Metadata,
    Selected,
    Enabled,
}

/// Limits extension discovery to all configured entries or to named entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionSelection {
    All,
    Named(Vec<String>),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentKind {
    Explore,
    Plan,
    General,
}

impl AgentKind {
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Explore | Self::Plan)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonaKind {
    None,
    Reviewer,
    Implementer,
    Researcher,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowScope {
    Disabled,
    Plan,
    Goal,
    PlanAndGoal,
}

/// Selects bounded prompt sources for the regular agent.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PromptSources {
    pub project: bool,
    pub extensions: bool,
    pub workflows: bool,
}

impl Default for PromptSources {
    fn default() -> Self {
        Self {
            project: true,
            extensions: true,
            workflows: true,
        }
    }
}

/// Selects typed rule sources for the regular agent.
///
/// Rules are policy inputs rather than extra hidden system prompts. They are
/// resolved after the core and host safety rules and can only narrow the
/// effective scope.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuleSources {
    pub project: bool,
    pub extensions: bool,
    pub workflows: bool,
}

impl Default for RuleSources {
    fn default() -> Self {
        Self {
            project: true,
            extensions: true,
            workflows: true,
        }
    }
}

/// Prompt and rule source selections owned by the regular `general` agent.
///
/// Foundational agents and personas compose with this configuration; they do
/// not replace it. Keeping the settings in a named structure leaves room for
/// typed base-prompt, output-contract, and context-policy selections without
/// putting arbitrary prompt text in the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RegularAgentConfig {
    pub prompts: PromptSources,
    pub rules: RuleSources,
}

/// Declarative selections used by a frontend before host composition starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeComposition {
    /// Bounded identifier resolved by the capability provider registry.
    pub model_provider: String,
    /// Bounded identifier for the tool provider selected by this composition.
    pub tool_provider: String,
    /// Bounded identifier for the extension provider selected by this composition.
    pub extension_provider: String,
    /// Bounded identifier for the sandbox, security, and approval provider.
    pub policy_provider: String,
    pub tools: ToolScope,
    pub extensions: ExtensionLoadDepth,
    pub extension_selection: ExtensionSelection,
    pub agent: AgentKind,
    pub persona: PersonaKind,
    pub workflows: WorkflowScope,
    pub regular_agent: RegularAgentConfig,
    pub sandbox: SandboxKind,
    pub security: SecurityPreset,
}

impl RuntimeComposition {
    pub fn without_tools(mut self) -> Self {
        self.tools = ToolScope::None;
        self.extensions = ExtensionLoadDepth::None;
        self.extension_selection = ExtensionSelection::All;
        self
    }

    pub fn with_sandbox(mut self, sandbox: SandboxKind) -> Self {
        self.sandbox = sandbox;
        self
    }

    pub fn with_security(mut self, security: SecurityPreset) -> Self {
        self.security = security;
        self
    }

    /// Selects an allowlisted model provider by stable identifier.
    pub fn with_model_provider(mut self, provider: impl Into<String>) -> Self {
        self.model_provider = provider.into();
        self
    }

    /// Renders only explicit non-default role overlays for the stable prompt.
    /// The ordinary general agent keeps the existing core prompt unchanged;
    /// selected foundations and personas add their bounded contract once.
    pub fn prompt_overlay(&self) -> String {
        let mut sections = Vec::new();
        if self.agent != AgentKind::General {
            let agent = match self.agent {
                AgentKind::Explore => mini_agent_capabilities::AgentPromptKind::Explore,
                AgentKind::Plan => mini_agent_capabilities::AgentPromptKind::Plan,
                AgentKind::General => unreachable!(),
            };
            sections.push(agent.prompt_template().to_string());
        }
        if self.persona != PersonaKind::None {
            let persona = match self.persona {
                PersonaKind::Reviewer => mini_agent_capabilities::PersonaPromptKind::Reviewer,
                PersonaKind::Implementer => mini_agent_capabilities::PersonaPromptKind::Implementer,
                PersonaKind::Researcher => mini_agent_capabilities::PersonaPromptKind::Researcher,
                PersonaKind::None => unreachable!(),
            };
            sections.push(persona.prompt_template().to_string());
        }
        sections.join("\n\n")
    }
}

impl Default for RuntimeComposition {
    fn default() -> Self {
        Self {
            model_provider: mini_agent_capabilities::OPENAI_MODEL_PROVIDER.to_string(),
            tool_provider: mini_agent_capabilities::BUILTIN_TOOL_PROVIDER.to_string(),
            extension_provider: mini_agent_capabilities::BUILTIN_EXTENSION_PROVIDER.to_string(),
            policy_provider: mini_agent_capabilities::BUILTIN_POLICY_PROVIDER.to_string(),
            tools: ToolScope::All,
            extensions: ExtensionLoadDepth::Enabled,
            extension_selection: ExtensionSelection::All,
            agent: AgentKind::General,
            persona: PersonaKind::None,
            workflows: WorkflowScope::PlanAndGoal,
            regular_agent: RegularAgentConfig::default(),
            sandbox: SandboxKind::Native,
            security: SecurityPreset::Default,
        }
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
