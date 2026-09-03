use super::*;
use mini_agent_protocol::Event;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInputMode;

#[test]
fn trace_redacts_payloads_and_carries_round_metadata() {
    let mut bytes = Vec::new();
    let mut trace = JsonlTrace::new("trace-1", &mut bytes).unwrap();
    trace.emit(EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        1,
        Event::TurnStarted {
            mode: TurnInputMode::Start,
            prompt: "secret prompt".to_string(),
        },
    ));
    trace.emit(EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        2,
        Event::ModelStarted {
            step: 1,
            input_bytes: 321,
            input_hash: "input-hash".to_string(),
            tool_manifest_hash: "manifest-hash".to_string(),
        },
    ));
    trace.emit(EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        3,
        Event::ModelResponded {
            reasoning: String::new(),
            text: "secret answer".to_string(),
            tool_calls: vec![],
            usage: None,
        },
    ));
    let _ = trace.finish().unwrap();

    let output = String::from_utf8(bytes).unwrap();
    assert!(!output.contains("secret prompt"));
    assert!(!output.contains("secret answer"));
    let records = output
        .lines()
        .map(|line| serde_json::from_str::<TraceRecord>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].round_index, 1);
    assert_eq!(records[1].input_bytes, Some(321));
    assert_eq!(records[1].input_hash.as_deref(), Some("input-hash"));
    assert_eq!(
        records[1].tool_manifest_hash.as_deref(),
        Some("manifest-hash")
    );
    assert_eq!(records[2].round_index, 1);
    assert_eq!(
        records[2].output_bytes,
        Some("secret answer".len() + serde_json::to_vec(&Vec::<()>::new()).unwrap().len())
    );
}

#[test]
fn diagnostic_metadata_stays_out_of_the_wire_event() {
    let event = Event::ModelStarted {
        step: 1,
        input_bytes: 321,
        input_hash: "input-hash".to_string(),
        tool_manifest_hash: "manifest-hash".to_string(),
    };
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        serde_json::json!({"type": "model_started", "step": 1})
    );
}

#[test]
fn trace_rejects_unbounded_trace_ids() {
    assert!(JsonlTrace::new("", Vec::new()).is_err());
    assert!(JsonlTrace::new("x".repeat(129), Vec::new()).is_err());
}

#[test]
fn trace_records_bounded_output_without_copying_tool_arguments() {
    let mut bytes = Vec::new();
    let mut trace = JsonlTrace::new("trace-1", &mut bytes).unwrap();
    trace.emit(EventEnvelope::new(
        ThreadId::new("thread-1"),
        Some(TurnId::new("turn-1")),
        1,
        Event::ToolStarted {
            call: mini_agent_protocol::ToolCall {
                id: "call-1".to_string(),
                name: "apply_patch".to_string(),
                arguments: serde_json::json!({"patch": "secret".repeat(8 * 1024)}),
            },
        },
    ));
    let _ = trace.finish().unwrap();
    let output = String::from_utf8(bytes).unwrap();
    assert!(
        output
            .lines()
            .all(|line| line.len() <= MAX_TRACE_RECORD_BYTES)
    );
    assert!(!output.contains("secret"));
}

#[test]
fn trace_refuses_to_exceed_total_artifact_limit() {
    let mut bytes = Vec::new();
    let mut trace = JsonlTrace::new("trace-1", &mut bytes).unwrap();
    for sequence in 0..10_000 {
        trace.emit(EventEnvelope::new(
            ThreadId::new("thread-1"),
            None,
            sequence,
            Event::RunStarted {
                prompt: "bounded".to_string(),
            },
        ));
    }

    let error = trace.finish().unwrap_err();
    assert_eq!(error.to_string(), "trace artifact exceeded 256 KiB");
    assert!(bytes.len() <= MAX_TRACE_TOTAL_BYTES);
}
