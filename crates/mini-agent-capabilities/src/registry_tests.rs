use super::*;

#[test]
fn builtin_registry_exposes_only_allowlisted_model_provider() {
    let registry = CapabilityRegistry::builtin();
    assert!(registry.contains_model(crate::OPENAI_MODEL_PROVIDER));
    assert!(!registry.contains_model("unknown"));
    assert_eq!(
        registry.descriptors(),
        &[CapabilityDescriptor {
            id: crate::OPENAI_MODEL_PROVIDER,
            kind: CapabilityKind::Model,
            description: "OpenAI-compatible Responses model provider",
        }]
    );
}
