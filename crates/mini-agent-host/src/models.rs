//! Machine-wide Responses model catalog and Host-side model selection.

use fs2::FileExt;
use mini_agent_capabilities::{
    ApprovalController, ImageStore, ModelProviderSettings, OpenAiError, OpenAiModel,
};
use mini_agent_protocol::{
    Message, Model, ModelEvent, ModelEventSink, ModelRequest, ModelResponse, ModelSelection,
    ReasoningSelection,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const STORE_FILE: &str = "model_catalog.json";
const CREDENTIAL_DIRECTORY: &str = "provider-credentials";
const MAX_ID_BYTES: usize = 128;
const MAX_BASE_URL_BYTES: usize = 2048;
const REASONING_RESERVED_FIELDS: &str =
    "model instructions input tools tool_choice parallel_tool_calls store stream max_output_tokens";
const MODEL_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const MODEL_TEST_MAX_OUTPUT_TOKENS: usize = 32;
const MODEL_TEST_MAX_RESPONSE_BYTES: usize = 4 * 1024;
const MODEL_TEST_PROMPT: &str = "Reply with OK.";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    #[serde(rename = "deepseek")]
    DeepSeek,
    Kimi,
    Glm,
    Volcengine,
    Custom,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub id: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_parameter_map: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub smart_managed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// Responses-compatible endpoint root. Deliberately has no vendor default.
    pub base_url: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<ModelProfile>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelConnectionTestStatus {
    Succeeded,
    InvalidCredentials,
    ProviderRejected,
    TimedOut,
    Unreachable,
    InvalidResponse,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    #[serde(default = "catalog_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
    #[serde(default)]
    pub default_model: Option<ModelSelection>,
    #[serde(default)]
    pub default_reasoning_selection: ReasoningSelection,
    #[serde(default)]
    pub verifier_default_model: Option<ModelSelection>,
    #[serde(default)]
    pub project_defaults: BTreeMap<String, ModelSelection>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    pub profile: ProviderProfile,
    pub api_key_configured: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogView {
    pub providers: Vec<ProviderView>,
    pub default_model: Option<ModelSelection>,
    pub default_reasoning_selection: ReasoningSelection,
    pub verifier_default_model: Option<ModelSelection>,
    pub project_defaults: BTreeMap<String, ModelSelection>,
}

#[derive(Clone)]
pub struct ModelCatalogStore {
    path: PathBuf,
    credentials: FileCredentialStore,
}

#[derive(Clone)]
struct FileCredentialStore(PathBuf);

impl FileCredentialStore {
    fn path(&self, provider_id: &str) -> Result<PathBuf, String> {
        validate_identifier(provider_id, "providerId")?;
        Ok(self.0.join(format!("{provider_id}.key")))
    }

    fn configured(&self, provider_id: &str) -> Result<bool, String> {
        match self.path(provider_id)?.metadata() {
            Ok(metadata) => Ok(metadata.is_file() && metadata.len() > 0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err("cannot inspect provider credential file".to_string()),
        }
    }

    fn get(&self, provider_id: &str) -> Result<Option<String>, String> {
        match fs::read_to_string(self.path(provider_id)?) {
            Ok(value) if !value.is_empty() => Ok(Some(value)),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("cannot read provider credential file".to_string()),
        }
    }

    fn set(&self, provider_id: &str, api_key: &str) -> Result<(), String> {
        fs::create_dir_all(&self.0)
            .map_err(|_| "cannot create provider credential directory".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700))
                .map_err(|_| "cannot restrict provider credential directory".to_string())?;
        }
        let path = self.path(provider_id)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|_| "cannot write provider credential file".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|_| "cannot restrict provider credential file".to_string())?;
        }
        use std::io::Write;
        file.write_all(api_key.as_bytes())
            .map_err(|_| "cannot write provider credential file".to_string())?;
        drop(file);
        replace_catalog_file(&temporary, &path)
            .map_err(|_| "cannot replace provider credential file".to_string())
    }

    fn delete(&self, provider_id: &str) -> Result<(), String> {
        match fs::remove_file(self.path(provider_id)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("cannot delete provider credential file".to_string()),
        }
    }
}

impl ModelCatalogStore {
    pub fn machine_default() -> Result<Self, String> {
        let root = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or_else(|| "cannot resolve the user home directory".to_string())?;
        Ok(Self::at(root.join(".mini-agent").join(STORE_FILE)))
    }

    pub fn at(path: PathBuf) -> Self {
        Self {
            credentials: FileCredentialStore(path.with_file_name(CREDENTIAL_DIRECTORY)),
            path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<ModelCatalog, String> {
        let Some(parent) = self.path.parent() else {
            return Err("model catalog path has no parent directory".to_string());
        };
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create model catalog directory: {error}"))?;
        let _lock = self.lock()?;
        let catalog = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<ModelCatalog>(&bytes)
                .map_err(|error| format!("cannot parse model catalog: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ModelCatalog {
                version: catalog_version(),
                ..ModelCatalog::default()
            },
            Err(error) => return Err(format!("cannot read model catalog: {error}")),
        };
        if catalog.version != catalog_version() {
            return Err(format!(
                "unsupported model catalog version {}",
                catalog.version
            ));
        }
        Ok(catalog)
    }

    pub fn view(&self) -> Result<ModelCatalogView, String> {
        let catalog = self.read()?;
        let providers = catalog
            .providers
            .into_iter()
            .map(|profile| {
                Ok(ProviderView {
                    api_key_configured: self.credentials.configured(&profile.id)?,
                    profile,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(ModelCatalogView {
            providers,
            default_model: catalog.default_model,
            default_reasoning_selection: catalog.default_reasoning_selection,
            verifier_default_model: catalog.verifier_default_model,
            project_defaults: catalog.project_defaults,
        })
    }

    pub fn upsert_provider(
        &self,
        profile: ProviderProfile,
        api_key: Option<String>,
    ) -> Result<ModelCatalogView, String> {
        validate_provider(&profile)?;
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        let mut profile = profile;
        profile.models = catalog
            .providers
            .iter()
            .find(|item| item.id == profile.id)
            .map(|existing| existing.models.clone())
            .unwrap_or_default();
        if let Some(api_key) = api_key {
            if api_key.trim().is_empty() {
                self.credentials.delete(&profile.id)?;
            } else {
                self.credentials.set(&profile.id, &api_key)?;
            }
        }
        match catalog
            .providers
            .iter_mut()
            .find(|item| item.id == profile.id)
        {
            Some(existing) => *existing = profile,
            None => catalog.providers.push(profile),
        }
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn upsert_model(
        &self,
        provider_id: &str,
        model: ModelProfile,
        previous_model_id: Option<&str>,
    ) -> Result<ModelCatalogView, String> {
        validate_model(&model)?;
        let model_id = model.id.clone();
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        let renamed_from = {
            let provider = catalog
                .providers
                .iter_mut()
                .find(|provider| provider.id == provider_id)
                .ok_or_else(|| format!("provider {provider_id} does not exist"))?;
            if let Some(previous_model_id) = previous_model_id
                && previous_model_id != model.id
            {
                let index = provider
                    .models
                    .iter()
                    .position(|entry| entry.id == previous_model_id)
                    .ok_or_else(|| format!("model {previous_model_id} does not exist"))?;
                if provider.models.iter().any(|entry| entry.id == model.id) {
                    return Err(format!("model {} already exists", model.id));
                }
                provider.models[index] = model.clone();
                Some(previous_model_id.to_string())
            } else {
                match provider
                    .models
                    .iter_mut()
                    .find(|entry| entry.id == model.id)
                {
                    Some(existing) => *existing = model,
                    None => provider.models.push(model),
                }
                None
            }
        };
        if let Some(previous_model_id) = renamed_from {
            rename_model_references(&mut catalog, provider_id, &previous_model_id, &model_id);
        }
        validate_default_reasoning(&catalog)?;
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn delete_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<ModelCatalogView, String> {
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        let provider = catalog
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("provider {provider_id} does not exist"))?;
        provider.models.retain(|model| model.id != model_id);
        clear_defaults(&mut catalog, provider_id, model_id);
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn delete_provider(&self, provider_id: &str) -> Result<ModelCatalogView, String> {
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        catalog
            .providers
            .retain(|provider| provider.id != provider_id);
        if catalog
            .default_model
            .as_ref()
            .is_some_and(|model| model.provider_id == provider_id)
        {
            catalog.default_model = None;
            catalog.default_reasoning_selection = ReasoningSelection::ApiDefault;
        }
        if catalog
            .verifier_default_model
            .as_ref()
            .is_some_and(|model| model.provider_id == provider_id)
        {
            catalog.verifier_default_model = None;
        }
        catalog
            .project_defaults
            .retain(|_, model| model.provider_id != provider_id);
        self.credentials.delete(provider_id)?;
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn set_defaults(
        &self,
        default_model: Option<ModelSelection>,
        verifier_default_model: Option<ModelSelection>,
    ) -> Result<ModelCatalogView, String> {
        self.set_defaults_with_reasoning(
            default_model,
            ReasoningSelection::ApiDefault,
            verifier_default_model,
        )
    }

    pub fn set_defaults_with_reasoning(
        &self,
        default_model: Option<ModelSelection>,
        default_reasoning_selection: ReasoningSelection,
        verifier_default_model: Option<ModelSelection>,
    ) -> Result<ModelCatalogView, String> {
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        if let Some(selection) = &default_model {
            ensure_enabled_model(&catalog, selection)?;
        }
        if let Some(selection) = &verifier_default_model {
            ensure_enabled_model(&catalog, selection)?;
        }
        if default_model.is_some() && default_model == verifier_default_model {
            return Err("primary and Goal Verifier defaults must be different models".to_string());
        }
        if let Some(selection) = &default_model {
            validate_reasoning_selection(&catalog, selection, &default_reasoning_selection)?;
        } else if matches!(default_reasoning_selection, ReasoningSelection::Level(_)) {
            return Err("a default reasoning level requires a global default model".to_string());
        }
        catalog.default_model = default_model;
        catalog.default_reasoning_selection = default_reasoning_selection;
        catalog.verifier_default_model = verifier_default_model;
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn set_project_default(
        &self,
        project_id: String,
        selection: Option<ModelSelection>,
    ) -> Result<ModelCatalogView, String> {
        validate_identifier(&project_id, "projectId")?;
        let _lock = self.lock()?;
        let mut catalog = self.read_unlocked()?;
        if let Some(selection) = selection {
            ensure_enabled_model(&catalog, &selection)?;
            catalog.project_defaults.insert(project_id, selection);
        } else {
            catalog.project_defaults.remove(&project_id);
        }
        self.write_unlocked(&catalog)?;
        drop(_lock);
        self.view()
    }

    pub fn resolve(
        &self,
        selection: &ModelSelection,
    ) -> Result<(OpenAiModel, ModelProfile), String> {
        let catalog = self.read()?;
        let (provider, model) = find_model(&catalog, selection)?;
        validate_model(model)?;
        if !provider.enabled || !model.enabled {
            return Err(format!(
                "model {}/{} is disabled",
                selection.provider_id, selection.model_id
            ));
        }
        if provider.base_url.trim().is_empty() {
            return Err(format!(
                "Base URL is not configured for provider {}",
                provider.name
            ));
        }
        let api_key = self
            .credentials
            .get(&provider.id)?
            .ok_or_else(|| format!("API Key is not configured for provider {}", provider.name))?;
        let model_instance = OpenAiModel::new(
            api_key,
            model.id.clone(),
            provider.base_url.clone(),
            ImageStore::memory_only(),
        )
        .map_err(|error| error.to_string())?
        .with_model_options(
            model.max_output_tokens.map(|value| value as usize),
            model.reasoning_parameter_map.clone(),
        );
        Ok((model_instance, model.clone()))
    }

    pub fn provider_settings(
        &self,
        selection: &ModelSelection,
    ) -> Result<ModelProviderSettings, String> {
        self.provider_settings_with_overrides(selection, None, None)
    }

    pub fn provider_settings_with_overrides(
        &self,
        selection: &ModelSelection,
        api_key_override: Option<&str>,
        base_url_override: Option<&str>,
    ) -> Result<ModelProviderSettings, String> {
        let catalog = self.read()?;
        let (provider, model) = find_model(&catalog, selection)?;
        if !provider.enabled || !model.enabled {
            return Err(format!(
                "model {}/{} is disabled",
                selection.provider_id, selection.model_id
            ));
        }
        let api_key = match api_key_override.filter(|value| !value.trim().is_empty()) {
            Some(api_key) => api_key.to_string(),
            None => self.credentials.get(&provider.id)?.ok_or_else(|| {
                format!("API Key is not configured for provider {}", provider.name)
            })?,
        };
        let base_url = base_url_override
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&provider.base_url);
        Ok(ModelProviderSettings {
            api_key,
            model: model.id.clone(),
            base_url: base_url.to_string(),
        })
    }

    /// Sends one bounded, tool-free request to the selected configured model.
    /// Provider error bodies and response text are intentionally discarded.
    pub async fn test_connection(
        &self,
        selection: &ModelSelection,
    ) -> Result<ModelConnectionTestStatus, String> {
        self.test_connection_with_timeout(selection, MODEL_TEST_TIMEOUT)
            .await
    }

    async fn test_connection_with_timeout(
        &self,
        selection: &ModelSelection,
        timeout: std::time::Duration,
    ) -> Result<ModelConnectionTestStatus, String> {
        let (mut model, _) = self.resolve(selection)?;
        model = model.with_model_options(Some(MODEL_TEST_MAX_OUTPUT_TOKENS), BTreeMap::new());
        let messages = [Message::User {
            text: MODEL_TEST_PROMPT.to_string(),
        }];
        let request = ModelRequest {
            system_prompt: "Reply only with OK.",
            messages: &messages,
            tools: &[],
            allowed_tools: None,
            max_response_bytes: MODEL_TEST_MAX_RESPONSE_BYTES,
            model_selection: None,
            reasoning_selection: None,
            reasoning_effort: None,
        };
        let mut events = DiscardModelEvents;
        let result = tokio::time::timeout(timeout, model.respond(request, &mut events)).await;
        Ok(match result {
            Err(_) => ModelConnectionTestStatus::TimedOut,
            Ok(Ok(_)) => ModelConnectionTestStatus::Succeeded,
            Ok(Err(error)) => classify_connection_error(error),
        })
    }

    pub fn model_profile(&self, selection: &ModelSelection) -> Result<ModelProfile, String> {
        let catalog = self.read()?;
        let (provider, model) = find_model(&catalog, selection)?;
        if !provider.enabled || !model.enabled {
            return Err(format!(
                "model {}/{} is disabled",
                selection.provider_id, selection.model_id
            ));
        }
        Ok(model.clone())
    }

    pub fn primary_default(&self, project_id: &str) -> Result<Option<ModelSelection>, String> {
        let catalog = self.read()?;
        let selection = catalog
            .project_defaults
            .get(project_id)
            .or(catalog.default_model.as_ref())
            .cloned();
        if let Some(selection) = &selection {
            ensure_enabled_model(&catalog, selection)?;
        }
        Ok(selection)
    }

    pub fn project_default(&self, project_id: &str) -> Result<Option<ModelSelection>, String> {
        let catalog = self.read()?;
        let selection = catalog.project_defaults.get(project_id).cloned();
        if let Some(selection) = &selection {
            ensure_enabled_model(&catalog, selection)?;
        }
        Ok(selection)
    }

    pub fn global_default(&self) -> Result<Option<ModelSelection>, String> {
        let catalog = self.read()?;
        if let Some(selection) = &catalog.default_model {
            ensure_enabled_model(&catalog, selection)?;
        }
        Ok(catalog.default_model)
    }

    pub fn global_default_reasoning_selection(&self) -> Result<ReasoningSelection, String> {
        Ok(self.read()?.default_reasoning_selection)
    }

    pub fn verifier_default(&self) -> Result<Option<ModelSelection>, String> {
        let catalog = self.read()?;
        if let Some(selection) = &catalog.verifier_default_model {
            ensure_enabled_model(&catalog, selection)?;
        }
        Ok(catalog.verifier_default_model)
    }

    fn lock(&self) -> Result<File, String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "model catalog path has no parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create model catalog directory: {error}"))?;
        let lock_path = self.path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|error| format!("cannot open model catalog lock: {error}"))?;
        lock.lock_exclusive()
            .map_err(|error| format!("cannot lock model catalog: {error}"))?;
        Ok(lock)
    }

    fn read_unlocked(&self) -> Result<ModelCatalog, String> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<ModelCatalog>(&bytes)
                .map_err(|error| format!("cannot parse model catalog: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ModelCatalog {
                version: catalog_version(),
                ..ModelCatalog::default()
            }),
            Err(error) => Err(format!("cannot read model catalog: {error}")),
        }
    }

    fn write_unlocked(&self, catalog: &ModelCatalog) -> Result<(), String> {
        let parent = self.path.parent().expect("validated catalog parent");
        let bytes = serde_json::to_vec_pretty(catalog)
            .map_err(|error| format!("cannot serialize model catalog: {error}"))?;
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, bytes)
            .map_err(|error| format!("cannot write model catalog: {error}"))?;
        replace_catalog_file(&temporary, &self.path)
            .map_err(|error| format!("cannot replace model catalog: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("cannot secure model catalog permissions: {error}"))?;
        }
        let _ = parent;
        Ok(())
    }
}

/// Resolves a model reference at the Host boundary on every new Turn.
pub struct HostResponsesModel {
    catalog: ModelCatalogStore,
    project_id: String,
    images: ImageStore,
    approval: Option<ApprovalController>,
}

impl HostResponsesModel {
    pub fn new(catalog: ModelCatalogStore, project_id: String, images: ImageStore) -> Self {
        Self {
            catalog,
            project_id,
            images,
            approval: None,
        }
    }

    pub fn with_plan_mode_approval(mut self, approval: ApprovalController) -> Self {
        self.approval = Some(approval);
        self
    }
}

impl Model for HostResponsesModel {
    type Error = OpenAiError;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        let available_tool_names = request
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let tool_selection = crate::goal::plan_mode_tool_selection(
            self.approval
                .as_ref()
                .and_then(ApprovalController::session_dir)
                .as_deref(),
            &available_tool_names,
        )
        .map_err(OpenAiError::Protocol)?;
        if tool_selection
            .as_ref()
            .is_some_and(|selection| selection.review_pending)
        {
            return Err(OpenAiError::Protocol(
                "Plan review is pending; review the Session plan before continuing".to_string(),
            ));
        }
        let thread_reasoning = request.reasoning_selection.cloned();
        let legacy_reasoning = request.reasoning_effort.map(ReasoningSelection::level);
        let (selection, reasoning_selection) = match request.model_selection {
            Some(selection) => (
                Some(selection.clone()),
                Some(
                    thread_reasoning
                        .or(legacy_reasoning)
                        .unwrap_or(ReasoningSelection::ApiDefault),
                ),
            ),
            None => match self
                .catalog
                .project_default(&self.project_id)
                .map_err(OpenAiError::Protocol)?
            {
                Some(selection) => (
                    Some(selection),
                    Some(
                        thread_reasoning
                            .or(legacy_reasoning)
                            .unwrap_or(ReasoningSelection::ApiDefault),
                    ),
                ),
                None => (
                    self.catalog
                        .global_default()
                        .map_err(OpenAiError::Protocol)?,
                    Some(
                        thread_reasoning.or(legacy_reasoning).unwrap_or(
                            self.catalog
                                .global_default_reasoning_selection()
                                .map_err(OpenAiError::Protocol)?,
                        ),
                    ),
                ),
            },
        };
        if let Some(selection) = selection {
            let reasoning_selection = reasoning_selection.unwrap_or(ReasoningSelection::ApiDefault);
            let (mut model, profile) = self
                .catalog
                .resolve(&selection)
                .map_err(OpenAiError::Protocol)?;
            validate_reasoning_for_profile(&profile, &reasoning_selection)
                .map_err(OpenAiError::Protocol)?;
            model.set_images(self.images.clone());
            if let Some(tool_selection) = tool_selection {
                if model.supports_allowed_tools() {
                    let resolved_request = ModelRequest {
                        allowed_tools: Some(&tool_selection.allowed_tools),
                        reasoning_selection: Some(&reasoning_selection),
                        reasoning_effort: None,
                        ..request
                    };
                    model.respond(resolved_request, events).await
                } else {
                    let mut hinted_messages = request.messages.to_vec();
                    hinted_messages.push(Message::Context {
                        text: tool_selection_hint(&tool_selection.allowed_tools),
                    });
                    let resolved_request = ModelRequest {
                        messages: &hinted_messages,
                        allowed_tools: None,
                        reasoning_selection: Some(&reasoning_selection),
                        reasoning_effort: None,
                        ..request
                    };
                    model.respond(resolved_request, events).await
                }
            } else {
                let resolved_request = ModelRequest {
                    reasoning_selection: Some(&reasoning_selection),
                    reasoning_effort: None,
                    ..request
                };
                model.respond(resolved_request, events).await
            }
        } else {
            Err(OpenAiError::Protocol(
                "no enabled default Responses model is configured".to_string(),
            ))
        }
    }
}

fn tool_selection_hint(allowed_tools: &[String]) -> String {
    format!(
        "[Host tool selection] Plan Mode is active. Call only these tools: {}. If the set is empty, answer without calling tools. Host admission still enforces this selection.",
        if allowed_tools.is_empty() {
            "(none)".to_string()
        } else {
            allowed_tools.join(", ")
        }
    )
}

struct DiscardModelEvents;

impl ModelEventSink for DiscardModelEvents {
    fn emit(&mut self, _event: ModelEvent) {}
}

fn classify_connection_error(error: OpenAiError) -> ModelConnectionTestStatus {
    match error {
        OpenAiError::IdleTimeout => ModelConnectionTestStatus::TimedOut,
        OpenAiError::Transport(_) => ModelConnectionTestStatus::Unreachable,
        OpenAiError::Api {
            status: 401 | 403, ..
        } => ModelConnectionTestStatus::InvalidCredentials,
        OpenAiError::Api { .. } => ModelConnectionTestStatus::ProviderRejected,
        OpenAiError::IncompleteStream | OpenAiError::Stream(_) => {
            ModelConnectionTestStatus::InvalidResponse
        }
        OpenAiError::Protocol(_) => ModelConnectionTestStatus::Failed,
    }
}

fn find_model<'a>(
    catalog: &'a ModelCatalog,
    selection: &ModelSelection,
) -> Result<(&'a ProviderProfile, &'a ModelProfile), String> {
    let provider = catalog
        .providers
        .iter()
        .find(|provider| provider.id == selection.provider_id)
        .ok_or_else(|| format!("model provider {} does not exist", selection.provider_id))?;
    let model = provider
        .models
        .iter()
        .find(|model| model.id == selection.model_id)
        .ok_or_else(|| {
            format!(
                "model {}/{} does not exist",
                selection.provider_id, selection.model_id
            )
        })?;
    Ok((provider, model))
}

fn ensure_enabled_model(catalog: &ModelCatalog, selection: &ModelSelection) -> Result<(), String> {
    let (provider, model) = find_model(catalog, selection)?;
    if !provider.enabled || !model.enabled {
        return Err(format!(
            "model {}/{} is disabled",
            selection.provider_id, selection.model_id
        ));
    }
    Ok(())
}

fn validate_provider(provider: &ProviderProfile) -> Result<(), String> {
    validate_identifier(&provider.id, "providerId")?;
    if provider.name.trim().is_empty() || provider.name.len() > 128 {
        return Err("provider name must be between 1 and 128 bytes".to_string());
    }
    if provider.base_url.len() > MAX_BASE_URL_BYTES {
        return Err("baseUrl exceeds the 2048 byte limit".to_string());
    }
    if !provider.base_url.trim().is_empty() {
        let url = reqwest::Url::parse(provider.base_url.trim())
            .map_err(|error| format!("baseUrl is invalid: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("baseUrl must be an absolute http or https URL".to_string());
        }
    }
    for model in &provider.models {
        validate_model(model)?;
    }
    Ok(())
}

fn validate_model(model: &ModelProfile) -> Result<(), String> {
    validate_identifier(&model.id, "modelId")?;
    if model.name.trim().is_empty() || model.name.len() > 128 {
        return Err("model name must be between 1 and 128 bytes".to_string());
    }
    if model.context_window.is_some_and(|value| value == 0)
        || model.max_output_tokens.is_some_and(|value| value == 0)
    {
        return Err("model token limits must be positive integers".to_string());
    }
    if model
        .input_modalities
        .iter()
        .any(|value| !matches!(value.as_str(), "text" | "image" | "video" | "pdf"))
    {
        return Err("input modalities may contain text, image, video, or pdf".to_string());
    }
    if model.reasoning_levels.iter().any(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    }) {
        return Err("reasoning levels must be bounded identifiers".to_string());
    }
    if model
        .reasoning_levels
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != model.reasoning_levels.len()
    {
        return Err("reasoning levels must be unique".to_string());
    }
    for level in &model.reasoning_levels {
        let parameters = model.reasoning_parameter_map.get(level);
        if !matches!(level.as_str(), "low" | "medium" | "high" | "xhigh" | "max")
            && parameters
                .and_then(serde_json::Value::as_object)
                .is_none_or(serde_json::Map::is_empty)
        {
            return Err("non-standard reasoning levels require a parameter mapping".to_string());
        }
        if let Some(parameters) = parameters {
            let Some(parameters) = parameters.as_object() else {
                return Err("reasoning mappings must be objects".to_string());
            };
            if parameters.is_empty()
                || parameters.keys().any(|key| {
                    REASONING_RESERVED_FIELDS
                        .split_ascii_whitespace()
                        .any(|field| field == key.as_str())
                })
            {
                return Err(
                    "reasoning mappings cannot override Responses request fields".to_string(),
                );
            }
        }
    }
    Ok(())
}

fn validate_default_reasoning(catalog: &ModelCatalog) -> Result<(), String> {
    let Some(selection) = &catalog.default_model else {
        if matches!(
            catalog.default_reasoning_selection,
            ReasoningSelection::Level(_)
        ) {
            return Err("a default reasoning level requires a global default model".to_string());
        }
        return Ok(());
    };
    validate_reasoning_selection(catalog, selection, &catalog.default_reasoning_selection)
}

fn validate_reasoning_selection(
    catalog: &ModelCatalog,
    selection: &ModelSelection,
    reasoning: &ReasoningSelection,
) -> Result<(), String> {
    let (_, model) = find_model(catalog, selection)?;
    validate_reasoning_for_profile(model, reasoning)
}

fn validate_reasoning_for_profile(
    model: &ModelProfile,
    reasoning: &ReasoningSelection,
) -> Result<(), String> {
    if let ReasoningSelection::Level(level) = reasoning
        && !model
            .reasoning_levels
            .iter()
            .any(|supported| supported == level)
    {
        return Err(format!(
            "reasoning level {level} is not supported by model {}",
            model.id
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, name: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err(format!("{name} must be a bounded identifier"));
    }
    Ok(())
}

fn clear_defaults(catalog: &mut ModelCatalog, provider_id: &str, model_id: &str) {
    let matches = |selection: &ModelSelection| {
        selection.provider_id == provider_id && selection.model_id == model_id
    };
    if catalog.default_model.as_ref().is_some_and(matches) {
        catalog.default_model = None;
        catalog.default_reasoning_selection = ReasoningSelection::ApiDefault;
    }
    if catalog.verifier_default_model.as_ref().is_some_and(matches) {
        catalog.verifier_default_model = None;
    }
    catalog
        .project_defaults
        .retain(|_, selection| !matches(selection));
}

fn rename_model_references(
    catalog: &mut ModelCatalog,
    provider_id: &str,
    previous_model_id: &str,
    model_id: &str,
) {
    let rename = |selection: &mut ModelSelection| {
        if selection.provider_id == provider_id && selection.model_id == previous_model_id {
            selection.model_id = model_id.to_string();
        }
    };
    if let Some(selection) = catalog.default_model.as_mut() {
        rename(selection);
    }
    if let Some(selection) = catalog.verifier_default_model.as_mut() {
        rename(selection);
    }
    for selection in catalog.project_defaults.values_mut() {
        rename(selection);
    }
}

const fn default_enabled() -> bool {
    true
}

#[cfg(not(windows))]
fn replace_catalog_file(temporary: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_catalog_file(temporary: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

const fn catalog_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::{Message, ModelEvent, ModelUsage, ToolSpec};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    struct EventCollector(Vec<ModelEvent>);

    impl ModelEventSink for EventCollector {
        fn emit(&mut self, event: ModelEvent) {
            self.0.push(event);
        }
    }

    fn test_store(base_url: String) -> ModelCatalogStore {
        let root = crate::test_support::test_root();
        let store = ModelCatalogStore::at(root.join(STORE_FILE));
        store
            .upsert_provider(
                ProviderProfile {
                    id: "deepseek".to_string(),
                    name: "DeepSeek".to_string(),
                    kind: ProviderKind::DeepSeek,
                    base_url,
                    enabled: true,
                    models: Vec::new(),
                },
                Some("test-secret-key".to_string()),
            )
            .unwrap();
        store
            .upsert_model(
                "deepseek",
                ModelProfile {
                    id: "deepseek-test".to_string(),
                    name: "DeepSeek Test".to_string(),
                    enabled: true,
                    context_window: Some(64_000),
                    max_output_tokens: Some(4_000),
                    input_modalities: vec!["text".to_string()],
                    capabilities: Vec::new(),
                    reasoning_levels: vec!["high".to_string(), "disabled".to_string()],
                    reasoning_parameter_map: BTreeMap::from([(
                        "disabled".to_string(),
                        serde_json::json!({"reasoning": {"effort": "none"}}),
                    )]),
                    smart_managed: false,
                },
                None,
            )
            .unwrap();
        store
    }

    fn start_test_provider(
        status: u16,
        body: &'static str,
        delay: Duration,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let mut body_start = None;
            let mut body_length = 0;
            loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "client closed before sending the test request");
                request.extend_from_slice(&buffer[..read]);
                if body_start.is_none()
                    && let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n")
                {
                    body_start = Some(index + 4);
                    let headers = String::from_utf8_lossy(&request[..index]);
                    body_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or_default();
                }
                if body_start.is_some_and(|start| request.len() >= start + body_length) {
                    break;
                }
            }
            thread::sleep(delay);
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                404 => "Not Found",
                _ => "Error",
            };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .unwrap();
            stream.flush().unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{address}/v1"), server)
    }

    #[test]
    fn catalog_views_never_serialize_provider_credentials() {
        let store = test_store("https://example.test/v1".to_string());
        let view = store.view().unwrap();
        let json = serde_json::to_string(&view).unwrap();

        assert!(view.providers[0].api_key_configured);
        let stored_key = store.credentials.get("deepseek").unwrap();
        assert_eq!(stored_key.as_deref(), Some("test-secret-key"));
        assert!(serde_json::to_value(ProviderKind::DeepSeek).unwrap() == "deepseek");
        assert!(!json.contains("test-secret-key"));
        assert!(!json.contains("apiKey\""));
    }

    #[tokio::test]
    async fn connection_test_sends_one_bounded_tool_free_request() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n"
        );
        let (base_url, server) = start_test_provider(200, body, Duration::ZERO);
        let store = test_store(base_url);
        let status = store
            .test_connection(&ModelSelection::new("deepseek", "deepseek-test"))
            .await
            .unwrap();
        let request = server.join().unwrap();
        let request_body = request.split_once("\r\n\r\n").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(request_body).unwrap();

        assert_eq!(status, ModelConnectionTestStatus::Succeeded);
        assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
        assert_eq!(payload["tools"], serde_json::json!([]));
        assert_eq!(payload["max_output_tokens"], MODEL_TEST_MAX_OUTPUT_TOKENS);
        assert!(payload["input"].to_string().contains(MODEL_TEST_PROMPT));
    }

    #[tokio::test]
    async fn connection_test_classifies_auth_and_endpoint_errors_without_returning_the_body() {
        let (base_url, server) =
            start_test_provider(401, "private provider response body", Duration::ZERO);
        let store = test_store(base_url);
        let status = store
            .test_connection(&ModelSelection::new("deepseek", "deepseek-test"))
            .await
            .unwrap();
        let _request = server.join().unwrap();

        assert_eq!(status, ModelConnectionTestStatus::InvalidCredentials);
        assert_ne!(status, ModelConnectionTestStatus::Failed);

        let (base_url, server) =
            start_test_provider(404, "private endpoint response body", Duration::ZERO);
        let store = test_store(base_url);
        let status = store
            .test_connection(&ModelSelection::new("deepseek", "deepseek-test"))
            .await
            .unwrap();
        let _request = server.join().unwrap();

        assert_eq!(status, ModelConnectionTestStatus::ProviderRejected);
    }

    #[test]
    fn connection_test_classifies_transport_errors_as_unreachable() {
        let status =
            classify_connection_error(OpenAiError::Transport("connection refused".to_string()));

        assert_eq!(status, ModelConnectionTestStatus::Unreachable);
    }

    #[tokio::test]
    async fn connection_test_stops_at_its_timeout() {
        let (base_url, server) = start_test_provider(200, "", Duration::from_millis(80));
        let store = test_store(base_url);
        let status = store
            .test_connection_with_timeout(
                &ModelSelection::new("deepseek", "deepseek-test"),
                Duration::from_millis(10),
            )
            .await
            .unwrap();
        let _request = server.join().unwrap();

        assert_eq!(status, ModelConnectionTestStatus::TimedOut);
    }

    #[test]
    fn defaults_are_validated_and_deleted_models_clear_references() {
        let store = test_store("https://example.test/v1".to_string());
        let primary = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };

        assert!(
            store
                .set_defaults(Some(primary.clone()), Some(primary.clone()))
                .is_err()
        );
        store.set_defaults(Some(primary.clone()), None).unwrap();
        store
            .set_project_default("project-a".to_string(), Some(primary))
            .unwrap();
        let view = store.delete_model("deepseek", "deepseek-test").unwrap();

        assert!(view.default_model.is_none());
        assert!(view.project_defaults.is_empty());
    }

    #[test]
    fn global_default_stores_a_model_supported_reasoning_level() {
        let store = test_store("https://example.test/v1".to_string());
        let primary = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };

        let view = store
            .set_defaults_with_reasoning(
                Some(primary.clone()),
                ReasoningSelection::Level("disabled".to_string()),
                None,
            )
            .unwrap();
        assert_eq!(
            view.default_reasoning_selection,
            ReasoningSelection::Level("disabled".to_string())
        );
        let mut unsafe_profile = store.read().unwrap().providers[0].models[0].clone();
        unsafe_profile.reasoning_parameter_map.insert(
            "disabled".to_string(),
            serde_json::json!({"model": "untrusted-override"}),
        );
        assert!(
            store
                .upsert_model("deepseek", unsafe_profile, None)
                .is_err()
        );
        assert!(
            store
                .set_defaults_with_reasoning(
                    Some(primary),
                    ReasoningSelection::Level("unsupported".to_string()),
                    None,
                )
                .is_err()
        );
    }

    #[test]
    fn renaming_a_model_updates_all_default_references() {
        let store = test_store("https://example.test/v1".to_string());
        let primary = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };
        let verifier = ModelSelection {
            model_id: "deepseek-verifier".to_string(),
            ..primary.clone()
        };
        let mut verifier_profile = store.read().unwrap().providers[0].models[0].clone();
        verifier_profile.id = verifier.model_id.clone();
        store
            .upsert_model("deepseek", verifier_profile, None)
            .unwrap();
        store
            .set_defaults(Some(primary.clone()), Some(verifier))
            .unwrap();
        store
            .set_project_default("project-a".to_string(), Some(primary.clone()))
            .unwrap();

        let mut renamed = store.read().unwrap().providers[0].models[0].clone();
        renamed.id = "deepseek-renamed".to_string();
        let view = store
            .upsert_model("deepseek", renamed, Some("deepseek-test"))
            .unwrap();

        assert_eq!(view.default_model.unwrap().model_id, "deepseek-renamed");
        assert_eq!(
            view.verifier_default_model.unwrap().model_id,
            "deepseek-verifier"
        );
        assert_eq!(
            view.project_defaults["project-a"].model_id,
            "deepseek-renamed"
        );
        assert!(
            store.read().unwrap().providers[0]
                .models
                .iter()
                .all(|model| model.id != "deepseek-test")
        );
    }

    #[tokio::test]
    async fn host_model_routes_responses_stream_and_tool_calls_to_selected_provider() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let mut body_start = None;
            let mut body_length = 0;
            loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(
                    read > 0,
                    "client closed before sending the Responses request"
                );
                request.extend_from_slice(&buffer[..read]);
                if body_start.is_none()
                    && let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n")
                {
                    body_start = Some(index + 4);
                    let headers = String::from_utf8_lossy(&request[..index]);
                    body_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or_default();
                }
                if body_start.is_some_and(|start| request.len() >= start + body_length) {
                    break;
                }
            }
            let body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"mock answer\"}\n\n",
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call-1\",\"name\":\"lookup\",\"arguments\":\"{\\\"key\\\":\\\"value\\\"}\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            stream.flush().unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });

        let store = test_store(format!("http://{address}/v1"));
        let mut model =
            HostResponsesModel::new(store, "project-a".to_string(), ImageStore::memory_only());
        let selection = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };
        let messages = [Message::User {
            text: "inspect".to_string(),
        }];
        let tools = [ToolSpec {
            name: "lookup".to_string(),
            description: "Look up a value".to_string(),
            parameters: serde_json::json!({"type":"object"}),
        }];
        let mut events = EventCollector(Vec::new());
        let response = model
            .respond(
                ModelRequest {
                    system_prompt: "test",
                    messages: &messages,
                    tools: &tools,
                    allowed_tools: None,
                    max_response_bytes: 64 * 1024,
                    model_selection: Some(&selection),
                    reasoning_selection: None,
                    reasoning_effort: Some("high"),
                },
                &mut events,
            )
            .await
            .unwrap();
        let request = server.join().unwrap();
        let request_body = request.split_once("\r\n\r\n").unwrap().1;

        assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
        assert!(request.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("authorization")
                    && value.trim() == "Bearer test-secret-key"
            })
        }));
        assert!(!request_body.contains("test-secret-key"));
        let payload: serde_json::Value = serde_json::from_str(request_body).unwrap();
        assert_eq!(payload["model"], "deepseek-test");
        assert_eq!(payload["reasoning"]["effort"], "high");
        assert_eq!(payload["tools"][0]["name"], "lookup");
        assert_eq!(response.text, "mock answer");
        assert_eq!(response.tool_calls[0].name, "lookup");
        assert_eq!(response.tool_calls[0].arguments["key"], "value");
        assert_eq!(
            response.usage,
            Some(ModelUsage {
                input_tokens: 3,
                cached_input_tokens: None,
                output_tokens: 2,
            })
        );
        assert_eq!(
            events.0,
            vec![ModelEvent::TextDelta("mock answer".to_string())]
        );
    }

    #[tokio::test]
    async fn host_model_applies_plan_selection_without_removing_tool_definitions() {
        let (base_url, server) = start_test_provider(
            200,
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
            ),
            Duration::ZERO,
        );
        let store = test_store(base_url);
        let session_dir = crate::test_support::test_root();
        let session_file = session_dir.join("session.jsonl");
        std::fs::write(&session_file, "").unwrap();
        crate::goal::init_plan_mode_with_prompt(&session_dir, None).unwrap();
        let approval = ApprovalController::new(mini_agent_protocol::ApprovalPolicy::Automatic);
        approval.bind_session_file(&session_file);
        let mut model =
            HostResponsesModel::new(store, "project-a".to_string(), ImageStore::memory_only())
                .with_plan_mode_approval(approval);
        let selection = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };
        let messages = [Message::User {
            text: "review the workspace".to_string(),
        }];
        let tools = ["read_file", "shell", "delegate_task"].map(|name| ToolSpec {
            name: name.to_string(),
            description: format!("Run {name}."),
            parameters: serde_json::json!({"type": "object"}),
        });
        let mut events = EventCollector(Vec::new());

        model
            .respond(
                ModelRequest {
                    system_prompt: "test",
                    messages: &messages,
                    tools: &tools,
                    allowed_tools: None,
                    max_response_bytes: 64 * 1024,
                    model_selection: Some(&selection),
                    reasoning_selection: None,
                    reasoning_effort: None,
                },
                &mut events,
            )
            .await
            .unwrap();
        let request = server.join().unwrap();
        let request_body = request.split_once("\r\n\r\n").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(request_body).unwrap();

        assert_eq!(payload["tools"].as_array().unwrap().len(), tools.len());
        assert_eq!(
            payload["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["read_file", "shell", "delegate_task"]
        );
        assert_eq!(
            payload["tool_choice"]["tools"],
            serde_json::json!([
                {"type": "function", "name": "read_file"},
                {"type": "function", "name": "shell"}
            ])
        );
        std::fs::remove_dir_all(session_dir).unwrap();
    }

    #[test]
    fn non_native_tool_selection_hint_names_only_allowed_tools() {
        let allowed_tools = vec!["read_file".to_string(), "shell".to_string()];
        let hint = tool_selection_hint(&allowed_tools);

        assert!(hint.contains("Call only these tools: read_file, shell"));
        assert!(hint.contains("Host admission still enforces this selection"));
        assert!(!hint.contains("delegate_task"));
    }

    #[tokio::test]
    async fn host_model_does_not_issue_provider_request_while_plan_review_is_pending() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();

        let session_dir = crate::test_support::test_root();
        let session_file = session_dir.join("session.jsonl");
        std::fs::write(&session_file, "").unwrap();
        crate::goal::init_plan_mode_with_prompt(&session_dir, None).unwrap();
        crate::goal::set_plan_review_pending(&session_dir, true).unwrap();
        let approval = ApprovalController::new(mini_agent_protocol::ApprovalPolicy::Automatic);
        approval.bind_session_file(&session_file);
        let catalog = ModelCatalogStore::at(session_dir.join(STORE_FILE));
        let mut model =
            HostResponsesModel::new(catalog, "project-a".to_string(), ImageStore::memory_only())
                .with_plan_mode_approval(approval);
        let messages = [Message::User {
            text: "review the plan".to_string(),
        }];
        let tools = [ToolSpec {
            name: "read_file".to_string(),
            description: "Read a workspace file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let mut events = EventCollector(Vec::new());

        let result = model
            .respond(
                ModelRequest {
                    system_prompt: "test",
                    messages: &messages,
                    tools: &tools,
                    allowed_tools: None,
                    max_response_bytes: 64 * 1024,
                    model_selection: None,
                    reasoning_selection: None,
                    reasoning_effort: None,
                },
                &mut events,
            )
            .await;

        assert!(matches!(
            result,
            Err(OpenAiError::Protocol(message)) if message.contains("Plan review is pending")
        ));
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
        std::fs::remove_dir_all(session_dir).unwrap();
    }

    #[tokio::test]
    async fn host_model_uses_legacy_reasoning_effort_with_default_model() {
        let (base_url, server) = start_test_provider(
            200,
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
            ),
            Duration::ZERO,
        );
        let store = test_store(base_url);
        let selection = ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-test".to_string(),
        };
        store
            .set_defaults_with_reasoning(Some(selection), ReasoningSelection::ApiDefault, None)
            .unwrap();
        let mut model =
            HostResponsesModel::new(store, "project-a".to_string(), ImageStore::memory_only());
        let messages = [Message::User {
            text: "inspect".to_string(),
        }];
        let mut events = EventCollector(Vec::new());

        model
            .respond(
                ModelRequest {
                    system_prompt: "test",
                    messages: &messages,
                    tools: &[],
                    allowed_tools: None,
                    max_response_bytes: 64 * 1024,
                    model_selection: None,
                    reasoning_selection: None,
                    reasoning_effort: Some("high"),
                },
                &mut events,
            )
            .await
            .unwrap();

        let request = server.join().unwrap();
        let request_body = request.split_once("\r\n\r\n").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(request_body).unwrap();
        assert_eq!(payload["model"], "deepseek-test");
        assert_eq!(payload["reasoning"]["effort"], "high");
    }
}
