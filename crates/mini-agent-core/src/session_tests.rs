use super::SessionState;
use mini_agent_protocol::Message;
use mini_agent_protocol::{ContextInjectionKind, ContextInjectionRecord};

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
fn appending_context_slot_preserves_prior_messages_and_deduplicates() {
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

    assert!(state.append_context_slot_if_changed(
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
                text: "<world_state><old /></world_state>".to_string()
            },
            Message::Context {
                text: "<world_state><stale /></world_state>".to_string()
            },
            Message::Context {
                text: "<world_state><new /></world_state>".to_string()
            },
        ]
    );
    let revision = state.context_revision();
    assert!(!state.append_context_slot_if_changed(
        "world_state",
        "<world_state><new /></world_state>".to_string()
    ));
    assert_eq!(state.context_revision(), revision);
}

fn context_record(fingerprint: &str) -> ContextInjectionRecord {
    ContextInjectionRecord {
        id: "workspace_instruction_fixture".to_string(),
        kind: ContextInjectionKind::ProjectInstructions,
        source: "AGENTS.md".to_string(),
        workspace: Some("主工作区".to_string()),
        path: Some("AGENTS.md".to_string()),
        scope: "整个工作区及其子目录".to_string(),
        bytes: 12,
        fingerprint: fingerprint.to_string(),
        supersedes: None,
        reused: false,
    }
}

#[test]
fn injected_context_is_append_only_deduplicated_and_recoverable() {
    let mut state = SessionState::new();
    let first = context_record("first");
    let first_message = first.context_message("first instructions");
    assert_eq!(
        state.append_context_injection(first_message.clone(), first.clone()),
        Some(first.clone())
    );
    assert_eq!(state.append_context_injection(first_message, first), None);

    let updated = context_record("second");
    let updated_message = updated.context_message("updated instructions");
    let stored = state
        .append_context_injection(updated_message, context_record("second"))
        .unwrap();

    assert_eq!(stored.supersedes.as_deref(), Some("first"));
    assert_eq!(state.messages().len(), 2);
    assert!(
        matches!(&state.messages()[0], Message::Context { text } if text.contains("first instructions"))
    );
    assert!(
        matches!(&state.messages()[1], Message::Context { text } if ContextInjectionRecord::from_context_message(text).is_some_and(|record| record.supersedes.as_deref() == Some("first")))
    );

    let restored = SessionState::from_messages(state.messages().to_vec());
    assert_eq!(restored.context_injections(), vec![stored]);
}

#[test]
fn missing_context_slot_is_appended_after_turn_history() {
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

    state.append_context_slot_if_changed(
        "session_capabilities",
        "<session_capabilities />".to_string(),
    );
    assert!(matches!(
        state.messages().last(),
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
