//! Thread-owned settings request boundary.

use crate::action::{ActionFailure, ActionResponse};
use crate::runtime_command::{RuntimeCommand, RuntimeCommandClient};
use mini_agent_app_server_protocol::ContinuationMode;
use mini_agent_host::BuiltinToolSelection;
use mini_agent_protocol::ModelSelection;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreadModelSettings {
    pub(crate) selection: Option<ModelSelection>,
    pub(crate) reasoning_effort: Option<String>,
}

/// App Server settings boundary for one Thread runtime.
#[derive(Clone)]
pub struct ThreadSettingsService {
    client: Option<RuntimeCommandClient>,
    stable_system_prompt: Option<String>,
    model_settings: Arc<RwLock<ThreadModelSettings>>,
}

impl ThreadSettingsService {
    pub fn new() -> Self {
        Self {
            client: None,
            stable_system_prompt: None,
            model_settings: Arc::new(RwLock::new(ThreadModelSettings::default())),
        }
    }

    /// Associates the bounded Host prompt used when collaboration mode changes.
    pub fn with_stable_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.stable_system_prompt = Some(prompt.into());
        self
    }

    pub(crate) fn stable_system_prompt(&self) -> Option<&str> {
        self.stable_system_prompt.as_deref()
    }

    pub(crate) fn bound(
        client: RuntimeCommandClient,
        stable_system_prompt: Option<String>,
        model_settings: Arc<RwLock<ThreadModelSettings>>,
    ) -> Self {
        Self {
            client: Some(client),
            stable_system_prompt,
            model_settings,
        }
    }

    pub(crate) fn model_settings(&self) -> ThreadModelSettings {
        self.model_settings.read().unwrap().clone()
    }

    pub(crate) fn model_settings_handle(&self) -> Arc<RwLock<ThreadModelSettings>> {
        Arc::clone(&self.model_settings)
    }

    pub(crate) fn set_initial_model_settings(&self, settings: ThreadModelSettings) {
        *self.model_settings.write().unwrap() = settings;
    }

    pub(crate) async fn update_action(
        &self,
        active: Option<bool>,
        builtin_tools: Option<BuiltinToolSelection>,
        continuation_mode: Option<ContinuationMode>,
        model_selection: Option<Option<ModelSelection>>,
        reasoning_effort: Option<Option<String>>,
    ) -> Result<ActionResponse<crate::management::ThreadSettingsRuntimeSnapshot>, ActionFailure>
    {
        let client = self.client.as_ref().ok_or_else(|| {
            ActionFailure::without_receipt(crate::AppServerError::RuntimeUnavailable)
        })?;
        let response = client
            .request_action(|reply| RuntimeCommand::ThreadSettingsUpdate {
                active,
                builtin_tools,
                continuation_mode,
                model_selection,
                reasoning_effort,
                reply,
            })
            .await?;
        *self.model_settings.write().unwrap() = ThreadModelSettings {
            selection: response.value.model_selection.clone(),
            reasoning_effort: response.value.reasoning_effort.clone(),
        };
        Ok(response)
    }
}

impl Default for ThreadSettingsService {
    fn default() -> Self {
        Self::new()
    }
}
