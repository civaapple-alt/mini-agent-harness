use super::SessionState;
use mini_agent_protocol::Message;

#[test]
fn session_state_round_trips_messages_without_storage() {
    let messages = vec![Message::User {
        text: "hello".to_string(),
    }];
    let mut state = SessionState::from_messages(messages.clone());

    assert_eq!(state.messages(), messages);
    assert_eq!(state.context_revision(), 0);

    state.push(Message::Context {
        text: "world".to_string(),
    });
    assert_eq!(
        state.messages(),
        [
            Message::User {
                text: "hello".to_string()
            },
            Message::Context {
                text: "world".to_string()
            }
        ]
    );
}

#[test]
fn replacing_messages_advances_context_revision() {
    let mut state = SessionState::new();
    state.replace_messages(vec![Message::User {
        text: "restored".to_string(),
    }]);

    assert_eq!(state.context_revision(), 1);
    assert_eq!(
        state.messages(),
        [Message::User {
            text: "restored".to_string()
        }]
    );
}

#[test]
fn replacing_context_slot_removes_stale_duplicates() {
    let mut state = SessionState::from_messages(vec![
        Message::User {
            text: "hello".to_string(),
        },
        Message::Context {
            text: "<world_state><old /></world_state>".to_string(),
        },
        Message::Context {
            text: "<world_state><stale /></world_state>".to_string(),
        },
    ]);

    assert!(state.replace_context_slot(
        "world_state",
        "<world_state><new /></world_state>".to_string()
    ));
    assert_eq!(
        state.messages(),
        [
            Message::User {
                text: "hello".to_string()
            },
            Message::Context {
                text: "<world_state><new /></world_state>".to_string()
            },
        ]
    );
    let revision = state.context_revision();
    assert!(!state.replace_context_slot(
        "world_state",
        "<world_state><new /></world_state>".to_string()
    ));
    assert_eq!(state.context_revision(), revision);
}

#[test]
fn missing_context_slot_is_inserted_before_turn_history() {
    let mut state = SessionState::from_messages(vec![
        Message::User {
            text: "old turn".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: "reply".to_string(),
            tool_calls: Vec::new(),
        },
    ]);

    state.replace_context_slot(
        "session_capabilities",
        "<session_capabilities />".to_string(),
    );
    assert!(matches!(
        state.messages().first(),
        Some(Message::Context { text }) if text == "<session_capabilities />"
    ));
}

#[test]
fn repairs_an_incomplete_tool_group_before_retry() {
    let mut state = SessionState::from_messages(vec![
        Message::User {
            text: "first".to_string(),
        },
        Message::Assistant {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![mini_agent_protocol::ToolCall {
                id: "call-orphan".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({}),
            }],
        },
        Message::User {
            text: "retry".to_string(),
        },
    ]);

    assert_eq!(state.repair_incomplete_tool_groups(), 1);
    assert_eq!(
        state.messages(),
        &[
            Message::User {
                text: "first".to_string()
            },
            Message::User {
                text: "retry".to_string()
            },
        ]
    );
}

#[test]
fn retains_complete_tool_groups_during_repair() {
    let messages = vec![
        Message::Assistant {
            reasoning: String::new(),
            text: String::new(),
            tool_calls: vec![mini_agent_protocol::ToolCall {
                id: "call-complete".to_string(),
                name: "shell".to_string(),
                arguments: serde_json::json!({}),
            }],
        },
        Message::Tool {
            call_id: "call-complete".to_string(),
            name: "shell".to_string(),
            content: "done".to_string(),
            is_error: false,
            outcome: Some(mini_agent_protocol::ToolExecutionStatus::Completed),
        },
    ];
    let mut state = SessionState::from_messages(messages.clone());

    assert_eq!(state.repair_incomplete_tool_groups(), 0);
    assert_eq!(state.messages(), messages);
}
