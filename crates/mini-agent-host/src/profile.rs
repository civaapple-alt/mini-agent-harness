//! Declarative capability profiles used to assemble a host runtime.

use serde::Deserialize;
use serde::Serialize;

use mini_agent_capabilities::sandbox::SandboxKind;
use mini_agent_capabilities::security::SecurityPreset;

#[path = "profile_file.rs"]
mod profile_file;
#[path = "profile_manifest.rs"]
mod profile_manifest;

pub use profile_file::load_workspace_profile;
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
/// typed base-prompt, output-contract, and context-policy presets without
/// putting arbitrary prompt text in a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RegularAgentConfig {
    pub prompts: PromptSources,
    pub rules: RuleSources,
}

/// Compatibility name retained for callers that used the original combined
/// prompt/rule settings before they were assigned to the regular agent.
pub type PromptRulePolicy = RegularAgentConfig;

/// Declarative selections used by a frontend before host composition starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProfile {
    pub name: String,
    /// Bounded identifier resolved by the capability provider registry.
    pub model_provider: String,
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

impl RuntimeProfile {
    /// Resolves only the profiles intentionally exposed at an application
    /// edge. Workspace files may further narrow these selections, but cannot
    /// introduce a new wire-visible profile name.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "interactive" => Some(Self::interactive_default()),
            "ask" => Some(Self::ask_default()),
            "auto" => Some(Self::auto_default()),
            "acp" => Some(Self::acp_default()),
            "acp-minimal" => Some(Self::acp_minimal()),
            "demo" => Some(Self::demo()),
            _ => None,
        }
    }

    pub fn interactive_default() -> Self {
        Self::named("interactive", ToolScope::All, ExtensionLoadDepth::Enabled)
    }

    pub fn ask_default() -> Self {
        Self::named("ask", ToolScope::All, ExtensionLoadDepth::Enabled)
    }

    pub fn auto_default() -> Self {
        Self::named("auto", ToolScope::All, ExtensionLoadDepth::Enabled)
    }

    pub fn acp_default() -> Self {
        Self::named("acp", ToolScope::All, ExtensionLoadDepth::Selected)
    }

    pub fn acp_minimal() -> Self {
        let mut profile = Self::named("acp-minimal", ToolScope::None, ExtensionLoadDepth::None);
        profile.workflows = WorkflowScope::Disabled;
        profile.regular_agent.prompts.workflows = false;
        profile.regular_agent.rules.workflows = false;
        profile
    }

    pub fn demo() -> Self {
        let mut profile = Self::named("demo", ToolScope::All, ExtensionLoadDepth::None);
        profile.workflows = WorkflowScope::Disabled;
        profile.regular_agent.prompts.workflows = false;
        profile.regular_agent.rules.workflows = false;
        profile
    }

    pub fn without_tools(mut self) -> Self {
        self.tools = ToolScope::None;
        self.extensions = ExtensionLoadDepth::None;
        self.extension_selection = ExtensionSelection::All;
        self.name.push_str("-no-tools");
        self
    }

    pub fn without_workflows(mut self) -> Self {
        self.workflows = WorkflowScope::Disabled;
        self.regular_agent.prompts.workflows = false;
        self.regular_agent.rules.workflows = false;
        self.name.push_str("-no-workflows");
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

    /// Replaces the prompt-source selection for a regular agent.
    pub fn with_prompt_sources(mut self, prompts: PromptSources) -> Self {
        self.regular_agent.prompts = prompts;
        self
    }

    /// Replaces the rule-source selection for a regular agent.
    pub fn with_rule_sources(mut self, rules: RuleSources) -> Self {
        self.regular_agent.rules = rules;
        self
    }

    /// Restricts metadata discovery to named skills, plugins, or MCP servers.
    pub fn with_selected_extensions<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extensions = ExtensionLoadDepth::Selected;
        self.extension_selection =
            ExtensionSelection::Named(names.into_iter().map(Into::into).collect());
        self
    }

    /// Enables only the named extensions after policy and approval checks.
    pub fn with_enabled_extensions<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extensions = ExtensionLoadDepth::Enabled;
        self.extension_selection =
            ExtensionSelection::Named(names.into_iter().map(Into::into).collect());
        self
    }

    /// Renders only explicit non-default role overlays for the stable prompt.
    /// The ordinary general agent keeps the existing core prompt unchanged;
    /// selected foundations and personas add their bounded contract once.
    pub fn prompt_overlay(&self) -> String {
        let mut sections = Vec::new();
        if self.agent != AgentKind::General {
            let agent = match self.agent {
                AgentKind::Explore => mini_agent_capabilities::persona::AgentPromptKind::Explore,
                AgentKind::Plan => mini_agent_capabilities::persona::AgentPromptKind::Plan,
                AgentKind::General => unreachable!(),
            };
            sections.push(agent.prompt_template().to_string());
        }
        if self.persona != PersonaKind::None {
            let persona = match self.persona {
                PersonaKind::Reviewer => {
                    mini_agent_capabilities::persona::PersonaPromptKind::Reviewer
                }
                PersonaKind::Implementer => {
                    mini_agent_capabilities::persona::PersonaPromptKind::Implementer
                }
                PersonaKind::Researcher => {
                    mini_agent_capabilities::persona::PersonaPromptKind::Researcher
                }
                PersonaKind::None => unreachable!(),
            };
            sections.push(persona.prompt_template(None, None));
        }
        sections.join("\n\n")
    }

    fn named(name: &str, tools: ToolScope, extensions: ExtensionLoadDepth) -> Self {
        Self {
            name: name.to_string(),
            model_provider: mini_agent_capabilities::OPENAI_MODEL_PROVIDER.to_string(),
            tools,
            extensions,
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

impl Default for RuntimeProfile {
    fn default() -> Self {
        Self::interactive_default()
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
