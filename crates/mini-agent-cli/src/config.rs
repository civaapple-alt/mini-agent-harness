use crate::env_file::Environment;
use crate::env_file::ResolvedValue;
use crate::env_file::ValueSource;
use crate::project_context;
use crate::skills;
use reqwest::Url;
use serde_json::Value;
use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct RuntimeConfig {
    workspace: PathBuf,
    api_key: Option<ResolvedValue>,
    model: Option<ResolvedValue>,
    base_url: ResolvedSetting,
}

pub struct ProviderSettings {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

struct ResolvedSetting {
    value: String,
    source: SettingSource,
}

#[derive(Clone, Copy)]
enum SettingSource {
    Process,
    EnvFile,
    BuiltIn,
}

pub struct DoctorReport {
    pub ok: bool,
    pub json: Value,
    pub lines: Vec<String>,
}

impl RuntimeConfig {
    pub fn load() -> Result<Self, String> {
        let workspace = env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?;
        let environment = Environment::load(workspace.join(".env"))?;
        let api_key = environment.resolve("OPENAI_API_KEY");
        let model = environment.resolve("OPENAI_MODEL");
        let base_url = environment
            .resolve("OPENAI_BASE_URL")
            .map(ResolvedSetting::from_environment)
            .unwrap_or_else(|| ResolvedSetting {
                value: DEFAULT_BASE_URL.to_string(),
                source: SettingSource::BuiltIn,
            });
        Ok(Self {
            workspace,
            api_key,
            model,
            base_url,
        })
    }

    pub fn provider_settings(&self) -> Result<ProviderSettings, String> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| "OPENAI_API_KEY is required".to_string())?
            .value
            .clone();
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| "OPENAI_MODEL is required".to_string())?
            .value
            .clone();
        validate_base_url(&self.base_url.value)?;
        Ok(ProviderSettings {
            api_key,
            model,
            base_url: self.base_url.value.clone(),
        })
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace.clone()
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_ref().map(|model| model.value.as_str())
    }

    pub fn status_json(&self) -> Value {
        let display_base_url = display_base_url(&self.base_url.value);
        let extensions = skills::discover(&self.workspace);
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "workspace": self.workspace,
            "provider": "openai_responses",
            "model": self.model.as_ref().map(|value| value.value.as_str()),
            "model_source": self.model.as_ref().map(|value| source_name(value.source)),
            "base_url": display_base_url,
            "base_url_source": setting_source_name(self.base_url.source),
            "credential": if self.api_key.is_some() { "configured" } else { "missing" },
            "credential_source": self.api_key.as_ref().map(|value| source_name(value.source)),
            "instructions": extensions.len(),
            "skills": extensions.skill_count(),
            "plugin_agents": extensions.plugin_agent_count(),
            "plugins": extensions.plugin_count(),
            "marketplaces": extensions.marketplace_count(),
            "mcp_servers": extensions.mcp_server_count(),
            "mcp_stdio_servers": extensions.stdio_mcp_server_count(),
            "mcp_http_servers": extensions.http_mcp_server_count(),
            "telemetry": false,
            "session_persistence": false,
            "command_sandbox": false
        })
    }

    pub fn status_lines(&self) -> Vec<String> {
        let display_base_url = display_base_url(&self.base_url.value);
        let extensions = skills::discover(&self.workspace);
        vec![
            format!("version: {}", env!("CARGO_PKG_VERSION")),
            format!("workspace: {}", self.workspace.display()),
            "provider: openai_responses".to_string(),
            format!(
                "model: {} ({})",
                self.model
                    .as_ref()
                    .map_or("missing", |value| value.value.as_str()),
                self.model
                    .as_ref()
                    .map_or("unconfigured", |value| source_name(value.source))
            ),
            format!(
                "base_url: {} ({})",
                display_base_url,
                setting_source_name(self.base_url.source)
            ),
            format!(
                "credential: {}",
                self.api_key
                    .as_ref()
                    .map_or("missing".to_string(), |value| format!(
                        "configured ({})",
                        source_name(value.source)
                    ))
            ),
            format!("instructions: {}", extensions.len()),
            format!("skills: {}", extensions.skill_count()),
            format!("plugin_agents: {}", extensions.plugin_agent_count()),
            format!("plugins: {}", extensions.plugin_count()),
            format!("marketplaces: {}", extensions.marketplace_count()),
            format!("mcp_servers: {}", extensions.mcp_server_count()),
            format!("mcp_stdio_servers: {}", extensions.stdio_mcp_server_count()),
            format!("mcp_http_servers: {}", extensions.http_mcp_server_count()),
            "telemetry: disabled".to_string(),
            "session_persistence: disabled".to_string(),
            "command_sandbox: disabled".to_string(),
        ]
    }

    pub fn doctor(&self) -> DoctorReport {
        let mut checks = Vec::new();
        checks.push(check(
            "workspace",
            self.workspace.is_dir(),
            self.workspace.display().to_string(),
        ));
        checks.push(check(
            "credential",
            self.api_key.is_some(),
            if self.api_key.is_some() {
                "OPENAI_API_KEY is configured".to_string()
            } else {
                "OPENAI_API_KEY is missing".to_string()
            },
        ));
        checks.push(check(
            "model",
            self.model.is_some(),
            self.model.as_ref().map_or_else(
                || "OPENAI_MODEL is missing".to_string(),
                |model| format!("OPENAI_MODEL={}", model.value),
            ),
        ));
        checks.push(match validate_base_url(&self.base_url.value) {
            Ok(()) => check("base_url", true, display_base_url(&self.base_url.value)),
            Err(error) => check("base_url", false, error),
        });
        checks.push(match shell_available() {
            Ok(shell) => check("shell", true, shell),
            Err(error) => check("shell", false, error),
        });
        checks.push(
            match project_context::augment_system_prompt("", &self.workspace) {
                Ok(_) if self.workspace.join("AGENTS.md").is_file() => check(
                    "project_instructions",
                    true,
                    "root AGENTS.md is valid".to_string(),
                ),
                Ok(_) => check(
                    "project_instructions",
                    true,
                    "no root AGENTS.md".to_string(),
                ),
                Err(error) => check("project_instructions", false, error),
            },
        );
        let skill_discovery = skills::discover(&self.workspace);
        checks.push(if skill_discovery.diagnostics().is_empty() {
            check(
                "extensions",
                true,
                format!(
                    "{} instructions, {} plugins, {} marketplaces, and {} MCP servers ({} stdio, {} HTTP) discovered",
                    skill_discovery.len(),
                    skill_discovery.plugin_count(),
                    skill_discovery.marketplace_count(),
                    skill_discovery.mcp_server_count(),
                    skill_discovery.stdio_mcp_server_count(),
                    skill_discovery.http_mcp_server_count()
                ),
            )
        } else {
            check(
                "extensions",
                false,
                skill_discovery.diagnostics().join("; "),
            )
        });
        let ok = checks.iter().all(|item| item.ok);
        let lines = checks
            .iter()
            .map(|item| {
                let status = if item.ok { "ok" } else { "error" };
                format!("{status}: {} — {}", item.name, item.detail)
            })
            .collect();
        let json_checks = checks
            .iter()
            .map(|item| {
                json!({
                    "name": item.name,
                    "status": if item.ok { "ok" } else { "error" },
                    "detail": item.detail
                })
            })
            .collect::<Vec<_>>();
        DoctorReport {
            ok,
            json: json!({"ok": ok, "checks": json_checks}),
            lines,
        }
    }
}

impl ResolvedSetting {
    fn from_environment(value: ResolvedValue) -> Self {
        Self {
            value: value.value,
            source: match value.source {
                ValueSource::Process => SettingSource::Process,
                ValueSource::EnvFile => SettingSource::EnvFile,
            },
        }
    }
}

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn check(name: &'static str, ok: bool, detail: String) -> Check {
    Check { name, ok, detail }
}

fn validate_base_url(base_url: &str) -> Result<(), String> {
    let url = Url::parse(base_url)
        .map_err(|error| format!("OPENAI_BASE_URL is not a valid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("OPENAI_BASE_URL must be an absolute http or https URL".to_string());
    }
    Ok(())
}

fn display_base_url(base_url: &str) -> String {
    let Ok(mut url) = Url::parse(base_url) else {
        return "<invalid>".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn shell_available() -> Result<String, String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("pwsh");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "exit 0",
        ]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", ":"]);
        command
    };
    let name = if cfg!(windows) { "pwsh" } else { "sh" };
    match command.status() {
        Ok(status) if status.success() => Ok(format!("{name} is available")),
        Ok(status) => Err(format!("{name} health check exited with {status}")),
        Err(error) => Err(format!("cannot start {name}: {error}")),
    }
}

fn source_name(source: ValueSource) -> &'static str {
    match source {
        ValueSource::Process => "process",
        ValueSource::EnvFile => ".env",
    }
}

fn setting_source_name(source: SettingSource) -> &'static str {
    match source {
        SettingSource::Process => "process",
        SettingSource::EnvFile => ".env",
        SettingSource::BuiltIn => "built_in",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_absolute_http_urls() {
        assert!(validate_base_url("https://api.deepseek.com").is_ok());
        assert!(validate_base_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(validate_base_url("file:///tmp/api").is_err());
        assert!(validate_base_url("not a url").is_err());
    }

    #[test]
    fn redacts_url_credentials_and_query_values() {
        assert_eq!(
            display_base_url("https://user:secret@example.com/v1?token=private#fragment"),
            "https://example.com/v1"
        );
    }
}
