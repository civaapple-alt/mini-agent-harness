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
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
