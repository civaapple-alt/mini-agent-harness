use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SecurityPreset {
    #[default]
    Default,
    FullMachine,
    Turbomode,
    Custom,
}

impl SecurityPreset {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.to_ascii_lowercase().as_str() {
            "default" => Ok(Self::Default),
            "full-machine" | "full_machine" | "fullmachine" => Ok(Self::FullMachine),
            "turbomode" | "turbo" => Ok(Self::Turbomode),
            "custom" => Ok(Self::Custom),
            other => Err(format!("unknown security preset: {other}")),
        }
    }

    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::FullMachine => "full-machine",
            Self::Turbomode => "turbomode",
            Self::Custom => "custom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityDecision {
    Allow,
    Ask,
    Deny,
}

const MAX_CACHED_APPROVALS: usize = 1024;

#[derive(Clone, Default)]
pub struct ApprovalStore(Arc<Mutex<HashSet<String>>>);

impl ApprovalStore {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashSet::new())))
    }

    pub fn is_approved(&self, key: &str) -> bool {
        let store = self.0.lock().unwrap();
        store.contains(key)
    }

    pub fn remember_approval(&self, key: &str) {
        let mut store = self.0.lock().unwrap();
        if store.len() >= MAX_CACHED_APPROVALS {
            store.clear();
        }
        store.insert(key.to_string());
    }

    #[allow(dead_code)]
    pub fn clear(&self) {
        let mut store = self.0.lock().unwrap();
        store.clear();
    }
}

#[derive(Clone, Debug, Default)]
pub struct SecurityPolicy {
    pub preset: SecurityPreset,
    pub deny_patterns: Vec<String>,
    pub ask_patterns: Vec<String>,
    pub allow_patterns: Vec<String>,
}

impl SecurityPolicy {
    pub fn for_preset(preset: SecurityPreset) -> Self {
        match preset {
            SecurityPreset::Default => Self {
                preset,
                deny_patterns: vec![
                    "**/.env*".to_string(),
                    "**/*.pem".to_string(),
                    "**/*.key".to_string(),
                    "rm -rf /*".to_string(),
                    "gh auth *".to_string(),
                ],
                ask_patterns: vec!["*".to_string()],
                allow_patterns: Vec::new(),
            },
            SecurityPreset::FullMachine => Self {
                preset,
                deny_patterns: vec!["rm -rf /*".to_string()],
                ask_patterns: vec!["shell:*".to_string()],
                allow_patterns: vec!["file:*".to_string()],
            },
            SecurityPreset::Turbomode => Self {
                preset,
                deny_patterns: Vec::new(),
                ask_patterns: Vec::new(),
                allow_patterns: vec!["*".to_string()],
            },
            SecurityPreset::Custom => Self {
                preset,
                deny_patterns: Vec::new(),
                ask_patterns: Vec::new(),
                allow_patterns: Vec::new(),
            },
        }
    }

    pub fn evaluate(&self, action: &str) -> SecurityDecision {
        // Strict priority: deny > ask > allow
        for pattern in &self.deny_patterns {
            if matches_pattern(pattern, action) {
                return SecurityDecision::Deny;
            }
        }
        for pattern in &self.ask_patterns {
            if matches_pattern(pattern, action) {
                return SecurityDecision::Ask;
            }
        }
        for pattern in &self.allow_patterns {
            if matches_pattern(pattern, action) {
                return SecurityDecision::Allow;
            }
        }
        match self.preset {
            SecurityPreset::Turbomode => SecurityDecision::Allow,
            SecurityPreset::FullMachine => {
                if action.starts_with("file:") {
                    SecurityDecision::Allow
                } else {
                    SecurityDecision::Ask
                }
            }
            SecurityPreset::Default | SecurityPreset::Custom => SecurityDecision::Ask,
        }
    }

    #[allow(dead_code)]
    pub fn check_file_access(&self, path: &Path, is_write: bool) -> SecurityDecision {
        let path_str = path.to_string_lossy().replace('\\', "/");
        let action = format!(
            "file:{}:{}",
            if is_write { "write" } else { "read" },
            path_str
        );
        self.evaluate(&action)
    }

    #[allow(dead_code)]
    pub fn check_command(&self, command: &str) -> SecurityDecision {
        let action = format!("shell:{}", command.trim());
        self.evaluate(&action)
    }
}

fn matches_pattern(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if match_single(pattern, text) {
        return true;
    }
    if let Some(stripped) = text.strip_prefix("shell:")
        && match_single(pattern, stripped)
    {
        return true;
    }
    if let Some(stripped) = text
        .strip_prefix("file:read:")
        .or_else(|| text.strip_prefix("file:write:"))
        && match_single(pattern, stripped)
    {
        return true;
    }
    false
}

fn match_single(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        text.starts_with(prefix)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        text.ends_with(suffix)
    } else {
        pattern == text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_security_presets() {
        assert_eq!(
            SecurityPreset::parse("default").unwrap(),
            SecurityPreset::Default
        );
        assert_eq!(
            SecurityPreset::parse("full-machine").unwrap(),
            SecurityPreset::FullMachine
        );
        assert_eq!(
            SecurityPreset::parse("turbomode").unwrap(),
            SecurityPreset::Turbomode
        );
        assert_eq!(
            SecurityPreset::parse("custom").unwrap(),
            SecurityPreset::Custom
        );
        assert_eq!(SecurityPreset::Default.name(), "default");
        assert!(SecurityPreset::parse("invalid").is_err());
    }

    #[test]
    fn evaluates_preset_priorities() {
        let default_policy = SecurityPolicy::for_preset(SecurityPreset::Default);
        assert_eq!(
            default_policy.check_command("cargo test"),
            SecurityDecision::Ask
        );
        assert_eq!(
            default_policy.check_command("gh auth login"),
            SecurityDecision::Deny
        );

        let turbo = SecurityPolicy::for_preset(SecurityPreset::Turbomode);
        assert_eq!(turbo.check_command("cargo build"), SecurityDecision::Allow);

        let full = SecurityPolicy::for_preset(SecurityPreset::FullMachine);
        assert_eq!(
            full.check_file_access(Path::new("C:/some/file.txt"), false),
            SecurityDecision::Allow
        );
        assert_eq!(full.check_command("dir"), SecurityDecision::Ask);
    }

    #[test]
    fn caches_session_approvals() {
        let store = ApprovalStore::new();
        let key = "shell:cargo test";
        assert!(!store.is_approved(key));

        store.remember_approval(key);
        assert!(store.is_approved(key));

        store.clear();
        assert!(!store.is_approved(key));
    }
}
