use super::Accumulator;
use super::OpenAiError;
use super::drain_sse;
use super::glm_reasoning_effort;
use super::max_event_bytes;
use super::post_json;
use super::project_for_request;
use crate::image::ImageStore;
use crate::image::ProjectedImage;
use crate::image::vision_model_for;
use crate::image::wire_glm_image_block;
use mini_agent_core::Message;
use mini_agent_core::ModelEvent;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::ModelUsage;
use mini_agent_core::ToolCall;
use serde_json::Value;
use serde_json::json;

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct ChatStream {
    state: Accumulator,
    pending_tools: Vec<PendingToolCall>,
}

pub async fn complete(
    model: &super::OpenAiModel,
    endpoint: &str,
    request: &ModelRequest<'_>,
    events: &mut (dyn ModelEventSink + Send),
) -> Result<ModelResponse, OpenAiError> {
    let body = request_body(&model.model, request, &model.images);
    let response = post_json(&model.client, endpoint, &model.api_key, &body).await?;
    let mut stream = ChatStream {
        state: Accumulator::new(request.max_response_bytes),
        pending_tools: Vec::new(),
    };
    let completed_on_done = drain_sse(
        response,
        max_event_bytes(request.max_response_bytes),
        true,
        |value| stream.apply(value, events),
    )
    .await?;
    if completed_on_done {
        stream.state.completed = true;
    }
    stream.finish_tools()?;
    if !stream.state.completed {
        return Err(OpenAiError::Protocol(
            "chat stream ended without completion".to_string(),
        ));
    }
    Ok(stream.state.into_response())
}

fn request_body(model: &str, request: &ModelRequest<'_>, images: &ImageStore) -> Value {
    let (projected, has_live_image) = project_for_request(request, images);
    let model = vision_model_for(model, has_live_image);
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt
    })];
    let mut tool_index = 0;
    for message in request.messages {
        match message {
            Message::Context { text } | Message::User { text } => {
                messages.push(json!({ "role": "user", "content": text }));
            }
            Message::Assistant {
                reasoning,
                text,
                tool_calls,
            } => {
                if reasoning.is_empty() && text.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                let mut item = json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { json!(text) }
                });
                if !reasoning.is_empty() {
                    item["reasoning_content"] = json!(reasoning);
                }
                if !tool_calls.is_empty() {
                    item["tool_calls"] = json!(
                        tool_calls
                            .iter()
                            .map(|call| json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string()
                                }
                            }))
                            .collect::<Vec<_>>()
                    );
                }
                messages.push(item);
            }
            Message::Tool {
                call_id, content, ..
            } => {
                let image = projected.get(tool_index).and_then(|image| image.as_ref());
                tool_index += 1;
                let content = match image {
                    Some(ProjectedImage::Missing(note)) => format!("{content}\n{note}"),
                    _ => content.clone(),
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content
                }));
                if let Some(image @ (ProjectedImage::Inline { .. } | ProjectedImage::FileId(_))) =
                    image
                {
                    messages.push(json!({
                        "role": "user",
                        "content": [
                            { "type": "text", "text": "请描述这张图片的内容" },
                            wire_glm_image_block(image)
                        ]
                    }));
                }
            }
        }
    }
    let tools = request
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                }
            })
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "stream": true,
        "tool_stream": true
    });
    if glm_reasoning_effort(&model) {
        body["reasoning"] = json!({ "effort": "max" });
    }
    body
}

impl ChatStream {
    fn apply(
        &mut self,
        event: Value,
        events: &mut (dyn ModelEventSink + Send),
    ) -> Result<(), OpenAiError> {
        if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("chat stream failed");
            return Err(OpenAiError::Stream(message.to_string()));
        }
        if let Some(usage) = event.get("usage").filter(|usage| !usage.is_null()) {
            self.state.usage = parse_usage(usage);
        }
        let Some(choice) = event.get("choices").and_then(|choices| choices.get(0)) else {
            return Ok(());
        };
        if choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .is_some()
        {
            self.state.completed = true;
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };
        for key in ["reasoning_content", "reasoning"] {
            if let Some(delta) = delta
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                self.state.retain(delta.len())?;
                self.state.reasoning.push_str(delta);
                events.emit(ModelEvent::ReasoningDelta(delta.to_string()));
            }
        }
        if let Some(delta) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.state.retain(delta.len())?;
            self.state.text.push_str(delta);
            events.emit(ModelEvent::TextDelta(delta.to_string()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize)
                    .unwrap_or_else(|| self.pending_tools.len().saturating_sub(1));
                if self.pending_tools.is_empty() {
                    self.pending_tools.push(PendingToolCall::default());
                }
                while self.pending_tools.len() <= index {
                    self.pending_tools.push(PendingToolCall::default());
                }
                let pending = &mut self.pending_tools[index];
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    pending.id = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    pending.name.push_str(name);
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    pending.arguments.push_str(arguments);
                }
            }
        }
        Ok(())
    }

    fn finish_tools(&mut self) -> Result<(), OpenAiError> {
        let pending_tools = std::mem::take(&mut self.pending_tools);
        for pending in pending_tools {
            if pending.name.is_empty() {
                return Err(OpenAiError::Protocol(
                    "chat function call missing name".to_string(),
                ));
            }
            let arguments = if pending.arguments.is_empty() {
                json!({})
            } else {
                serde_json::from_str(&pending.arguments).map_err(|error| {
                    OpenAiError::Protocol(format!("invalid function call arguments: {error}"))
                })?
            };
            let encoded = serde_json::to_vec(&arguments).expect("tool call must serialize");
            self.state.retain(encoded.len())?;
            self.state.tool_calls.push(ToolCall {
                id: if pending.id.is_empty() {
                    format!("call-{}", self.state.tool_calls.len().saturating_add(1))
                } else {
                    pending.id
                },
                name: pending.name,
                arguments,
            });
        }
        Ok(())
    }
}

fn parse_usage(usage: &Value) -> Option<ModelUsage> {
    let input_tokens = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64)?;
    let output_tokens = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64)?;
    let cached_input_tokens = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(ModelUsage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
    })
}

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
            &images,
        );
        assert_eq!(body["model"], "glm-5.3-flash");
        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["tool_stream"], true);
        assert!(body.get("input").is_none());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read_image");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][2]["role"], "assistant");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["content"], envelope);
        let image_msg = &body["messages"][4];
        assert_eq!(image_msg["role"], "user");
        let content = &image_msg["content"];
        assert_eq!(content[0]["type"], "text");
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
            "GLM vision uses Chat Completions image_url, not Responses input_image"
        );
    }

    #[test]
    fn parses_chat_completion_deltas_and_tool_calls() {
        let mut stream = ChatStream {
            state: Accumulator::new(1024),
            pending_tools: Vec::new(),
        };
        let mut deltas = Deltas::default();
        stream
            .apply(
                json!({
                    "choices": [{
                        "delta": { "reasoning_content": "look" }
                    }]
                }),
                &mut deltas,
            )
            .unwrap();
        stream
            .apply(
                json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "call-1",
                                "type": "function",
                                "function": { "name": "read_image", "arguments": "{\"path\"" }
                            }]
                        }
                    }]
                }),
                &mut deltas,
            )
            .unwrap();
        stream
            .apply(
                json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "function": { "arguments": ":\"shot.png\"}" }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": { "prompt_tokens": 10, "completion_tokens": 4 }
                }),
                &mut deltas,
            )
            .unwrap();
        stream.finish_tools().unwrap();
        assert_eq!(stream.state.reasoning, "look");
        assert_eq!(deltas.reasoning, vec!["look"]);
        assert_eq!(
            stream.state.tool_calls,
            vec![ToolCall {
                id: "call-1".to_string(),
                name: "read_image".to_string(),
                arguments: json!({"path": "shot.png"}),
            }]
        );
        assert_eq!(
            stream.state.usage,
            Some(ModelUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 4,
            })
        );
        assert!(stream.state.completed);
    }
}
