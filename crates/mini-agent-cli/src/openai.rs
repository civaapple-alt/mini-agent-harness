use crate::image::ImageStore;
use crate::image::ProjectedImage;
use crate::image::is_glm_model;
use crate::image::project_images;
use crate::image::vision_model_for;
use crate::image::wire_glm_image_block;
use crate::image::wire_image_block;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEvent;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::ModelUsage;
use mini_agent_core::ToolCall;
use reqwest::Client;
use serde_json::Value;
use serde_json::json;
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
}

impl OpenAiModel {
    pub fn new(
        api_key: String,
        model: String,
        base_url: String,
        web_search: bool,
        images: ImageStore,
    ) -> Result<Self, OpenAiError> {
        let base_url = base_url.trim().trim_end_matches('/');
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
        let body = request_body(&self.model, &request, self.web_search, &self.images);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
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

        let mut stream = response.bytes_stream().eventsource();
        let mut state = StreamState::new(request.max_response_bytes);
        let max_event_bytes = request
            .max_response_bytes
            .saturating_mul(4)
            .saturating_add(64 * 1024);
        while let Some(event) = stream.next().await {
            let event = event.map_err(|error| OpenAiError::Transport(error.to_string()))?;
            if event.data == "[DONE]" {
                break;
            }
            if event.data.len() > max_event_bytes {
                return Err(OpenAiError::Protocol(format!(
                    "SSE event exceeds {max_event_bytes} byte limit"
                )));
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|error| OpenAiError::Protocol(format!("invalid SSE JSON: {error}")))?;
            state.apply(value, events)?;
        }

        if !state.completed {
            return Err(OpenAiError::Protocol(
                "stream ended before response.completed".to_string(),
            ));
        }
        Ok(ModelResponse {
            reasoning: state.reasoning,
            text: state.text,
            tool_calls: state.tool_calls,
            usage: state.usage,
        })
    }
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

fn request_body(
    model: &str,
    request: &ModelRequest<'_>,
    web_search: bool,
    images: &ImageStore,
) -> Value {
    let attach_images = !request.tools.is_empty();
    let tool_contents = request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content.clone()),
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
    let model = vision_model_for(model, has_live_image);
    let glm_image_on_user = is_glm_model(&model);
    let mut tool_index = 0;
    let input = request
        .messages
        .iter()
        .flat_map(|message| {
            let image = if matches!(message, Message::Tool { .. }) {
                let image = projected.get(tool_index).and_then(|image| image.as_ref());
                tool_index += 1;
                image
            } else {
                None
            };
            message_items(message, image, glm_image_on_user)
        })
        .collect::<Vec<_>>();
    let mut tools = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": false
            })
        })
        .collect::<Vec<_>>();

    if web_search && !request.tools.is_empty() {
        tools.push(json!({
            "type": "web_search"
        }));
    }

    let mut body = json!({
        "model": model,
        "instructions": request.system_prompt,
        "input": input,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true
    });
    if glm_reasoning_effort(&model) {
        body["reasoning"] = json!({ "effort": "max" });
    }
    body
}

fn glm_reasoning_effort(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("glm-5.3")
}

fn message_items(
    message: &Message,
    image: Option<&ProjectedImage>,
    glm_image_on_user: bool,
) -> Vec<Value> {
    match message {
        Message::Context { text } => vec![json!({
            "type": "message",
            "role": "developer",
            "content": [{ "type": "input_text", "text": text }]
        })],
        Message::User { text } => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }]
        })],
        Message::Assistant {
            reasoning,
            text,
            tool_calls,
        } => {
            let mut items = Vec::new();
            if !reasoning.is_empty() {
                items.push(json!({
                    "type": "reasoning",
                    "content": [{ "type": "reasoning_text", "text": reasoning }]
                }));
            }
            if !text.is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text }]
                }));
            }
            items.extend(tool_calls.iter().map(|call| {
                json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.arguments.to_string()
                })
            }));
            items
        }
        Message::Tool {
            call_id, content, ..
        } => {
            if glm_image_on_user {
                glm_tool_items(call_id, content, image)
            } else {
                vec![json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": match image {
                        Some(image) => json!([
                            { "type": "input_text", "text": content },
                            wire_image_block(image)
                        ]),
                        None => json!(content)
                    }
                })]
            }
        }
    }
}

fn glm_tool_items(call_id: &str, content: &str, image: Option<&ProjectedImage>) -> Vec<Value> {
    let output = match image {
        Some(ProjectedImage::Missing(note)) => format!("{content}\n{note}"),
        _ => content.to_string(),
    };
    let mut items = vec![json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    })];
    if let Some(image @ (ProjectedImage::Inline { .. } | ProjectedImage::FileId(_))) = image {
        items.push(json!({
            "type": "message",
            "role": "user",
            "content": [
                { "type": "input_text", "text": "Image from the previous tool result." },
                wire_glm_image_block(image)
            ]
        }));
    }
    items
}

struct StreamState {
    reasoning: String,
    text: String,
    tool_calls: Vec<ToolCall>,
    usage: Option<ModelUsage>,
    completed: bool,
    retained_bytes: usize,
    max_response_bytes: usize,
}

impl StreamState {
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

    fn apply(
        &mut self,
        event: Value,
        events: &mut (dyn ModelEventSink + Send),
    ) -> Result<(), OpenAiError> {
        match event.get("type").and_then(Value::as_str) {
            Some("response.reasoning_text.delta") => {
                let delta = event.get("delta").and_then(Value::as_str).ok_or_else(|| {
                    OpenAiError::Protocol("reasoning delta missing delta field".to_string())
                })?;
                self.retain(delta.len())?;
                self.reasoning.push_str(delta);
                events.emit(ModelEvent::ReasoningDelta(delta.to_string()));
            }
            Some("response.output_text.delta") => {
                let delta = event.get("delta").and_then(Value::as_str).ok_or_else(|| {
                    OpenAiError::Protocol("text delta missing delta field".to_string())
                })?;
                self.retain(delta.len())?;
                self.text.push_str(delta);
                events.emit(ModelEvent::TextDelta(delta.to_string()));
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    let call = parse_tool_call(item)?;
                    self.retain(
                        serde_json::to_vec(&call)
                            .expect("tool call must serialize")
                            .len(),
                    )?;
                    self.tool_calls.push(call);
                } else if self.text.is_empty()
                    && let Some(text) = event.get("item").and_then(message_text)
                {
                    self.retain(text.len())?;
                    self.text.push_str(&text);
                    events.emit(ModelEvent::TextDelta(text));
                }
            }
            Some("response.completed") => {
                self.usage = parse_usage(&event)?;
                self.completed = true;
            }
            Some("response.failed" | "response.incomplete") => {
                return Err(OpenAiError::Stream(event_error_message(&event)));
            }
            _ => {}
        }
        Ok(())
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
}

fn parse_usage(event: &Value) -> Result<Option<ModelUsage>, OpenAiError> {
    let Some(usage) = event
        .get("response")
        .and_then(|response| response.get("usage"))
        .filter(|usage| !usage.is_null())
    else {
        return Ok(None);
    };
    let token_count = |name| {
        usage
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| OpenAiError::Protocol(format!("response usage missing {name}")))
    };
    let cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(Some(ModelUsage {
        input_tokens: token_count("input_tokens")?,
        cached_input_tokens,
        output_tokens: token_count("output_tokens")?,
    }))
}

fn message_text(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    Some(
        item.get("content")?
            .as_array()?
            .iter()
            .filter_map(|content| {
                (content.get("type").and_then(Value::as_str) == Some("output_text"))
                    .then(|| content.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect(),
    )
}

fn parse_tool_call(item: &Value) -> Result<ToolCall, OpenAiError> {
    let field = |name| {
        item.get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| OpenAiError::Protocol(format!("function call missing {name}")))
    };
    let arguments = field("arguments")?;
    Ok(ToolCall {
        id: field("call_id")?,
        name: field("name")?,
        arguments: serde_json::from_str(&arguments).map_err(|error| {
            OpenAiError::Protocol(format!("invalid function call arguments: {error}"))
        })?,
    })
}

fn event_error_message(event: &Value) -> String {
    event
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .pointer("/response/incomplete_details/reason")
                .and_then(Value::as_str)
        })
        .unwrap_or("response stream failed")
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_core::HarnessConfig;
    use mini_agent_core::ToolSpec;

    #[derive(Default)]
    struct Deltas {
        reasoning: Vec<String>,
        text: Vec<String>,
    }

    impl ModelEventSink for Deltas {
        fn emit(&mut self, event: ModelEvent) {
            match event {
                ModelEvent::ReasoningDelta(delta) => self.reasoning.push(delta),
                ModelEvent::TextDelta(delta) => self.text.push(delta),
            }
        }
    }

    #[test]
    fn serializes_responses_input_and_tools() {
        let messages = vec![
            Message::Context {
                text: "world state".to_string(),
            },
            Message::User {
                text: "hello".to_string(),
            },
            Message::Assistant {
                reasoning: "thinking".to_string(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "lookup".to_string(),
                    arguments: json!({"key": "value"}),
                }],
            },
            Message::Tool {
                call_id: "call-1".to_string(),
                name: "lookup".to_string(),
                content: "found".to_string(),
                is_error: false,
            },
        ];
        let tools = vec![ToolSpec {
            name: "lookup".to_string(),
            description: "Look up a value".to_string(),
            parameters: json!({"type": "object"}),
        }];
        let config = HarnessConfig::default();
        let images = crate::image::ImageStore::memory_only();
        let body = request_body(
            "test-model",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            true,
            &images,
        );

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][1]["role"], "user");
        assert_eq!(body["input"][2]["type"], "reasoning");
        assert_eq!(body["input"][3]["type"], "function_call");
        assert_eq!(body["input"][4]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["tools"][1]["type"], "web_search");
        assert_eq!(body["parallel_tool_calls"], false);

        let body_no_search = request_body(
            "test-model",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            false,
            &images,
        );
        assert_eq!(body_no_search["tools"].as_array().unwrap().len(), 1);

        let empty_tools = [];
        let body_empty_tools = request_body(
            "test-model",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &empty_tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            true,
            &images,
        );
        assert_eq!(body_empty_tools["tools"], json!([]));
    }

    #[test]
    fn projects_file_id_and_switches_deepseek_vision_model() {
        let images = crate::image::ImageStore::memory_only();
        let envelope = "<path>shot.png</path>\n<type>image</type>\n<mini_agent_image id=\"att-1\" file_id=\"file-api-test\" media_type=\"image/png\" bytes=\"8\"/>";
        let messages = vec![
            Message::User {
                text: "what is in the screenshot?".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "read_image".to_string(),
                    arguments: json!({"path": "shot.png"}),
                }],
            },
            Message::Tool {
                call_id: "call-1".to_string(),
                name: "read_image".to_string(),
                content: envelope.to_string(),
                is_error: false,
            },
        ];
        let tools = vec![ToolSpec {
            name: "read_image".to_string(),
            description: "Read an image".to_string(),
            parameters: json!({"type": "object"}),
        }];
        let config = HarnessConfig::default();
        let body = request_body(
            "deepseek-v4-flash",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            false,
            &images,
        );
        assert_eq!(body["model"], "deepseek-v4-flash-vision-exp");
        let output = &body["input"][2]["output"];
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["file_id"], "file-api-test");
        assert!(output[1].get("image_url").is_none());
        assert!(!body.to_string().contains("data:image"));

        let compacted = request_body(
            "deepseek-v4-flash",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &[],
                max_response_bytes: config.max_model_response_bytes,
            },
            true,
            &images,
        );
        assert_eq!(compacted["model"], "deepseek-v4-flash");
        assert_eq!(compacted["input"][2]["output"], envelope);
    }

    #[test]
    fn glm_image_turns_use_flash_and_inline_data_urls() {
        let images = crate::image::ImageStore::memory_only();
        let stored = images
            .save("shot.png", "image/png", crate::image::TINY_PNG.to_vec())
            .unwrap();
        let envelope = crate::image::format_envelope(&stored);
        assert!(!envelope.contains("file_id="));
        let messages = vec![
            Message::User {
                text: "what is in the screenshot?".to_string(),
            },
            Message::Assistant {
                reasoning: String::new(),
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "read_image".to_string(),
                    arguments: json!({"path": "shot.png"}),
                }],
            },
            Message::Tool {
                call_id: "call-1".to_string(),
                name: "read_image".to_string(),
                content: envelope.clone(),
                is_error: false,
            },
        ];
        let tools = vec![ToolSpec {
            name: "read_image".to_string(),
            description: "Read an image".to_string(),
            parameters: json!({"type": "object"}),
        }];
        let config = HarnessConfig::default();
        let body = request_body(
            "glm-5.3",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &messages,
                tools: &tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            false,
            &images,
        );
        assert_eq!(body["model"], "glm-5.3-flash");
        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["output"], envelope);
        let image_msg = &body["input"][3];
        assert_eq!(image_msg["type"], "message");
        assert_eq!(image_msg["role"], "user");
        let content = &image_msg["content"];
        assert_eq!(content[1]["type"], "image_url");
        assert!(
            content[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(content[1].get("file_id").is_none());
        assert!(
            !body.to_string().contains("\"type\":\"input_image\""),
            "GLM vision uses Chat Completions image_url on a user message, not Responses input_image on tool output"
        );

        let text_only = request_body(
            "glm-5.3",
            &ModelRequest {
                system_prompt: &config.system_prompt,
                messages: &[Message::User {
                    text: "hello".to_string(),
                }],
                tools: &tools,
                max_response_bytes: config.max_model_response_bytes,
            },
            false,
            &images,
        );
        assert_eq!(text_only["model"], "glm-5.3");
        assert_eq!(text_only["reasoning"]["effort"], "max");
        assert!(
            text_only
                .get("tools")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .all(|tool| tool["type"] != "web_search")
        );
    }

    #[test]
    fn parses_text_and_function_call_events() {
        let mut state = StreamState::new(1024);
        let mut deltas = Deltas::default();

        state
            .apply(
                json!({"type": "response.reasoning_text.delta", "delta": "think"}),
                &mut deltas,
            )
            .unwrap();
        state
            .apply(
                json!({"type": "response.output_text.delta", "delta": "hello"}),
                &mut deltas,
            )
            .unwrap();
        state
            .apply(
                json!({
                    "type": "response.output_item.done",
                    "item": {
                        "type": "function_call",
                        "call_id": "call-1",
                        "name": "lookup",
                        "arguments": "{\"key\":\"value\"}"
                    }
                }),
                &mut deltas,
            )
            .unwrap();
        state
            .apply(
                json!({
                    "type": "response.completed",
                    "response": {
                        "usage": {
                            "input_tokens": 37,
                            "input_tokens_details": {"cached_tokens": 11},
                            "output_tokens": 8
                        }
                    }
                }),
                &mut deltas,
            )
            .unwrap();

        assert_eq!(state.reasoning, "think");
        assert_eq!(state.text, "hello");
        assert_eq!(deltas.reasoning, vec!["think"]);
        assert_eq!(deltas.text, vec!["hello"]);
        assert_eq!(
            state.usage,
            Some(ModelUsage {
                input_tokens: 37,
                cached_input_tokens: 11,
                output_tokens: 8,
            })
        );
        assert_eq!(
            state.tool_calls,
            vec![ToolCall {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: json!({"key": "value"}),
            }]
        );
        assert!(state.completed);
    }

    #[test]
    fn rejects_stream_content_over_response_limit() {
        let mut state = StreamState::new(4);
        let mut deltas = Deltas::default();

        state
            .apply(
                json!({"type": "response.reasoning_text.delta", "delta": "why"}),
                &mut deltas,
            )
            .unwrap();

        let error = state
            .apply(
                json!({"type": "response.output_text.delta", "delta": "ok"}),
                &mut deltas,
            )
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "protocol error: model response exceeds 4 byte limit"
        );
        assert_eq!(state.reasoning, "why");
        assert!(state.text.is_empty());
        assert_eq!(deltas.reasoning, vec!["why"]);
        assert!(deltas.text.is_empty());
    }
}
