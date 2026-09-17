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
        arguments: serde_json::json!({"command": "pwd"}),
        content: "retry later".to_string(),
        is_error: true,
        truncated: false,
        outcome: Some(ToolExecutionStatus::Retryable),
    };

    let encoded = serde_json::to_value(&event).unwrap();
    assert!(encoded.get("arguments").is_none());
    let decoded = serde_json::from_value::<Event>(encoded).unwrap();
    assert!(matches!(decoded, Event::ToolFinished { arguments, .. } if arguments.is_null()));
}

#[test]
fn skill_activation_events_are_backward_compatible_and_namespaced() {
    let event = Event::SkillsLoaded {
        activation: Some("explicit".to_string()),
        skills: vec![super::SkillLoadRecord {
            name: "how".to_string(),
            qualified_name: Some("pstack:how".to_string()),
            source: "builtin".to_string(),
            group: Some("pstack".to_string()),
        }],
    };
    let encoded = serde_json::to_value(&event).unwrap();
    assert_eq!(encoded["activation"], "explicit");
    assert_eq!(encoded["skills"][0]["qualifiedName"], "pstack:how");
    assert!(
        serde_json::from_value::<Event>(serde_json::json!({
            "type": "skills_loaded",
            "skills": []
        }))
        .is_ok()
    );
}
