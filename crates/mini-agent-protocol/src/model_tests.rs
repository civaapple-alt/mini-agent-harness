use super::{Message, ModelUsage};
use crate::ToolExecutionStatus;
use serde_json::json;

#[test]
fn provider_cached_usage_distinguishes_missing_from_reported_zero() {
    let missing: ModelUsage = serde_json::from_value(json!({
        "input_tokens": 12,
        "output_tokens": 3
    }))
    .unwrap();
    assert_eq!(missing.cached_input_tokens, None);

    let reported_zero: ModelUsage = serde_json::from_value(json!({
        "input_tokens": 12,
        "cached_input_tokens": 0,
        "output_tokens": 3
    }))
    .unwrap();
    assert_eq!(reported_zero.cached_input_tokens, Some(0));
}

#[test]
fn legacy_tool_message_deserializes_without_a_status() {
    let message: Message = serde_json::from_value(json!({
        "role": "tool",
        "call_id": "call-1",
        "name": "shell",
        "content": "ok",
        "is_error": false
    }))
    .unwrap();

    assert!(matches!(
        message,
        Message::Tool {
            outcome: None,
            is_error: false,
            ..
        }
    ));
}

#[test]
fn structured_tool_message_round_trips_and_omits_legacy_none() {
    let message = Message::Tool {
        call_id: "call-1".to_string(),
        name: "shell".to_string(),
        content: "approval required".to_string(),
        is_error: true,
        outcome: Some(ToolExecutionStatus::NeedsApproval),
    };
    let encoded = serde_json::to_value(&message).unwrap();
    assert_eq!(encoded["outcome"], json!("needs_approval"));
    assert_eq!(serde_json::from_value::<Message>(encoded).unwrap(), message);

    let legacy = Message::Tool {
        call_id: "call-2".to_string(),
        name: "read_file".to_string(),
        content: "ok".to_string(),
        is_error: false,
        outcome: None,
    };
    assert!(
        serde_json::to_value(legacy)
            .unwrap()
            .get("outcome")
            .is_none()
    );
}
