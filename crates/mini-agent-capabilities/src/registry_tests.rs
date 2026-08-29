use super::*;

#[test]
fn builtin_registry_exposes_allowlisted_provider_categories() {
    let registry = CapabilityRegistry::builtin();
    assert!(registry.contains_model(crate::OPENAI_MODEL_PROVIDER));
    assert!(registry.contains(CapabilityKind::Tool, crate::BUILTIN_TOOL_PROVIDER));
    assert!(registry.contains(CapabilityKind::Extension, crate::BUILTIN_EXTENSION_PROVIDER));
    assert!(registry.contains(CapabilityKind::Policy, crate::BUILTIN_POLICY_PROVIDER));
    assert!(!registry.contains_model("unknown"));
    assert!(registry.validate(CapabilityKind::Tool, "unknown").is_err());
    assert_eq!(
        registry.descriptors(),
        &[
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
        ]
    );
}

#[test]
fn policy_provider_builds_the_profile_selected_preset() {
    let policy = CapabilityRegistry::builtin()
        .build_policy(
            crate::BUILTIN_POLICY_PROVIDER,
            crate::security::SecurityPreset::Turbomode,
        )
        .unwrap();

    assert_eq!(policy.preset, crate::security::SecurityPreset::Turbomode);
}
