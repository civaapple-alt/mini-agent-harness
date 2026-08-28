use super::Event;
use crate::ToolExecutionStatus;
use serde_json::json;

#[test]
fn tool_finished_accepts_legacy_payload_without_outcome() {
    let event: Event = serde_json::from_value(json!({
        "type": "tool_finished",
        "call_id": "call-1",
        "name": "read_file",
        "content": "ok",
        "is_error": false,
        "truncated": false
    }))
    .unwrap();

    assert!(matches!(
        event,
        Event::ToolFinished {
            outcome: None,
            is_error: false,
            ..
        }
    ));
}

#[test]
fn tool_finished_round_trips_structured_outcome() {
    let event = Event::ToolFinished {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        content: "retry later".to_string(),
        is_error: true,
        truncated: false,
        outcome: Some(ToolExecutionStatus::Retryable),
    };

    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(serde_json::from_value::<Event>(encoded).unwrap(), event);
}
