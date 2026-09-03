/// A bounded, Thread-local selection of the model-visible Builtin tools.
///
/// The selection changes exposure only. Provider construction, approval, and
/// execution continue to use their existing Host/Core boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinToolSelection {
    names: Vec<String>,
}

const DEFAULT_BUILTIN_TOOLS: [&str; 4] = ["read_file", "apply_patch", "shell", "read_image"];

const AVAILABLE_BUILTIN_TOOLS: [&str; 5] = [
    "read_file",
    "apply_patch",
    "shell",
    "read_image",
    "web_fetch",
];

impl BuiltinToolSelection {
    /// Select every Builtin implementation available to this Host.
    pub fn all() -> Self {
        Self {
            names: AVAILABLE_BUILTIN_TOOLS
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn from_names(names: Vec<String>) -> Result<Self, String> {
        if names.len() > AVAILABLE_BUILTIN_TOOLS.len() {
            return Err(format!(
                "builtin tool selection has too many entries: {}",
                names.len()
            ));
        }
        for name in &names {
            if !AVAILABLE_BUILTIN_TOOLS.contains(&name.as_str()) {
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
        AVAILABLE_BUILTIN_TOOLS
            .iter()
            .filter(|name| !self.names.iter().any(|selected| selected == *name))
            .map(|name| (*name).to_string())
            .collect()
    }
}

impl Default for BuiltinToolSelection {
    fn default() -> Self {
        Self {
            names: DEFAULT_BUILTIN_TOOLS
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selection_is_small_and_all_contains_only_supported_builtins() {
        assert_eq!(
            BuiltinToolSelection::default().names().to_vec(),
            vec!["read_file", "apply_patch", "shell", "read_image"]
        );
        assert_eq!(
            BuiltinToolSelection::default().hidden_names(),
            vec!["web_fetch"]
        );
        assert_eq!(BuiltinToolSelection::all().names().len(), 5);
        assert!(BuiltinToolSelection::all().hidden_names().is_empty());
    }

    #[test]
    fn explicit_empty_selection_is_valid_and_hides_every_builtin() {
        let selection = BuiltinToolSelection::from_names(Vec::new()).unwrap();
        assert!(selection.names().is_empty());
        assert_eq!(selection.hidden_names().len(), 5);
    }
}
