use super::MAX_VERIFIER_HISTORY_MESSAGES;
use super::bounded_verifier_history;
use mini_agent_protocol::Message;

#[test]
fn verifier_history_keeps_only_the_newest_bounded_window() {
    let messages = (0..(MAX_VERIFIER_HISTORY_MESSAGES + 3))
        .map(|index| Message::User {
            text: format!("message-{index}"),
        })
        .collect::<Vec<_>>();

    let bounded = bounded_verifier_history(&messages);

    assert_eq!(bounded.len(), MAX_VERIFIER_HISTORY_MESSAGES);
    assert_eq!(
        bounded.first(),
        Some(&Message::User {
            text: "message-3".to_string(),
        })
    );
    assert_eq!(
        bounded.last(),
        Some(&Message::User {
            text: "message-26".to_string(),
        })
    );
}
