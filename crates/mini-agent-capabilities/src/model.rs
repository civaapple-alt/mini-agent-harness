use crate::image::ImageStore;
use crate::openai::OpenAiError;
use crate::openai::OpenAiModel;

/// Provider settings after secret resolution at the local application edge.
///
/// Credentials are consumed by the provider and are never serialized as part
/// of a profile or capability manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelProviderSettings {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub web_search: bool,
}

/// Builds the selected concrete model provider.
///
/// The first extracted provider is OpenAI-compatible. Keeping selection here
/// gives Host and future frontends one registry seam without exposing the
/// provider's HTTP client or credentials to the App Server protocol.
pub fn build_model(
    provider_id: &str,
    settings: ModelProviderSettings,
    images: ImageStore,
) -> Result<OpenAiModel, OpenAiError> {
    match provider_id {
        crate::OPENAI_MODEL_PROVIDER => OpenAiModel::new(
            settings.api_key,
            settings.model,
            settings.base_url,
            settings.web_search,
            images,
        ),
        other => Err(OpenAiError::Protocol(format!(
            "unknown model provider: {other}"
        ))),
    }
}
