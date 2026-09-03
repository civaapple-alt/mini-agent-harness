/// A bounded, Thread-local selection of the model-visible Builtin tools.
///
/// The selection changes exposure only. Provider construction, approval, and
/// execution continue to use their existing Host/Core boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinToolSelection {
    names: Vec<String>,
}

const DEFAULT_BUILTIN_TOOLS: [&str; 6] = [
    "read_file",
    "edit_file",
    "write_file",
    "shell",
    "web_fetch",
    "read_image",
];

impl BuiltinToolSelection {
    pub fn all() -> Self {
        Self {
            names: DEFAULT_BUILTIN_TOOLS
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn from_names(names: Vec<String>) -> Result<Self, String> {
        if names.len() > DEFAULT_BUILTIN_TOOLS.len() {
            return Err(format!(
                "builtin tool selection has too many entries: {}",
                names.len()
            ));
        }
        for name in &names {
            if !DEFAULT_BUILTIN_TOOLS.contains(&name.as_str()) {
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
        DEFAULT_BUILTIN_TOOLS
            .iter()
            .filter(|name| !self.names.iter().any(|selected| selected == *name))
            .map(|name| (*name).to_string())
            .collect()
    }
}

impl Default for BuiltinToolSelection {
    fn default() -> Self {
        Self::all()
    }
}
