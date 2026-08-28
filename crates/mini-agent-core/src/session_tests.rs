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
