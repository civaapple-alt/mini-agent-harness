mod responses;

use crate::image::ImageStore;
use crate::image::ProjectedImage;
use crate::image::project_images;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use mini_agent_protocol::Model;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelUsage;
use mini_agent_protocol::ToolCall;
use reqwest::Client;
use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

pub struct OpenAiModel {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
    web_search: bool,
    images: ImageStore,
    max_output_tokens: Option<usize>,
}

impl OpenAiModel {
    pub fn new(
        api_key: String,
        model: String,
        base_url: String,
        web_search: bool,
        images: ImageStore,
    ) -> Result<Self, OpenAiError> {
        let base_url = trim_base(&base_url);
        if base_url.is_empty() {
            return Err(OpenAiError::Protocol(
                "OPENAI_BASE_URL must not be empty".to_string(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| OpenAiError::Transport(error.to_string()))?;
        Ok(Self {
            client,
            api_key,
            model,
            endpoint: format!("{base_url}/responses"),
            web_search,
            images,
            max_output_tokens: None,
        })
    }
}

impl Model for OpenAiModel {
    type Error = OpenAiError;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        responses::complete(self, &request, events).await
    }
}

fn trim_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn project_for_request(
    request: &ModelRequest<'_>,
    images: &ImageStore,
) -> (Vec<Option<ProjectedImage>>, bool) {
    let attach_images = !request.tools.is_empty();
    let tool_contents = request
        .messages
        .iter()
        .filter_map(|message| match message {
            mini_agent_protocol::Message::Tool { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let projected = if attach_images {
        project_images(&tool_contents, images)
    } else {
        vec![None; tool_contents.len()]
    };
    let has_live_image = projected.iter().any(|image| {
        matches!(
            image,
            Some(ProjectedImage::FileId(_) | ProjectedImage::Inline { .. })
        )
    });
    (projected, has_live_image)
}

async fn post_json(
    client: &Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> Result<reqwest::Response, OpenAiError> {
    let response = client
        .post(url)
        .bearer_auth(api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| OpenAiError::Transport(error.to_string()))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = bounded_error_body(response).await;
        return Err(OpenAiError::Api {
            status: status.as_u16(),
            message: body,
        });
    }
    Ok(response)
}

async fn drain_sse(
    response: reqwest::Response,
    max_event_bytes: usize,
    complete_on_done: bool,
    mut on_event: impl FnMut(Value) -> Result<(), OpenAiError>,
) -> Result<bool, OpenAiError> {
    let mut stream = response.bytes_stream().eventsource();
    let mut completed_on_done = false;
    while let Some(event) = stream.next().await {
        let event = event.map_err(|error| OpenAiError::Transport(error.to_string()))?;
        if event.data == "[DONE]" {
            completed_on_done = complete_on_done;
            break;
        }
        if event.data.len() > max_event_bytes {
            return Err(OpenAiError::Protocol(format!(
                "SSE event exceeds {max_event_bytes} byte limit"
            )));
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|error| OpenAiError::Protocol(format!("invalid SSE JSON: {error}")))?;
        on_event(value)?;
    }
    Ok(completed_on_done)
}

fn max_event_bytes(max_response_bytes: usize) -> usize {
    max_response_bytes
        .saturating_mul(4)
        .saturating_add(64 * 1024)
}

async fn bounded_error_body(response: reqwest::Response) -> String {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() == MAX_ERROR_BODY_BYTES {
            break;
        }
    }
    if bytes.is_empty() {
        "response body unavailable".to_string()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

struct Accumulator {
    reasoning: String,
    text: String,
    tool_calls: Vec<ToolCall>,
    usage: Option<ModelUsage>,
    completed: bool,
    retained_bytes: usize,
    max_response_bytes: usize,
}

impl Accumulator {
    fn new(max_response_bytes: usize) -> Self {
        Self {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: Vec::new(),
            usage: None,
            completed: false,
            retained_bytes: 0,
            max_response_bytes,
        }
    }

    fn retain(&mut self, bytes: usize) -> Result<(), OpenAiError> {
        let actual = self.retained_bytes.saturating_add(bytes);
        if actual > self.max_response_bytes {
            return Err(OpenAiError::Protocol(format!(
                "model response exceeds {} byte limit",
                self.max_response_bytes
            )));
        }
        self.retained_bytes = actual;
        Ok(())
    }

    fn into_response(self) -> ModelResponse {
        ModelResponse {
            reasoning: self.reasoning,
            text: self.text,
            tool_calls: self.tool_calls,
            usage: self.usage,
        }
    }
}

#[derive(Debug)]
pub enum OpenAiError {
    Transport(String),
    Api { status: u16, message: String },
    Stream(String),
    Protocol(String),
}

impl fmt::Display for OpenAiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(message) => write!(formatter, "transport error: {message}"),
            Self::Api { status, message } => write!(formatter, "API error ({status}): {message}"),
            Self::Stream(message) => write!(formatter, "stream error: {message}"),
            Self::Protocol(message) => write!(formatter, "protocol error: {message}"),
        }
    }
}

impl Error for OpenAiError {}
