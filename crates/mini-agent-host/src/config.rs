use crate::env_file::Environment;
use crate::env_file::ResolvedValue;
use crate::env_file::ValueSource;
use crate::goal::GoalLimits;
use reqwest::Url;
use std::env;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const VERIFIER_OPENAI_API_KEY: &str = "VERIFIER_OPENAI_API_KEY";
const VERIFIER_OPENAI_MODEL: &str = "VERIFIER_OPENAI_MODEL";
const VERIFIER_OPENAI_BASE_URL: &str = "VERIFIER_OPENAI_BASE_URL";

#[derive(Clone)]
pub struct RuntimeConfig {
    workspace: PathBuf,
    api_key: Option<ResolvedValue>,
    model: Option<ResolvedValue>,
    base_url: String,
    verifier_api_key: Option<ResolvedValue>,
    verifier_model: Option<ResolvedValue>,
    verifier_base_url: Option<String>,
    max_agent_steps: Option<usize>,
    goal_limits: GoalLimits,
    web_search: bool,
    project_id: Option<String>,
    extra_read_roots: Vec<PathBuf>,
    extra_write_roots: Vec<PathBuf>,
}

pub struct ProviderSettings {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub web_search: bool,
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
            .map(|value| value.value)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let verifier_api_key = resolve_value(VERIFIER_OPENAI_API_KEY, &workspace_env, &user_env);
        let verifier_model = resolve_value(VERIFIER_OPENAI_MODEL, &workspace_env, &user_env);
        let verifier_base_url = resolve_value(VERIFIER_OPENAI_BASE_URL, &workspace_env, &user_env)
            .map(|value| value.value);
        let max_agent_steps = match resolve_value("MINI_AGENT_MAX_STEPS", &workspace_env, &user_env)
        {
            Some(value) => Some(parse_max_agent_steps(&value.value)?),
            None => None,
        };
        let goal_limits = GoalLimits {
            max_loops: resolve_positive_usize(
                "MINI_AGENT_GOAL_MAX_LOOPS",
                &workspace_env,
                &user_env,
                GoalLimits::default().max_loops,
            )?,
            milestone_step_budget: resolve_positive_usize(
                "MINI_AGENT_GOAL_STEP_BUDGET",
                &workspace_env,
                &user_env,
                GoalLimits::default().milestone_step_budget,
            )?,
            milestone_timeout_secs: resolve_positive_u64(
                "MINI_AGENT_GOAL_TIMEOUT_SECS",
                &workspace_env,
                &user_env,
                GoalLimits::default().milestone_timeout_secs,
            )?,
        };
        let web_search = match resolve_value("MINI_AGENT_WEB_SEARCH", &workspace_env, &user_env)
            .or_else(|| resolve_value("OPENAI_WEB_SEARCH", &workspace_env, &user_env))
        {
            Some(value) => parse_bool_setting("MINI_AGENT_WEB_SEARCH", &value.value)?,
            None => is_official_search_endpoint(&base_url),
        };
        let project_id = env::var("MINI_AGENT_PROJECT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let extra_read_roots = env_path_list("MINI_AGENT_EXTRA_READ_ROOTS");
        let extra_write_roots = env_path_list("MINI_AGENT_EXTRA_WRITE_ROOTS");
        Ok(Self {
            workspace,
            api_key,
            model,
            base_url,
            verifier_api_key,
            verifier_model,
            verifier_base_url,
            max_agent_steps,
            goal_limits,
            web_search,
            project_id,
            extra_read_roots,
            extra_write_roots,
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
        validate_base_url(&self.base_url)?;
        Ok(ProviderSettings {
            api_key,
            model,
            base_url: self.base_url.clone(),
            web_search: self.web_search,
        })
    }

    pub fn workspace(&self) -> PathBuf {
        self.workspace.clone()
    }

    pub fn project_id(&self) -> String {
        self.project_id
            .clone()
            .unwrap_or_else(|| self.workspace.display().to_string())
    }

    pub fn extra_read_roots(&self) -> Vec<PathBuf> {
        self.extra_read_roots.clone()
    }

    pub fn extra_write_roots(&self) -> Vec<PathBuf> {
        self.extra_write_roots.clone()
    }

    pub fn workspace_revision(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.workspace.hash(&mut hasher);
        self.extra_read_roots.hash(&mut hasher);
        self.extra_write_roots.hash(&mut hasher);
        hasher.finish()
    }

    pub fn copilot_max_steps(&self) -> usize {
        self.max_agent_steps.unwrap_or(0)
    }

    pub fn goal_limits(&self) -> GoalLimits {
        self.goal_limits
    }

    pub fn web_search(&self) -> bool {
        self.web_search
    }

    pub fn with_web_search(mut self, enabled: bool) -> Self {
        self.web_search = enabled;
        self
    }

    /// Resolves the separate tool-free provider used by Goal verification.
    pub fn verifier_provider_settings(&self) -> Result<ProviderSettings, String> {
        let model = self
            .verifier_model
            .as_ref()
            .ok_or_else(|| "VERIFIER_OPENAI_MODEL is required for Goal verification".to_string())?
            .value
            .clone();
        let api_key = self
            .verifier_api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| {
                "VERIFIER_OPENAI_API_KEY or OPENAI_API_KEY is required for Goal verification"
                    .to_string()
            })?
            .value
            .clone();
        let base_url = self.verifier_base_url.as_deref().unwrap_or(&self.base_url);
        validate_base_url_named(VERIFIER_OPENAI_BASE_URL, base_url)?;
        Ok(ProviderSettings {
            api_key,
            model,
            base_url: base_url.to_string(),
            web_search: false,
        })
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_ref().map(|model| model.value.as_str())
    }
}

fn is_official_search_endpoint(base_url: &str) -> bool {
    let val = base_url.to_ascii_lowercase();
    val.contains("api.openai.com") || val.contains("api.deepseek.com")
}

fn env_path_list(name: &str) -> Vec<PathBuf> {
    env::var_os(name)
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default()
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

fn parse_max_agent_steps(value: &str) -> Result<usize, String> {
    value.parse::<usize>().map_err(|_| {
        "MINI_AGENT_MAX_STEPS must be a non-negative integer (0 = unlimited)".to_string()
    })
}

fn resolve_positive_usize(
    name: &str,
    workspace: &Environment,
    user: &Environment,
    default: usize,
) -> Result<usize, String> {
    match resolve_value(name, workspace, user) {
        Some(value) => value
            .value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{name} must be a positive integer")),
        None => Ok(default),
    }
}

fn resolve_positive_u64(
    name: &str,
    workspace: &Environment,
    user: &Environment,
    default: u64,
) -> Result<u64, String> {
    match resolve_value(name, workspace, user) {
        Some(value) => value
            .value
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{name} must be a positive integer")),
        None => Ok(default),
    }
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
    fn non_official_provider_does_not_enable_builtin_web_search() {
        let workspace = unique_dir("workspace");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=k\nOPENAI_MODEL=test-model\nOPENAI_BASE_URL=https://example.com/api/v1\n",
        )
        .unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert!(!config.web_search());
        assert!(!is_official_search_endpoint(&config.base_url));
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
    }

    #[test]
    fn verifier_provider_settings_reads_canonical_names() {
        let workspace = unique_dir("verifier-canonical");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=primary-key\nOPENAI_MODEL=primary-model\nVERIFIER_OPENAI_API_KEY=verifier-key\nVERIFIER_OPENAI_MODEL=verifier-model\nVERIFIER_OPENAI_BASE_URL=http://verifier.test/v1\n",
        )
        .unwrap();

        let config = RuntimeConfig::load_from(workspace, None).unwrap();

        let provider = config.verifier_provider_settings().unwrap();
        assert_eq!(provider.api_key, "verifier-key");
        assert_eq!(provider.model, "verifier-model");
        assert_eq!(provider.base_url, "http://verifier.test/v1");
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

    #[test]
    fn goal_limits_read_workspace_env() {
        let workspace = unique_dir("goal-limits");
        fs::write(
            workspace.join(".env"),
            "MINI_AGENT_GOAL_MAX_LOOPS=2\nMINI_AGENT_GOAL_STEP_BUDGET=7\nMINI_AGENT_GOAL_TIMEOUT_SECS=3\n",
        )
        .unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert_eq!(
            config.goal_limits(),
            GoalLimits {
                max_loops: 2,
                milestone_step_budget: 7,
                milestone_timeout_secs: 3,
            }
        );
    }

    #[test]
    fn goal_limits_reject_zero_values() {
        let workspace = unique_dir("goal-limits-invalid");
        fs::write(workspace.join(".env"), "MINI_AGENT_GOAL_TIMEOUT_SECS=0\n").unwrap();
        let error = match RuntimeConfig::load_from(workspace, None) {
            Ok(_) => panic!("expected zero timeout to be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("MINI_AGENT_GOAL_TIMEOUT_SECS"));
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
