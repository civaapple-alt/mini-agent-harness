use crate::env_file::Environment;
use crate::env_file::ResolvedValue;
use crate::env_file::ValueSource;
use crate::project_context;
use crate::session;
use crate::skills;
use crate::workspace::ApprovalMode;
use crate::world::WorldState;
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
    chat_base_url: Option<ResolvedSetting>,
    mentor_api_key: Option<ResolvedValue>,
    mentor_model: Option<ResolvedValue>,
    mentor_base_url: Option<ResolvedSetting>,
    max_agent_steps: Option<usize>,
    web_search: bool,
}

pub struct ProviderSettings {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub chat_base_url: Option<String>,
    pub web_search: bool,
}

struct ResolvedSetting {
    value: String,
    source: SettingSource,
}

#[derive(Clone, Copy)]
enum SettingSource {
    Process,
    EnvFile,
    UserEnv,
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
        Self::load_from(workspace, user_env_path())
    }

    fn load_from(workspace: PathBuf, user_env: Option<PathBuf>) -> Result<Self, String> {
        let workspace_env = Environment::load(workspace.join(".env"))?;
        let user_env = match user_env {
            Some(path) => Environment::load(path)?,
            None => Environment::default(),
        };
        let api_key = resolve_value("OPENAI_API_KEY", &workspace_env, &user_env);
        let model = resolve_value("OPENAI_MODEL", &workspace_env, &user_env);
        let base_url = resolve_value("OPENAI_BASE_URL", &workspace_env, &user_env)
            .map(ResolvedSetting::from_environment)
            .unwrap_or_else(|| ResolvedSetting {
                value: DEFAULT_BASE_URL.to_string(),
                source: SettingSource::BuiltIn,
            });
        let chat_base_url = resolve_value("OPENAI_CHAT_BASE_URL", &workspace_env, &user_env)
            .map(ResolvedSetting::from_environment);
        let mentor_api_key = resolve_value("MENTOR_OPENAI_API_KEY", &workspace_env, &user_env);
        let mentor_model = resolve_value("MENTOR_OPENAI_MODEL", &workspace_env, &user_env);
        let mentor_base_url = resolve_value("MENTOR_OPENAI_BASE_URL", &workspace_env, &user_env)
            .map(ResolvedSetting::from_environment);
        let max_agent_steps = match resolve_value("MINI_AGENT_MAX_STEPS", &workspace_env, &user_env)
        {
            Some(value) => Some(parse_max_agent_steps(&value.value)?),
            None => None,
        };
        let web_search = match resolve_value("MINI_AGENT_WEB_SEARCH", &workspace_env, &user_env)
            .or_else(|| resolve_value("OPENAI_WEB_SEARCH", &workspace_env, &user_env))
        {
            Some(value) => parse_bool_setting("MINI_AGENT_WEB_SEARCH", &value.value)?,
            None => is_official_search_endpoint(&base_url),
        };
        Ok(Self {
            workspace,
            api_key,
            model,
            base_url,
            chat_base_url,
            mentor_api_key,
            mentor_model,
            mentor_base_url,
            max_agent_steps,
            web_search,
        })
    }

    pub fn provider_settings(&self) -> Result<ProviderSettings, String> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| {
                "OPENAI_API_KEY is required (process, .env, or ~/.mini-agent/.env)".to_string()
            })?
            .value
            .clone();
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| {
                "OPENAI_MODEL is required (process, .env, or ~/.mini-agent/.env)".to_string()
            })?
            .value
            .clone();
        validate_base_url(&self.base_url.value)?;
        let chat_base_url = match &self.chat_base_url {
            Some(url) => {
                validate_base_url_named("OPENAI_CHAT_BASE_URL", &url.value)?;
                Some(url.value.clone())
            }
            None => None,
        };
        Ok(ProviderSettings {
            api_key,
            model,
            base_url: self.base_url.value.clone(),
            chat_base_url,
            web_search: self.web_search,
        })
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace.clone()
    }

    pub fn copilot_max_steps(&self) -> usize {
        self.max_agent_steps.unwrap_or(0)
    }

    pub fn web_search(&self) -> bool {
        self.web_search
    }

    pub fn with_web_search(mut self, enabled: bool) -> Self {
        self.web_search = enabled;
        self
    }

    pub fn mentor_provider_settings(&self) -> Result<ProviderSettings, String> {
        let model = self
            .mentor_model
            .as_ref()
            .ok_or_else(|| {
                "MENTOR_OPENAI_MODEL is required for mentor commands (process, .env, or ~/.mini-agent/.env)"
                    .to_string()
            })?
            .value
            .clone();
        let api_key = self
            .mentor_api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| {
                "MENTOR_OPENAI_API_KEY or OPENAI_API_KEY is required for mentor commands"
                    .to_string()
            })?
            .value
            .clone();
        let base_url = self
            .mentor_base_url
            .as_ref()
            .unwrap_or(&self.base_url)
            .value
            .clone();
        validate_base_url_named("MENTOR_OPENAI_BASE_URL", &base_url)?;
        Ok(ProviderSettings {
            api_key,
            model,
            base_url,
            chat_base_url: None,
            web_search: false,
        })
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_ref().map(|model| model.value.as_str())
    }

    pub fn status_json(&self) -> Value {
        let primary_display_base_url = display_base_url(&self.base_url.value);
        let (extensions, world) = self.status_snapshot();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "git_sha": crate::git_sha(),
            "workspace": self.workspace,
            "provider": "openai_responses",
            "model": self.model.as_ref().map(|value| value.value.as_str()),
            "model_source": self.model.as_ref().map(|value| source_name(value.source)),
            "base_url": primary_display_base_url,
            "base_url_source": setting_source_name(self.base_url.source),
            "chat_base_url": self
                .chat_base_url
                .as_ref()
                .map(|url| display_base_url(&url.value)),
            "chat_base_url_source": self
                .chat_base_url
                .as_ref()
                .map(|url| setting_source_name(url.source)),
            "credential": if self.api_key.is_some() { "configured" } else { "missing" },
            "credential_source": self.api_key.as_ref().map(|value| source_name(value.source)),
            "web_search": self.web_search,
            "mentor": {
                "enabled": self.mentor_model.is_some(),
                "model": self.mentor_model.as_ref().map(|value| value.value.as_str()),
                "model_source": self.mentor_model.as_ref().map(|value| source_name(value.source)),
                "credential": if self.mentor_api_key.is_some() {
                    "dedicated"
                } else if self.api_key.is_some() {
                    "inherited"
                } else {
                    "missing"
                },
                "base_url": display_base_url(
                    &self.mentor_base_url.as_ref().unwrap_or(&self.base_url).value
                ),
                "base_url_source": self.mentor_base_url.as_ref().map_or(
                    "inherited",
                    |value| setting_source_name(value.source)
                )
            },
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
            "session_persistence_available": true,
            "session_directory": session::session_directory(&self.workspace)
                .ok()
                .map(|path| json!(path.display().to_string()))
                .unwrap_or(Value::Null),
            "user_config_directory": user_config_dir(),
            "auto_max_agent_steps": self.copilot_max_steps(),
            "command_sandbox": true,
            "world": world.status_json()
        })
    }

    pub fn status_lines(&self) -> Vec<String> {
        let shown_base_url = display_base_url(&self.base_url.value);
        let (extensions, world) = self.status_snapshot();
        let mut lines = vec![
            format!(
                "version: {} ({})",
                env!("CARGO_PKG_VERSION"),
                crate::git_sha()
            ),
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
                shown_base_url,
                setting_source_name(self.base_url.source)
            ),
            format!(
                "chat_base_url: {} ({})",
                self.chat_base_url
                    .as_ref()
                    .map(|url| display_base_url(&url.value))
                    .unwrap_or_else(|| "unset".to_string()),
                self.chat_base_url
                    .as_ref()
                    .map(|url| setting_source_name(url.source))
                    .unwrap_or("unset")
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
            self.mentor_status_line(),
            format!("instructions: {}", extensions.len()),
            format!("skills: {}", extensions.skill_count()),
            format!("plugin_agents: {}", extensions.plugin_agent_count()),
            format!("plugins: {}", extensions.plugin_count()),
            format!("marketplaces: {}", extensions.marketplace_count()),
            format!("mcp_servers: {}", extensions.mcp_server_count()),
            format!("mcp_stdio_servers: {}", extensions.stdio_mcp_server_count()),
            format!("mcp_http_servers: {}", extensions.http_mcp_server_count()),
            "telemetry: disabled".to_string(),
            "session_persistence: opt_in (--persist or resume)".to_string(),
            format!(
                "session_directory: {}",
                session::session_directory(&self.workspace).map_or_else(
                    |_| "unavailable".to_string(),
                    |path| path.display().to_string()
                )
            ),
            format!(
                "user_config_directory: {}",
                user_config_dir().map_or_else(
                    || "unavailable".to_string(),
                    |path| path.display().to_string()
                )
            ),
            format!(
                "auto_max_agent_steps: {}",
                display_max_agent_steps(self.copilot_max_steps())
            ),
            format!(
                "web_search: {}",
                if self.web_search {
                    "enabled (built-in responses web_search)"
                } else {
                    "disabled"
                }
            ),
            "command_sandbox: native".to_string(),
        ];
        lines.extend(world.status_lines());
        lines
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
        checks.push(match &self.chat_base_url {
            Some(url) => match validate_base_url_named("OPENAI_CHAT_BASE_URL", &url.value) {
                Ok(()) => check("chat_base_url", true, display_base_url(&url.value)),
                Err(error) => check("chat_base_url", false, error),
            },
            None => check(
                "chat_base_url",
                true,
                "unset (set OPENAI_CHAT_BASE_URL for GLM image turns)".to_string(),
            ),
        });
        if self.mentor_model.is_some() {
            checks.push(check(
                "mentor_credential",
                self.mentor_api_key.is_some() || self.api_key.is_some(),
                if self.mentor_api_key.is_some() {
                    "MENTOR_OPENAI_API_KEY is configured".to_string()
                } else if self.api_key.is_some() {
                    "using OPENAI_API_KEY".to_string()
                } else {
                    "MENTOR_OPENAI_API_KEY and OPENAI_API_KEY are missing".to_string()
                },
            ));
            let mentor_base_url = &self
                .mentor_base_url
                .as_ref()
                .unwrap_or(&self.base_url)
                .value;
            checks.push(
                match validate_base_url_named("MENTOR_OPENAI_BASE_URL", mentor_base_url) {
                    Ok(()) => check("mentor_base_url", true, display_base_url(mentor_base_url)),
                    Err(error) => check("mentor_base_url", false, error),
                },
            );
        }
        checks.push(match shell_available() {
            Ok(shell) => check("shell", true, shell),
            Err(error) => check("shell", false, error),
        });
        checks.push(match project_context::load_agents_md(&self.workspace) {
            Ok(project_context::AgentsMd::Absent) => check(
                "project_instructions",
                true,
                "no root AGENTS.md".to_string(),
            ),
            Ok(project_context::AgentsMd::Loaded {
                truncated: false, ..
            }) => check(
                "project_instructions",
                true,
                "root AGENTS.md is valid".to_string(),
            ),
            Ok(project_context::AgentsMd::Loaded { source_bytes, .. }) => check(
                "project_instructions",
                false,
                format!(
                    "root AGENTS.md exceeds {} bytes ({source_bytes}); using bounded head and tail",
                    project_context::MAX_PROJECT_INSTRUCTIONS_BYTES
                ),
            ),
            Err(error) => check("project_instructions", false, error),
        });
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

    fn status_snapshot(&self) -> (skills::Discovery, WorldState) {
        (
            skills::discover(&self.workspace),
            WorldState::detect(
                &self.workspace,
                ApprovalMode::Automatic,
                false,
                crate::sandbox::SandboxKind::Native,
            ),
        )
    }

    fn mentor_status_line(&self) -> String {
        self.mentor_model.as_ref().map_or_else(
            || "mentor: disabled (set MENTOR_OPENAI_MODEL)".to_string(),
            |model| {
                format!(
                    "mentor: {} ({}, {} credential)",
                    model.value,
                    source_name(model.source),
                    if self.mentor_api_key.is_some() {
                        "dedicated"
                    } else if self.api_key.is_some() {
                        "inherited"
                    } else {
                        "missing"
                    }
                )
            },
        )
    }
}

impl ResolvedSetting {
    fn from_environment(value: ResolvedValue) -> Self {
        Self {
            value: value.value,
            source: match value.source {
                ValueSource::Process => SettingSource::Process,
                ValueSource::EnvFile => SettingSource::EnvFile,
                ValueSource::UserEnv => SettingSource::UserEnv,
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

fn is_official_search_endpoint(base_url: &ResolvedSetting) -> bool {
    let val = base_url.value.to_ascii_lowercase();
    val.contains("api.openai.com") || val.contains("api.deepseek.com")
}

fn validate_base_url(base_url: &str) -> Result<(), String> {
    validate_base_url_named("OPENAI_BASE_URL", base_url)
}

fn validate_base_url_named(name: &str, base_url: &str) -> Result<(), String> {
    let url =
        Url::parse(base_url).map_err(|error| format!("{name} is not a valid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{name} must be an absolute http or https URL"));
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

fn parse_max_agent_steps(value: &str) -> Result<usize, String> {
    value.parse::<usize>().map_err(|_| {
        "MINI_AGENT_MAX_STEPS must be a non-negative integer (0 = unlimited)".to_string()
    })
}

fn parse_bool_setting(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" | "enable" | "enabled" => Ok(true),
        "false" | "0" | "no" | "off" | "disable" | "disabled" => Ok(false),
        _ => Err(format!(
            "{name} must be a boolean (true/false, 1/0, on/off)"
        )),
    }
}

fn display_max_agent_steps(steps: usize) -> String {
    if steps == 0 {
        "unlimited".to_string()
    } else {
        steps.to_string()
    }
}

fn resolve_value(name: &str, workspace: &Environment, user: &Environment) -> Option<ResolvedValue> {
    workspace.resolve(name).or_else(|| {
        user.get(name).map(|value| ResolvedValue {
            value: value.to_string(),
            source: ValueSource::UserEnv,
        })
    })
}

fn user_config_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".mini-agent"))
}

fn user_env_path() -> Option<PathBuf> {
    user_config_dir().map(|directory| directory.join(".env"))
}

fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(key)
        .or_else(|| {
            if cfg!(windows) {
                env::var_os("HOME")
            } else {
                None
            }
        })
        .map(PathBuf::from)
}

fn source_name(source: ValueSource) -> &'static str {
    match source {
        ValueSource::Process => "process",
        ValueSource::EnvFile => ".env",
        ValueSource::UserEnv => "~/.mini-agent/.env",
    }
}

fn setting_source_name(source: SettingSource) -> &'static str {
    match source {
        SettingSource::Process => "process",
        SettingSource::EnvFile => ".env",
        SettingSource::UserEnv => "~/.mini-agent/.env",
        SettingSource::BuiltIn => "built_in",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn validates_absolute_http_urls() {
        assert!(validate_base_url("https://api.deepseek.com").is_ok());
        assert!(validate_base_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(validate_base_url("file:///tmp/api").is_err());
        assert!(validate_base_url("not a url").is_err());
    }

    #[test]
    fn glm_coding_plan_does_not_enable_builtin_web_search() {
        let workspace = unique_dir("workspace");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=k\nOPENAI_MODEL=glm-5.3\nOPENAI_BASE_URL=https://open.bigmodel.cn/api/v1\n",
        )
        .unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert!(!config.web_search());
        assert!(!is_official_search_endpoint(&config.base_url));
        assert!(config.provider_settings().unwrap().chat_base_url.is_none());
    }

    #[test]
    fn chat_base_url_is_explicit_and_not_rewritten() {
        let workspace = unique_dir("workspace");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=k\nOPENAI_MODEL=glm-5.3-flash\nOPENAI_BASE_URL=https://open.bigmodel.cn/api/v1\nOPENAI_CHAT_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4\n",
        )
        .unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        let provider = config.provider_settings().unwrap();
        assert_eq!(provider.base_url, "https://open.bigmodel.cn/api/v1");
        assert_eq!(
            provider.chat_base_url.as_deref(),
            Some("https://open.bigmodel.cn/api/coding/paas/v4")
        );
    }

    #[test]
    fn redacts_url_credentials_and_query_values() {
        assert_eq!(
            display_base_url("https://user:secret@example.com/v1?token=private#fragment"),
            "https://example.com/v1"
        );
    }

    #[test]
    fn user_env_fills_provider_settings_when_workspace_env_is_absent() {
        let workspace = unique_dir("workspace");
        let user_env = unique_dir("user").join(".env");
        fs::write(
            &user_env,
            "OPENAI_API_KEY=user-key\nOPENAI_MODEL=deepseek-v4-flash\nOPENAI_BASE_URL=https://api.deepseek.com\n",
        )
        .unwrap();

        let config = RuntimeConfig::load_from(workspace, Some(user_env)).unwrap();

        let provider = config.provider_settings().unwrap();
        assert_eq!(provider.api_key, "user-key");
        assert_eq!(provider.model, "deepseek-v4-flash");
        assert_eq!(
            source_name(config.api_key.as_ref().unwrap().source),
            "~/.mini-agent/.env"
        );
        assert_eq!(
            setting_source_name(config.base_url.source),
            "~/.mini-agent/.env"
        );
    }

    #[test]
    fn workspace_env_overrides_user_env() {
        let workspace = unique_dir("workspace");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=workspace-key\nOPENAI_MODEL=workspace-model\n",
        )
        .unwrap();
        let user_env = unique_dir("user").join(".env");
        fs::write(
            &user_env,
            "OPENAI_API_KEY=user-key\nOPENAI_MODEL=user-model\n",
        )
        .unwrap();

        let config = RuntimeConfig::load_from(workspace, Some(user_env)).unwrap();

        assert_eq!(config.api_key.as_ref().unwrap().value, "workspace-key");
        assert_eq!(source_name(config.api_key.as_ref().unwrap().source), ".env");
    }

    #[test]
    fn copilot_max_steps_default_to_unlimited() {
        let workspace = unique_dir("workspace");
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert_eq!(config.copilot_max_steps(), 0);
        assert_eq!(display_max_agent_steps(0), "unlimited");
        assert_eq!(display_max_agent_steps(40), "40");
    }

    #[test]
    fn copilot_max_steps_reads_workspace_env() {
        let workspace = unique_dir("workspace");
        fs::write(workspace.join(".env"), "MINI_AGENT_MAX_STEPS=40\n").unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert_eq!(config.copilot_max_steps(), 40);
    }

    #[test]
    fn copilot_max_steps_rejects_invalid_values() {
        let workspace = unique_dir("workspace");
        fs::write(workspace.join(".env"), "MINI_AGENT_MAX_STEPS=-1\n").unwrap();
        let error = match RuntimeConfig::load_from(workspace, None) {
            Ok(_) => panic!("expected MINI_AGENT_MAX_STEPS to be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("MINI_AGENT_MAX_STEPS"));
    }

    fn unique_dir(label: &str) -> PathBuf {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering;
        use std::time::SystemTime;
        use std::time::UNIX_EPOCH;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("mini-agent-config-{label}-{nonce}-{sequence}"));
        fs::create_dir(&root).unwrap();
        root
    }
}
