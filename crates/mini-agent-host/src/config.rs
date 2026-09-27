use crate::env_file::Environment;
use crate::env_file::ResolvedValue;
use crate::env_file::ValueSource;
use crate::goal::GoalLimits;
use mini_agent_capabilities::{ImageStore, OpenAiModel};
use mini_agent_protocol::ModelSelection;
use std::env;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;
use std::str::FromStr;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone)]
pub struct RuntimeConfig {
    workspace: PathBuf,
    goal_limits: GoalLimits,
    web_search: bool,
    project_id: Option<String>,
    extra_read_roots: Vec<PathBuf>,
    extra_write_roots: Vec<PathBuf>,
    session_read_roots: Vec<PathBuf>,
    builtin_skill_groups: Vec<String>,
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
        let goal_limits = GoalLimits {
            max_loops: resolve_positive(
                "MINI_AGENT_GOAL_MAX_LOOPS",
                &workspace_env,
                &user_env,
                GoalLimits::default().max_loops,
            )?,
            milestone_step_budget: resolve_positive(
                "MINI_AGENT_GOAL_STEP_BUDGET",
                &workspace_env,
                &user_env,
                GoalLimits::default().milestone_step_budget,
            )?,
            milestone_timeout_secs: resolve_positive(
                "MINI_AGENT_GOAL_TIMEOUT_SECS",
                &workspace_env,
                &user_env,
                GoalLimits::default().milestone_timeout_secs,
            )?,
        };
        let project_id = env::var("MINI_AGENT_PROJECT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let extra_read_roots = env_path_list("MINI_AGENT_EXTRA_READ_ROOTS");
        let extra_write_roots = env_path_list("MINI_AGENT_EXTRA_WRITE_ROOTS");
        let session_read_roots = env_path_list("MINI_AGENT_SESSION_READ_ROOTS");
        let builtin_skill_groups =
            parse_builtin_skill_groups(env::var("MINI_AGENT_BUILTIN_SKILL_GROUPS").ok());
        Ok(Self {
            workspace,
            goal_limits,
            web_search: true,
            project_id,
            extra_read_roots,
            extra_write_roots,
            session_read_roots,
            builtin_skill_groups,
        })
    }

    pub fn provider_settings(&self) -> Result<ProviderSettings, String> {
        let catalog = crate::models::ModelCatalogStore::machine_default()?;
        self.provider_settings_from(&catalog)
    }

    fn provider_settings_from(
        &self,
        catalog: &crate::models::ModelCatalogStore,
    ) -> Result<ProviderSettings, String> {
        let selected = catalog
            .primary_default(&self.project_id())?
            .and_then(|selection| catalog.provider_settings(&selection, true).ok());
        Ok(match selected {
            Some(mut settings) => {
                // The CLI switch can disable provider search, while provider
                // configuration and endpoint detection decide whether it is available.
                settings.web_search &= self.web_search;
                ProviderSettings {
                    api_key: settings.api_key,
                    model: settings.model,
                    base_url: settings.base_url,
                    web_search: settings.web_search,
                }
            }
            None => ProviderSettings {
                // Lets management APIs and first-run Studio start without a model.
                // HostResponsesModel rejects turns until a usable catalog default exists.
                api_key: String::new(),
                model: "unconfigured".to_string(),
                base_url: DEFAULT_BASE_URL.to_string(),
                web_search: self.web_search,
            },
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

    pub fn session_read_roots(&self) -> Vec<PathBuf> {
        self.session_read_roots.clone()
    }

    pub fn builtin_skill_groups(&self) -> Vec<String> {
        self.builtin_skill_groups.clone()
    }

    pub fn workspace_revision(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.workspace.hash(&mut hasher);
        self.extra_read_roots.hash(&mut hasher);
        self.builtin_skill_groups.hash(&mut hasher);
        self.extra_write_roots.hash(&mut hasher);
        self.session_read_roots.hash(&mut hasher);
        hasher.finish()
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
        let catalog = crate::models::ModelCatalogStore::machine_default()?;
        let selection = catalog.verifier_default()?.ok_or_else(|| {
            "configure a Goal Verifier default model in Web Studio model settings".to_string()
        })?;
        catalog
            .provider_settings(&selection, false)
            .map(|settings| ProviderSettings {
                api_key: settings.api_key,
                model: settings.model,
                base_url: settings.base_url,
                web_search: false,
            })
    }

    pub fn verifier_model_for(
        &self,
        selection: Option<&ModelSelection>,
    ) -> Result<OpenAiModel, String> {
        let catalog = crate::models::ModelCatalogStore::machine_default()?;
        self.verifier_model_for_catalog(selection, &catalog)
    }

    fn verifier_model_for_catalog(
        &self,
        selection: Option<&ModelSelection>,
        catalog: &crate::models::ModelCatalogStore,
    ) -> Result<OpenAiModel, String> {
        let selection = match selection {
            Some(selection) => selection.clone(),
            None => catalog.verifier_default()?.ok_or_else(|| {
                "configure a Goal Verifier default model in Web Studio model settings".to_string()
            })?,
        };
        let provider = catalog.provider_settings(&selection, false)?;
        let profile = catalog.model_profile(&selection)?;
        OpenAiModel::new(
            provider.api_key,
            provider.model,
            provider.base_url,
            false,
            ImageStore::memory_only(),
        )
        .map(|model| {
            model.with_model_options(
                profile.max_output_tokens.map(|value| value as usize),
                profile.reasoning_parameter_map,
            )
        })
        .map_err(|error| error.to_string())
    }
}

fn env_path_list(name: &str) -> Vec<PathBuf> {
    env::var_os(name)
        .map(|value| {
            env::split_paths(&value)
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_positive<T>(
    name: &str,
    workspace: &Environment,
    user: &Environment,
    default: T,
) -> Result<T, String>
where
    T: Default + FromStr + PartialOrd,
{
    match resolve_value(name, workspace, user) {
        Some(value) => value
            .value
            .parse::<T>()
            .ok()
            .filter(|value| *value > T::default())
            .ok_or_else(|| format!("{name} must be a positive integer")),
        None => Ok(default),
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
        .or_else(|| (cfg!(windows)).then(|| env::var_os("HOME")).flatten())
        .map(PathBuf::from)
}

fn parse_builtin_skill_groups(value: Option<String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|group| !group.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["pstack".to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelCatalogStore, ModelProfile, ProviderKind, ProviderProfile};
    use std::fs;

    #[test]
    fn legacy_provider_environment_values_do_not_configure_the_runtime() {
        let workspace = unique_dir("workspace");
        fs::write(
            workspace.join(".env"),
            "OPENAI_API_KEY=ignored-key\nOPENAI_MODEL=ignored-model\nOPENAI_BASE_URL=https://example.com/v1\nVERIFIER_OPENAI_API_KEY=ignored-verifier\nVERIFIER_OPENAI_MODEL=ignored-verifier-model\nVERIFIER_OPENAI_BASE_URL=https://example.com/v1\nMINI_AGENT_WEB_SEARCH=false\n",
        )
        .unwrap();
        let store = ModelCatalogStore::at(workspace.join(".mini-agent/model_catalog.json"));
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        let provider = config.provider_settings_from(&store).unwrap();
        assert!(provider.api_key.is_empty());
        assert_eq!(provider.model, "unconfigured");
        assert!(config.web_search());
    }

    #[test]
    fn goal_verifier_requires_a_catalog_default_even_when_legacy_values_exist() {
        let workspace = unique_dir("verifier-legacy-ignored");
        fs::write(
            workspace.join(".env"),
            "VERIFIER_OPENAI_API_KEY=ignored-key\nVERIFIER_OPENAI_MODEL=ignored-model\n",
        )
        .unwrap();
        let store = ModelCatalogStore::at(workspace.join(".mini-agent/model_catalog.json"));
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        let error = match config.verifier_model_for_catalog(None, &store) {
            Ok(_) => panic!("legacy verifier environment values must not configure the model"),
            Err(error) => error,
        };
        assert!(error.contains("Goal Verifier default model"));
    }

    #[test]
    fn provider_search_defaults_to_endpoint_detection_and_can_be_disabled() {
        let workspace = unique_dir("search-setting");
        let store = ModelCatalogStore::at(workspace.join(".mini-agent/model_catalog.json"));
        let provider = ProviderProfile {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            kind: ProviderKind::DeepSeek,
            base_url: "https://api.deepseek.com".to_string(),
            enabled: true,
            web_search: None,
            models: Vec::new(),
        };
        store
            .upsert_provider(provider, Some("key".to_string()))
            .unwrap();
        store
            .upsert_model(
                "deepseek",
                ModelProfile {
                    id: "search-model".to_string(),
                    name: "Search Model".to_string(),
                    enabled: true,
                    context_window: None,
                    max_output_tokens: None,
                    input_modalities: vec!["text".to_string()],
                    capabilities: vec!["web_search".to_string()],
                    reasoning_levels: Vec::new(),
                    reasoning_parameter_map: Default::default(),
                    smart_managed: false,
                },
                None,
            )
            .unwrap();
        let selection = ModelSelection::new("deepseek", "search-model");
        let config = RuntimeConfig::load_from(workspace, None).unwrap();
        assert!(
            store
                .provider_settings(&selection, config.web_search())
                .unwrap()
                .web_search
        );
        assert!(
            !store
                .provider_settings(&selection, config.with_web_search(false).web_search())
                .unwrap()
                .web_search
        );
    }

    #[test]
    fn runtime_provider_settings_preserve_explicit_search_choice() {
        let workspace = unique_dir("explicit-search-setting");
        let store = ModelCatalogStore::at(workspace.join(".mini-agent/model_catalog.json"));
        let selection = ModelSelection::new("custom", "search-model");
        store
            .upsert_provider(
                ProviderProfile {
                    id: "custom".to_string(),
                    name: "Custom".to_string(),
                    kind: ProviderKind::Custom,
                    base_url: "https://example.test/v1".to_string(),
                    enabled: true,
                    web_search: Some(false),
                    models: Vec::new(),
                },
                Some("key".to_string()),
            )
            .unwrap();
        store
            .upsert_model(
                "custom",
                ModelProfile {
                    id: "search-model".to_string(),
                    name: "Search Model".to_string(),
                    enabled: true,
                    context_window: None,
                    max_output_tokens: None,
                    input_modalities: vec!["text".to_string()],
                    capabilities: vec!["web_search".to_string()],
                    reasoning_levels: Vec::new(),
                    reasoning_parameter_map: Default::default(),
                    smart_managed: false,
                },
                None,
            )
            .unwrap();
        store.set_defaults(Some(selection), None).unwrap();
        let config = RuntimeConfig::load_from(workspace.clone(), None).unwrap();

        assert!(!config.provider_settings_from(&store).unwrap().web_search);

        store
            .upsert_provider(
                ProviderProfile {
                    id: "custom".to_string(),
                    name: "Custom".to_string(),
                    kind: ProviderKind::Custom,
                    base_url: "https://example.test/v1".to_string(),
                    enabled: true,
                    web_search: Some(true),
                    models: Vec::new(),
                },
                None,
            )
            .unwrap();
        assert!(config.provider_settings_from(&store).unwrap().web_search);
    }

    #[test]
    fn primary_chat_model_does_not_require_a_goal_verifier() {
        let workspace = unique_dir("primary-without-verifier");
        let store = ModelCatalogStore::at(workspace.join(".mini-agent/model_catalog.json"));
        let provider = ProviderProfile {
            id: "custom".to_string(),
            name: "Custom".to_string(),
            kind: ProviderKind::Custom,
            base_url: "https://example.test/v1".to_string(),
            enabled: true,
            web_search: None,
            models: Vec::new(),
        };
        store
            .upsert_provider(provider, Some("test-key".to_string()))
            .unwrap();
        store
            .upsert_model(
                "custom",
                ModelProfile {
                    id: "chat-model".to_string(),
                    name: "Chat Model".to_string(),
                    enabled: true,
                    context_window: None,
                    max_output_tokens: None,
                    input_modalities: vec!["text".to_string()],
                    capabilities: Vec::new(),
                    reasoning_levels: Vec::new(),
                    reasoning_parameter_map: Default::default(),
                    smart_managed: false,
                },
                None,
            )
            .unwrap();
        let selection = ModelSelection::new("custom", "chat-model");
        store.set_defaults(Some(selection), None).unwrap();
        let config = RuntimeConfig::load_from(workspace, None).unwrap();

        let primary = config.provider_settings_from(&store).unwrap();
        assert_eq!(primary.model, "chat-model");
        assert_eq!(primary.api_key, "test-key");
        assert!(config.verifier_model_for_catalog(None, &store).is_err());
    }

    #[test]
    fn workspace_goal_limits_override_user_env_limits() {
        let workspace = unique_dir("workspace");
        fs::write(workspace.join(".env"), "MINI_AGENT_GOAL_MAX_LOOPS=2\n").unwrap();
        let user_env = unique_dir("user").join(".env");
        fs::write(&user_env, "MINI_AGENT_GOAL_MAX_LOOPS=9\n").unwrap();

        let config = RuntimeConfig::load_from(workspace, Some(user_env)).unwrap();

        assert_eq!(config.goal_limits().max_loops, 2);
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

    #[test]
    fn builtin_skill_group_config_distinguishes_empty_from_absent() {
        assert_eq!(parse_builtin_skill_groups(None), vec!["pstack".to_string()]);
        assert!(parse_builtin_skill_groups(Some(String::new())).is_empty());
        assert_eq!(
            parse_builtin_skill_groups(Some("pstack, other, pstack".to_string())),
            vec![
                "pstack".to_string(),
                "other".to_string(),
                "pstack".to_string()
            ]
        );
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
