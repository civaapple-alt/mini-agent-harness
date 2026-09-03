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

#[test]
fn tool_arguments_are_bounded_and_redacted() {
    let arguments = serde_json::json!({
        "command": "echo hello",
        "token": "do-not-expose",
        "content": "private file contents",
        "note": "x".repeat(MAX_ARGUMENT_TEXT_BYTES + 20),
    });
    let event = EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        5,
        Event::ToolStarted {
            call: ToolCall {
                id: "call-1".to_string(),
                name: "shell".to_string(),
                arguments,
            },
        },
    );

    let ThreadItem::ToolCall { arguments, .. } = &ThreadItem::from_event(&event)[0] else {
        panic!("expected tool item");
    };
    assert_eq!(arguments["token"], "[REDACTED]");
    assert_eq!(arguments["content"], "[REDACTED]");
    assert!(serde_json::to_vec(arguments).unwrap().len() <= MAX_ARGUMENT_BYTES);
}

#[test]
fn completed_tool_item_keeps_the_call_projection() {
    let event = EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        6,
        Event::ToolFinished {
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "pwd"}),
            content: "ok".to_string(),
            is_error: false,
            truncated: false,
            outcome: None,
        },
    );

    assert_eq!(
        ThreadItem::from_event(&event),
        vec![ThreadItem::ToolCall {
            id: "call-1".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "pwd"}),
            status: ItemStatus::Completed,
            output: Some("ok".to_string()),
        }]
    );
}

#[test]
fn lifecycle_projection_does_not_duplicate_model_tool_calls() {
    let event = EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        7,
        Event::ModelResponded {
            reasoning: "thinking".to_string(),
            text: "answer".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({"command": "pwd"}),
            }],
            usage: None,
        },
    );

    let started = ThreadItem::started_from_event(&event);
    let completed = ThreadItem::completed_from_event(&event);
    assert_eq!(started.len(), 2);
    assert_eq!(completed.len(), 2);
    assert!(
        started
            .iter()
            .all(|item| !matches!(item, ThreadItem::ToolCall { .. }))
    );
    assert!(
        completed
            .iter()
            .all(|item| !matches!(item, ThreadItem::ToolCall { .. }))
    );
}

#[test]
fn persisted_projection_preserves_supplied_identity() {
    let message = mini_agent_protocol::Message::Assistant {
        reasoning: "thinking".to_string(),
        text: "answer".to_string(),
        tool_calls: Vec::new(),
    };
    let items = ThreadItem::from_message_with_id(&message, "message-1");
    assert_eq!(
        items,
        vec![
            ThreadItem::Reasoning {
                id: "message-1:reasoning".to_string(),
                text: "thinking".to_string(),
            },
            ThreadItem::AgentMessage {
                id: "message-1:agent".to_string(),
                text: "answer".to_string(),
            },
        ]
    );
}
