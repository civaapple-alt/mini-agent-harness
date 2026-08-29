use super::*;
use std::sync::Arc;

struct EmptyExternalProvider;

impl ToolProvider for EmptyExternalProvider {
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: "example-test",
            kind: CapabilityKind::Tool,
            description: "test external tool provider",
        }
    }

    fn build_tools(&self, _request: ToolBuildRequest) -> Result<Vec<Box<dyn Tool>>, ToolError> {
        Ok(Vec::new())
    }
}

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

#[test]
fn external_tool_provider_is_registered_without_changing_builtin_order() {
    let registry =
        CapabilityRegistry::builtin().with_tool_provider(Arc::new(EmptyExternalProvider));

    assert!(
        registry
            .validate(CapabilityKind::Tool, "example-test")
            .is_ok()
    );
    assert_eq!(registry.descriptors()[4].id, "example-test");
    assert_eq!(registry.descriptors()[4].kind, CapabilityKind::Tool);
}

#[test]
fn external_model_provider_is_selectable_by_stable_id() {
    let registry = CapabilityRegistry::builtin().with_model_provider(CapabilityDescriptor {
        id: "example-model",
        kind: CapabilityKind::Model,
        description: "test external model provider",
    });

    assert!(
        registry
            .validate(CapabilityKind::Model, "example-model")
            .is_ok()
    );
    assert_eq!(registry.descriptors()[4].id, "example-model");
    assert_eq!(registry.descriptors()[4].kind, CapabilityKind::Model);
}
