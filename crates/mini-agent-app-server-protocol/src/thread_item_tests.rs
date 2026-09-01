use super::*;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ToolCall;
use mini_agent_protocol::TurnId;

#[test]
fn event_projection_reuses_tool_call_id() {
    let event = EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        4,
        Event::ToolStarted {
            call: ToolCall {
                id: "call-1".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
            },
        },
    );

    assert_eq!(
        ThreadItem::from_event(&event),
        vec![ThreadItem::ToolCall {
            id: "call-1".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "pwd"}),
            status: ItemStatus::InProgress,
            output: None,
        }]
    );
}

#[test]
fn settled_projection_bounds_item_text() {
    let messages = vec![mini_agent_protocol::Message::User {
        text: "x".repeat(MAX_ITEM_TEXT_BYTES + 20),
    }];

    let items = ThreadItem::from_messages(&messages);
    let ThreadItem::UserMessage { text, .. } = &items[0] else {
        panic!("expected user item");
    };
    assert_eq!(text.len(), MAX_ITEM_TEXT_BYTES);
    assert!(text.ends_with("..."));
}
