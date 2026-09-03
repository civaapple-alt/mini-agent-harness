//! Thread-owned settings request boundary.

use crate::action::{ActionFailure, ActionResponse};
use crate::runtime_command::{RuntimeCommand, RuntimeCommandClient};
use mini_agent_host::BuiltinToolSelection;

/// App Server settings boundary for one Thread runtime.
#[derive(Clone)]
pub struct ThreadSettingsService {
    client: Option<RuntimeCommandClient>,
    stable_system_prompt: Option<String>,
}

impl ThreadSettingsService {
    pub fn new() -> Self {
        Self {
            client: None,
            stable_system_prompt: None,
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
    ) -> Self {
        Self {
            client: Some(client),
            stable_system_prompt,
        }
    }

    pub(crate) async fn update_action(
        &self,
        active: bool,
        builtin_tools: Option<BuiltinToolSelection>,
    ) -> Result<ActionResponse<Vec<String>>, ActionFailure> {
        let client = self.client.as_ref().ok_or_else(|| {
            ActionFailure::without_receipt(crate::AppServerError::RuntimeUnavailable)
        })?;
        client
            .request_action(|reply| RuntimeCommand::ThreadSettingsUpdate {
                active,
                builtin_tools,
                reply,
            })
            .await
    }
}

impl Default for ThreadSettingsService {
    fn default() -> Self {
        Self::new()
    }
}
