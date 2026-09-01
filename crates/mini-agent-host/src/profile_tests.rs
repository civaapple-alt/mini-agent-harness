use super::*;
use crate::test_support::test_root;
use std::fs;

#[test]
fn default_profile_exposes_tools_and_prompt_rule_sources() {
    let profile = RuntimeProfile::interactive_default();
    let manifest = profile.manifest();

    assert_eq!(manifest.profile, "interactive");
    assert_eq!(manifest.model_provider, "openai");
    assert_eq!(manifest.tool_provider, "builtin");
    assert_eq!(manifest.extension_provider, "builtin");
    assert_eq!(manifest.policy_provider, "builtin");
    assert!(manifest.enabled.contains(&"workspace".to_string()));
    assert_eq!(
        manifest.prompt_sources,
        ["builtin", "project", "extensions", "workflows"]
    );
    assert_eq!(
        manifest.rule_sources,
        ["project", "extensions", "workflows"]
    );
    assert!(manifest.disabled.is_empty());
    assert_eq!(manifest.sandbox, "native");
    assert_eq!(manifest.security, "default");
    assert_eq!(manifest.rule_resolution, "typed-agent-scope");
    assert!(!manifest.prompt_rule_precedence.is_empty());
    assert_eq!(manifest.rule_source_status.len(), 7);
    assert_eq!(manifest.rule_source_status[0].source, "core-safety");
    assert_eq!(
        manifest.rule_source_status[0].state,
        RuleSourceState::Active
    );
    assert_eq!(
        manifest.rule_source_status[3].state,
        RuleSourceState::Disabled
    );
    assert_eq!(
        manifest.context_limits.max_context_bytes,
        mini_agent_core::HarnessConfig::default().max_context_bytes
    );

    let config = mini_agent_core::HarnessConfig {
        system_prompt: "bounded base prompt".to_string(),
        ..mini_agent_core::HarnessConfig::default()
    };
    let configured_manifest = profile.manifest_with_config(&config);
    assert_eq!(configured_manifest.prompt_source_fingerprints.len(), 1);
    assert_eq!(
        configured_manifest.prompt_source_fingerprints[0].source,
        "builtin"
    );
    assert!(
        !configured_manifest.prompt_source_fingerprints[0]
            .fingerprint
            .is_empty()
    );
    assert_eq!(configured_manifest.rule_source_fingerprints.len(), 1);
}

#[test]
fn no_tools_profile_is_explicit_and_does_not_admit_extensions() {
    let manifest = RuntimeProfile::ask_default().without_tools().manifest();

    assert_eq!(manifest.profile, "ask-no-tools");
    assert!(!manifest.enabled.iter().any(|name| name == "workspace"));
    assert!(
        manifest
            .disabled
            .iter()
            .any(|(name, reason)| name == "tools" && reason.contains("no-tools"))
    );
    assert!(
        manifest
            .disabled
            .iter()
            .any(|(name, _)| name == "extensions")
    );
    assert!(manifest.enabled.iter().any(|name| name == "workflows"));
    assert_eq!(manifest.prompt_sources, ["builtin", "project", "workflows"]);
    assert_eq!(manifest.rule_sources, ["project", "workflows"]);
}

#[test]
fn explicit_agent_and_persona_profiles_render_one_bounded_overlay() {
    let profile = RuntimeProfile {
        agent: AgentKind::Plan,
        persona: PersonaKind::Reviewer,
        ..RuntimeProfile::default()
    };

    let overlay = profile.prompt_overlay();

    assert!(overlay.contains("read-only software architect"));
    assert!(overlay.contains("meticulous code reviewer"));
    assert!(!overlay.is_empty());

    let manifest = profile.manifest();
    assert!(
        manifest
            .disabled
            .iter()
            .any(|(name, reason)| name == "workspace-write" && reason.contains("read-only"))
    );
    assert!(!manifest.rule_policy.workspace_write);
    assert!(!manifest.rule_policy.shell_execution);
}

#[test]
fn rule_policy_reports_shadowed_sources_and_read_only_security() {
    let profile = RuntimeProfile {
        agent: AgentKind::Plan,
        extensions: ExtensionLoadDepth::None,
        security: SecurityPreset::Turbomode,
        ..RuntimeProfile::default()
    };
    let manifest = profile.manifest();

    assert_eq!(
        manifest.rule_policy.workflow_scope,
        WorkflowScope::PlanAndGoal
    );
    assert!(!manifest.rule_policy.workspace_write);
    assert!(
        manifest
            .rule_conflicts
            .iter()
            .any(|conflict| conflict.contains("extension depth none"))
    );
    assert!(
        manifest
            .rule_conflicts
            .iter()
            .any(|conflict| conflict.contains("turbomode"))
    );
    assert_eq!(
        manifest.rule_source_status[6].state,
        RuleSourceState::Shadowed
    );
}

#[test]
fn workspace_profile_file_overlays_bounded_selections() {
    let root = test_root();
    fs::create_dir_all(root.join(".agents")).unwrap();
    fs::write(
        root.join(".agents/profile.json"),
        r#"{
            "name": "repo-review",
            "modelProvider": "openai",
            "toolProvider": "builtin",
            "extensionProvider": "builtin",
            "policyProvider": "builtin",
            "tools": "none",
            "extensionDepth": "selected",
            "selectedExtensions": ["review"],
            "agent": "plan",
            "persona": "reviewer",
            "workflows": "plan",
            "promptSources": {"project": true, "extensions": false, "workflows": true},
            "ruleSources": {"project": false, "extensions": true, "workflows": true},
            "sandbox": "none",
            "security": "full-machine"
        }"#,
    )
    .unwrap();

    let profile = load_workspace_profile(&root, RuntimeProfile::ask_default()).unwrap();

    assert_eq!(profile.name, "repo-review");
    assert_eq!(profile.model_provider, "openai");
    assert_eq!(profile.tool_provider, "builtin");
    assert_eq!(profile.extension_provider, "builtin");
    assert_eq!(profile.policy_provider, "builtin");
    assert_eq!(profile.tools, ToolScope::None);
    assert_eq!(profile.extensions, ExtensionLoadDepth::Selected);
    assert_eq!(
        profile.extension_selection,
        ExtensionSelection::Named(vec!["review".into()])
    );
    assert_eq!(profile.agent, AgentKind::Plan);
    assert_eq!(profile.persona, PersonaKind::Reviewer);
    assert_eq!(profile.workflows, WorkflowScope::Plan);
    assert!(!profile.regular_agent.rules.project);
    assert_eq!(profile.sandbox, SandboxKind::None);
    assert_eq!(profile.security, SecurityPreset::FullMachine);

    fs::remove_dir_all(root).unwrap();
}
