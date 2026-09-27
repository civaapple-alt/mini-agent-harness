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
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::time::Duration;

pub(super) const PROVIDER_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;
const CONNECT_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(250), Duration::from_millis(500)];
const MAX_TRANSPORT_ERROR_BYTES: usize = 1024;

pub struct OpenAiModel {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
    web_search: bool,
    images: ImageStore,
    max_output_tokens: Option<usize>,
    reasoning_parameter_map: BTreeMap<String, serde_json::Value>,
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
                "Responses API Base URL must not be empty".to_string(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|error| OpenAiError::Transport(transport_error_message(&error)))?;
        Ok(Self {
            client,
            api_key,
            model,
            endpoint: format!("{base_url}/responses"),
            web_search,
            images,
            max_output_tokens: None,
            reasoning_parameter_map: BTreeMap::new(),
        })
    }

    pub fn with_model_options(
        mut self,
        max_output_tokens: Option<usize>,
        reasoning_parameter_map: BTreeMap<String, serde_json::Value>,
    ) -> Self {
        self.max_output_tokens = max_output_tokens;
        self.reasoning_parameter_map = reasoning_parameter_map;
        self
    }

    pub fn set_images(&mut self, images: ImageStore) {
        self.images = images;
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
    let response = tokio::time::timeout(
        PROVIDER_IDLE_TIMEOUT,
        send_with_connect_retries(|| client.post(url).bearer_auth(api_key).json(body).send()),
    )
    .await
    .map_err(|_| OpenAiError::IdleTimeout)?
    .map_err(|(error, attempts)| {
        let mut message = transport_error_message(&error);
        if attempts > 1 {
            message.push_str(&format!(" (after {attempts} connection attempts)"));
        }
        OpenAiError::Transport(message)
    })?;
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

async fn send_with_connect_retries<F, Fut>(
    mut send: F,
) -> Result<reqwest::Response, (reqwest::Error, usize)>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut attempts = 0;
    loop {
        attempts += 1;
        match send().await {
            Ok(response) => return Ok(response),
            Err(error) if error.is_connect() && attempts <= CONNECT_RETRY_DELAYS.len() => {
                tokio::time::sleep(CONNECT_RETRY_DELAYS[attempts - 1]).await;
            }
            Err(error) => return Err((error, attempts)),
        }
    }
}

fn transport_error_message(error: &(dyn Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = Error::source(error);
    let mut depth = 0;
    while let Some(cause) = source {
        if depth == 3 {
            break;
        }
        let detail = cause.to_string();
        if !detail.is_empty() && !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = cause.source();
        depth += 1;
    }
    if message.len() > MAX_TRANSPORT_ERROR_BYTES {
        let mut end = MAX_TRANSPORT_ERROR_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    message
}

async fn drain_sse(
    response: reqwest::Response,
    max_event_bytes: usize,
    complete_on_done: bool,
    mut on_event: impl FnMut(Value) -> Result<bool, OpenAiError>,
) -> Result<bool, OpenAiError> {
    let mut stream = response.bytes_stream().eventsource();
    let mut completed_on_done = false;
    loop {
        let next = tokio::time::timeout(PROVIDER_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| OpenAiError::IdleTimeout)?;
        let Some(event) = next else {
            break;
        };
        let event =
            event.map_err(|error| OpenAiError::Transport(transport_error_message(&error)))?;
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
        if on_event(value)? {
            break;
        }
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
    loop {
        let next = tokio::time::timeout(PROVIDER_IDLE_TIMEOUT, stream.next()).await;
        let Ok(Some(chunk)) = next else {
            break;
        };
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
    IdleTimeout,
    IncompleteStream,
    Protocol(String),
}

impl fmt::Display for OpenAiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(message) => write!(formatter, "transport error: {message}"),
            Self::Api { status, message } => write!(formatter, "API error ({status}): {message}"),
            Self::Stream(message) => write!(formatter, "stream error: {message}"),
            Self::IdleTimeout => write!(formatter, "provider produced no data for 120 seconds"),
            Self::IncompleteStream => write!(formatter, "provider stream ended before completion"),
            Self::Protocol(message) => write!(formatter, "protocol error: {message}"),
        }
    }
}

impl Error for OpenAiError {}
