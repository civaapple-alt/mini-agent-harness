use super::Accumulator;
use super::OpenAiError;
use super::drain_sse;
use super::max_event_bytes;
use super::post_json;
use super::project_for_request;
use crate::image::ImageStore;
use crate::image::ProjectedImage;
use crate::image::vision_model_for;
use crate::image::wire_image_block;
use mini_agent_protocol::Message;
use mini_agent_protocol::ModelEvent;
use mini_agent_protocol::ModelEventSink;
use mini_agent_protocol::ModelRequest;
use mini_agent_protocol::ModelResponse;
use mini_agent_protocol::ModelUsage;
use mini_agent_protocol::ToolCall;
use serde_json::Value;
use serde_json::json;

pub async fn complete(
    model: &super::OpenAiModel,
    request: &ModelRequest<'_>,
    events: &mut (dyn ModelEventSink + Send),
) -> Result<ModelResponse, OpenAiError> {
    let body = request_body_with_limit(
        &model.model,
        request,
        model.web_search,
        &model.images,
        model.max_output_tokens,
    );
    let response = post_json(&model.client, &model.endpoint, &model.api_key, &body).await?;
    let mut state = Accumulator::new(request.max_response_bytes);
    drain_sse(
        response,
        max_event_bytes(request.max_response_bytes),
        false,
        |value| apply(&mut state, value, events),
    )
    .await?;
    if !state.completed {
        return Err(OpenAiError::Protocol(
            "stream ended before response.completed".to_string(),
        ));
    }
    Ok(state.into_response())
}

#[cfg(test)]
fn request_body(
    model: &str,
    request: &ModelRequest<'_>,
    web_search: bool,
    images: &ImageStore,
) -> Value {
    request_body_with_limit(model, request, web_search, images, None)
}

fn request_body_with_limit(
    model: &str,
    request: &ModelRequest<'_>,
    web_search: bool,
    images: &ImageStore,
    max_output_tokens: Option<usize>,
) -> Value {
    let (projected, has_live_image) = project_for_request(request, images);
    let model = vision_model_for(model, has_live_image);
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
            message_items(message, image)
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
    if let Some(max_output_tokens) = max_output_tokens {
        body["max_output_tokens"] = json!(max_output_tokens);
    }
    body
}

fn message_items(message: &Message, image: Option<&ProjectedImage>) -> Vec<Value> {
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
        } => vec![json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": match image {
                Some(image) => json!([
                    { "type": "input_text", "text": content },
                    wire_image_block(image)
                ]),
                None => json!(content)
            }
        })],
    }
}

fn apply(
    state: &mut Accumulator,
    event: Value,
    events: &mut (dyn ModelEventSink + Send),
) -> Result<bool, OpenAiError> {
    match event.get("type").and_then(Value::as_str) {
        Some("response.reasoning_text.delta") => {
            let delta = event.get("delta").and_then(Value::as_str).ok_or_else(|| {
                OpenAiError::Protocol("reasoning delta missing delta field".to_string())
            })?;
            state.retain(delta.len())?;
            state.reasoning.push_str(delta);
            events.emit(ModelEvent::ReasoningDelta(delta.to_string()));
        }
        Some("response.output_text.delta") => {
            let delta = event.get("delta").and_then(Value::as_str).ok_or_else(|| {
                OpenAiError::Protocol("text delta missing delta field".to_string())
            })?;
            state.retain(delta.len())?;
            state.text.push_str(delta);
            events.emit(ModelEvent::TextDelta(delta.to_string()));
        }
        Some("response.output_item.done") => {
            if let Some(item) = event.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
            {
                let call = parse_tool_call(item)?;
                state.retain(
                    serde_json::to_vec(&call)
                        .expect("tool call must serialize")
                        .len(),
                )?;
                state.tool_calls.push(call);
            } else if state.text.is_empty()
                && let Some(text) = event.get("item").and_then(message_text)
            {
                state.retain(text.len())?;
                state.text.push_str(&text);
                events.emit(ModelEvent::TextDelta(text));
            }
        }
        Some("response.completed") => {
            state.usage = parse_usage(&event)?;
            state.completed = true;
            return Ok(true);
        }
        Some("response.failed" | "response.incomplete") => {
            return Err(OpenAiError::Stream(event_error_message(&event)));
        }
        _ => {}
    }
    Ok(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_core::HarnessConfig;
    use mini_agent_protocol::ToolSpec;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;

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

    fn lookup_messages() -> (Vec<Message>, Vec<ToolSpec>) {
        (
            vec![
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
                    outcome: None,
                },
            ],
            vec![ToolSpec {
                name: "lookup".to_string(),
                description: "Look up a value".to_string(),
                parameters: json!({"type": "object"}),
            }],
        )
    }

    fn request<'a>(
        config: &'a HarnessConfig,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> ModelRequest<'a> {
        ModelRequest {
            system_prompt: &config.system_prompt,
            messages,
            tools,
            max_response_bytes: config.max_model_response_bytes,
        }
    }

    fn render_body(
        model: &str,
        config: &HarnessConfig,
        messages: &[Message],
        tools: &[ToolSpec],
        web_search: bool,
        images: &ImageStore,
    ) -> Value {
        request_body(model, &request(config, messages, tools), web_search, images)
    }

    #[test]
    fn serializes_responses_input_and_tools() {
        let (messages, tools) = lookup_messages();
        let config = HarnessConfig::default();
        let images = crate::image::ImageStore::memory_only();
        let body = render_body("test-model", &config, &messages, &tools, true, &images);

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][1]["role"], "user");
        assert_eq!(body["input"][2]["type"], "reasoning");
        assert_eq!(body["input"][3]["type"], "function_call");
        assert_eq!(body["input"][4]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["tools"][1]["type"], "web_search");
        assert_eq!(body["parallel_tool_calls"], false);

        let body_no_search = render_body("test-model", &config, &messages, &tools, false, &images);
        assert_eq!(body_no_search["tools"].as_array().unwrap().len(), 1);

        let empty_tools: [ToolSpec; 0] = [];
        let body_empty_tools = render_body(
            "test-model",
            &config,
            &messages,
            &empty_tools,
            true,
            &images,
        );
        assert_eq!(body_empty_tools["tools"], json!([]));
    }

    #[test]
    fn serializes_provider_output_token_budget() {
        let config = HarnessConfig::default();
        let images = crate::image::ImageStore::memory_only();
        let body = request_body_with_limit(
            "test-model",
            &request(&config, &[], &[]),
            false,
            &images,
            Some(64),
        );
        assert_eq!(body["max_output_tokens"], 64);
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
                outcome: None,
            },
        ];
        let tools = vec![ToolSpec {
            name: "read_image".to_string(),
            description: "Read an image".to_string(),
            parameters: json!({"type": "object"}),
        }];
        let config = HarnessConfig::default();
        let body = render_body(
            "deepseek-v4-flash",
            &config,
            &messages,
            &tools,
            false,
            &images,
        );
        assert_eq!(body["model"], "deepseek-v4-flash-vision-exp");
        let output = &body["input"][2]["output"];
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["file_id"], "file-api-test");
        assert!(output[1].get("image_url").is_none());
        assert!(!body.to_string().contains("data:image"));

        let compacted = render_body("deepseek-v4-flash", &config, &messages, &[], true, &images);
        assert_eq!(compacted["model"], "deepseek-v4-flash");
        assert_eq!(compacted["input"][2]["output"], envelope);
    }

    #[test]
    fn parses_text_and_function_call_events() {
        let mut state = Accumulator::new(1024);
        let mut deltas = Deltas::default();

        apply(
            &mut state,
            json!({"type": "response.reasoning_text.delta", "delta": "think"}),
            &mut deltas,
        )
        .unwrap();
        apply(
            &mut state,
            json!({"type": "response.output_text.delta", "delta": "hello"}),
            &mut deltas,
        )
        .unwrap();
        apply(
            &mut state,
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
        apply(
            &mut state,
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
        let mut state = Accumulator::new(4);
        let mut deltas = Deltas::default();

        apply(
            &mut state,
            json!({"type": "response.reasoning_text.delta", "delta": "why"}),
            &mut deltas,
        )
        .unwrap();

        let error = apply(
            &mut state,
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

    #[tokio::test]
    async fn maps_http_429_to_bounded_api_error_without_retrying() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = "rate limited fixture";
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let error = super::super::post_json(
            &client,
            &format!("http://127.0.0.1:{}/responses", address.port()),
            "test-key",
            &json!({}),
        )
        .await
        .unwrap_err();
        server.join().unwrap();

        match error {
            OpenAiError::Api { status, message } => {
                assert_eq!(status, 429);
                assert_eq!(message, "rate limited fixture");
            }
            error => panic!("unexpected provider error: {error}"),
        }
    }

    #[test]
    fn rejects_malformed_or_incomplete_function_call_events() {
        let mut state = Accumulator::new(1024);
        let mut deltas = Deltas::default();

        let malformed = apply(
            &mut state,
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "lookup",
                    "arguments": "{not-json"
                }
            }),
            &mut deltas,
        )
        .unwrap_err();
        assert!(
            malformed
                .to_string()
                .contains("invalid function call arguments")
        );

        let missing_field = apply(
            &mut state,
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": "call-2",
                    "arguments": "{}"
                }
            }),
            &mut deltas,
        )
        .unwrap_err();
        assert_eq!(
            missing_field.to_string(),
            "protocol error: function call missing name"
        );
    }
}
