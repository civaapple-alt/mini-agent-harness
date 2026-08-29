use mini_agent_core::HarnessConfig;
use serde::Serialize;

use super::WorkflowScope;
use super::{
    AgentKind, ExtensionLoadDepth, ExtensionSelection, PersonaKind, RuntimeProfile, ToolScope,
};
use mini_agent_capabilities::SecurityPreset;

const PROMPT_RULE_PRECEDENCE: [&str; 7] = [
    "core-safety",
    "host-policy",
    "agent",
    "persona",
    "workflows",
    "project",
    "extensions",
];

/// The effective typed policy derived from a runtime profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePolicy {
    pub workspace_write: bool,
    pub shell_execution: bool,
    pub process_execution: bool,
    pub workflow_scope: WorkflowScope,
}

/// Resolution state for one bounded rule source in the profile precedence
/// chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSourceStatus {
    pub source: String,
    pub state: RuleSourceState,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleSourceState {
    Active,
    Shadowed,
    Disabled,
}

/// A deterministic, bounded fingerprint for a source admitted to the
/// resolved prompt or rule set. The fingerprint is diagnostic only; source
/// contents and credentials never cross the service boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFingerprint {
    pub source: String,
    pub fingerprint: String,
}

pub(crate) fn stable_fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3_u64);
    }
    format!("{hash:016x}")
}

impl RuntimeProfile {
    pub fn rule_policy(&self) -> RulePolicy {
        let read_only = self.agent.is_read_only();
        RulePolicy {
            workspace_write: self.tools == ToolScope::All && !read_only,
            shell_execution: self.tools == ToolScope::All && !read_only,
            process_execution: self.tools == ToolScope::All && !read_only,
            workflow_scope: self.workflows,
        }
    }

    pub fn manifest(&self) -> CapabilityManifest {
        let mut enabled = vec!["model".to_string(), "builtin-prompt".to_string()];
        let mut disabled = Vec::new();
        if self.tools == ToolScope::All {
            enabled.push("workspace".to_string());
            enabled.push("web".to_string());
            enabled.push("image".to_string());
            if self.agent.is_read_only() {
                disabled.push(("shell".to_string(), "agent scope: read-only".to_string()));
                disabled.push(("process".to_string(), "agent scope: read-only".to_string()));
                disabled.push((
                    "workspace-write".to_string(),
                    "agent scope: read-only".to_string(),
                ));
            } else {
                enabled.push("shell".to_string());
                enabled.push("process".to_string());
                enabled.push("workspace-write".to_string());
            }
        } else {
            disabled.push(("tools".to_string(), "profile scope: no-tools".to_string()));
        }
        if self.extensions == ExtensionLoadDepth::None {
            disabled.push((
                "extensions".to_string(),
                "profile extension depth: none".to_string(),
            ));
        } else {
            enabled.push("extensions".to_string());
        }
        let mut prompt_sources = vec!["builtin".to_string()];
        let mut rule_sources = Vec::new();
        if self.regular_agent.prompts.project {
            prompt_sources.push("project".to_string());
        }
        if self.regular_agent.prompts.extensions && self.extensions != ExtensionLoadDepth::None {
            prompt_sources.push("extensions".to_string());
        }
        if self.regular_agent.prompts.workflows && self.workflows != WorkflowScope::Disabled {
            prompt_sources.push("workflows".to_string());
        }
        if self.regular_agent.rules.project {
            rule_sources.push("project".to_string());
        }
        if self.regular_agent.rules.extensions && self.extensions != ExtensionLoadDepth::None {
            rule_sources.push("extensions".to_string());
        }
        if self.workflows != WorkflowScope::Disabled {
            enabled.push("workflows".to_string());
            if self.regular_agent.rules.workflows {
                rule_sources.push("workflows".to_string());
            }
        } else {
            disabled.push((
                "workflows".to_string(),
                "profile workflow scope: disabled".to_string(),
            ));
        }
        let mut rule_conflicts = Vec::new();
        if self.regular_agent.rules.extensions && self.extensions == ExtensionLoadDepth::None {
            rule_conflicts
                .push("extensions rule source shadowed by extension depth none".to_string());
        }
        if self.regular_agent.rules.workflows && self.workflows == WorkflowScope::Disabled {
            rule_conflicts.push("workflow rules shadowed by disabled workflow scope".to_string());
        }
        if self.agent.is_read_only() && self.security == SecurityPreset::Turbomode {
            rule_conflicts
                .push("turbomode write allowance shadowed by read-only agent scope".to_string());
        }
        let mut rule_source_status = vec![
            RuleSourceStatus {
                source: "core-safety".to_string(),
                state: RuleSourceState::Active,
                reason: None,
            },
            RuleSourceStatus {
                source: "host-policy".to_string(),
                state: RuleSourceState::Active,
                reason: None,
            },
            RuleSourceStatus {
                source: "agent".to_string(),
                state: RuleSourceState::Active,
                reason: None,
            },
            RuleSourceStatus {
                source: "persona".to_string(),
                state: if self.persona == PersonaKind::None {
                    RuleSourceState::Disabled
                } else {
                    RuleSourceState::Active
                },
                reason: (self.persona == PersonaKind::None)
                    .then(|| "no persona selected".to_string()),
            },
        ];
        rule_source_status.push(RuleSourceStatus {
            source: "workflows".to_string(),
            state: if self.workflows == WorkflowScope::Disabled
                || !self.regular_agent.rules.workflows
            {
                RuleSourceState::Disabled
            } else {
                RuleSourceState::Active
            },
            reason: if self.workflows == WorkflowScope::Disabled {
                Some("workflow scope disabled".to_string())
            } else if !self.regular_agent.rules.workflows {
                Some("workflow rule source disabled".to_string())
            } else {
                None
            },
        });
        rule_source_status.push(RuleSourceStatus {
            source: "project".to_string(),
            state: if self.regular_agent.rules.project {
                RuleSourceState::Active
            } else {
                RuleSourceState::Disabled
            },
            reason: (!self.regular_agent.rules.project)
                .then(|| "project rule source disabled".to_string()),
        });
        rule_source_status.push(RuleSourceStatus {
            source: "extensions".to_string(),
            state: if !self.regular_agent.rules.extensions {
                RuleSourceState::Disabled
            } else if self.extensions == ExtensionLoadDepth::None {
                RuleSourceState::Shadowed
            } else {
                RuleSourceState::Active
            },
            reason: if !self.regular_agent.rules.extensions {
                Some("extension rule source disabled".to_string())
            } else if self.extensions == ExtensionLoadDepth::None {
                Some("extension depth none".to_string())
            } else {
                None
            },
        });
        let mut prompt_source_fingerprints = Vec::new();
        if self.agent != AgentKind::General {
            let agent_prompt = match self.agent {
                AgentKind::Explore => {
                    mini_agent_capabilities::AgentPromptKind::Explore.prompt_template()
                }
                AgentKind::Plan => mini_agent_capabilities::AgentPromptKind::Plan.prompt_template(),
                AgentKind::General => unreachable!(),
            };
            prompt_source_fingerprints.push(SourceFingerprint {
                source: "agent".to_string(),
                fingerprint: stable_fingerprint(agent_prompt.as_bytes()),
            });
        }
        if self.persona != PersonaKind::None {
            let persona_prompt = match self.persona {
                PersonaKind::Reviewer => {
                    mini_agent_capabilities::PersonaPromptKind::Reviewer.prompt_template()
                }
                PersonaKind::Implementer => {
                    mini_agent_capabilities::PersonaPromptKind::Implementer.prompt_template()
                }
                PersonaKind::Researcher => {
                    mini_agent_capabilities::PersonaPromptKind::Researcher.prompt_template()
                }
                PersonaKind::None => unreachable!(),
            };
            prompt_source_fingerprints.push(SourceFingerprint {
                source: "persona".to_string(),
                fingerprint: stable_fingerprint(persona_prompt.as_bytes()),
            });
        }
        let rule_policy_fingerprint =
            serde_json::to_vec(&self.rule_policy()).expect("rule policy is serializable");
        let rule_source_fingerprints = vec![SourceFingerprint {
            source: "host-policy".to_string(),
            fingerprint: stable_fingerprint(&rule_policy_fingerprint),
        }];
        CapabilityManifest {
            profile: self.name.clone(),
            model_provider: self.model_provider.clone(),
            tool_provider: self.tool_provider.clone(),
            extension_provider: self.extension_provider.clone(),
            policy_provider: self.policy_provider.clone(),
            enabled,
            disabled,
            extension_depth: self.extensions,
            selected_extensions: match &self.extension_selection {
                ExtensionSelection::All => Vec::new(),
                ExtensionSelection::Named(names) => names.clone(),
            },
            prompt_sources,
            rule_sources,
            rule_source_status,
            prompt_source_fingerprints,
            rule_source_fingerprints,
            prompt_rule_precedence: PROMPT_RULE_PRECEDENCE
                .iter()
                .map(|source| (*source).to_string())
                .collect(),
            rule_resolution: "typed-agent-scope".to_string(),
            rule_conflicts,
            rule_policy: self.rule_policy(),
            context_limits: ContextLimits::default(),
            sandbox: self.sandbox.name().to_string(),
            security: self.security.name().to_string(),
        }
    }

    /// Produces a manifest using the actual Harness limits selected by the
    /// caller rather than the default diagnostic limits.
    pub fn manifest_with_config(&self, config: &HarnessConfig) -> CapabilityManifest {
        let mut manifest = self.manifest();
        manifest.context_limits = ContextLimits::from(config);
        manifest.prompt_source_fingerprints.push(SourceFingerprint {
            source: "builtin".to_string(),
            fingerprint: stable_fingerprint(config.system_prompt.as_bytes()),
        });
        manifest
    }
}

/// Bounded, secret-free description of the effective runtime capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    pub profile: String,
    pub model_provider: String,
    pub tool_provider: String,
    pub extension_provider: String,
    pub policy_provider: String,
    pub enabled: Vec<String>,
    pub disabled: Vec<(String, String)>,
    pub extension_depth: ExtensionLoadDepth,
    pub selected_extensions: Vec<String>,
    pub prompt_sources: Vec<String>,
    pub rule_sources: Vec<String>,
    pub rule_source_status: Vec<RuleSourceStatus>,
    pub prompt_source_fingerprints: Vec<SourceFingerprint>,
    pub rule_source_fingerprints: Vec<SourceFingerprint>,
    pub prompt_rule_precedence: Vec<String>,
    pub rule_resolution: String,
    pub rule_conflicts: Vec<String>,
    pub rule_policy: RulePolicy,
    pub context_limits: ContextLimits,
    pub sandbox: String,
    pub security: String,
}

/// Bounded context limits visible to clients without exposing prompt content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLimits {
    pub max_context_bytes: usize,
    pub max_context_item_bytes: usize,
    pub max_user_input_bytes: usize,
    pub max_model_response_bytes: usize,
    pub max_tool_output_bytes: usize,
    pub max_tool_calls_per_step: usize,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self::from(&HarnessConfig::default())
    }
}

impl From<&HarnessConfig> for ContextLimits {
    fn from(config: &HarnessConfig) -> Self {
        Self {
            max_context_bytes: config.max_context_bytes,
            max_context_item_bytes: config.max_context_item_bytes,
            max_user_input_bytes: config.max_user_input_bytes,
            max_model_response_bytes: config.max_model_response_bytes,
            max_tool_output_bytes: config.max_tool_output_bytes,
            max_tool_calls_per_step: config.max_tool_calls_per_step,
        }
    }
}
