use super::ThreadId;
use super::ThreadStart;
use super::TurnCancel;
use super::TurnId;
use super::TurnInput;
use super::TurnInputMode;
use super::TurnStart;
use super::TurnSubmission;
use super::TurnWorkflow;
use super::TurnWorkflowKind;
use super::TurnWorkflowMode;

#[test]
fn control_contracts_round_trip_with_typed_ids() {
    let start = ThreadStart::new(ThreadId::new("thread-1"));
    let turn = TurnStart::new(TurnInput::new(TurnInputMode::Start, "inspect"));
    let cancel = TurnCancel::new(TurnId::new("turn-1"));
    let submission = TurnSubmission::Started {
        turn_id: TurnId::new("turn-1"),
    };

    assert_eq!(
        serde_json::from_str::<ThreadStart>(&serde_json::to_string(&start).unwrap()).unwrap(),
        start
    );
    assert_eq!(
        serde_json::from_str::<TurnStart>(&serde_json::to_string(&turn).unwrap()).unwrap(),
        turn
    );
    assert_eq!(
        serde_json::from_str::<TurnCancel>(&serde_json::to_string(&cancel).unwrap()).unwrap(),
        cancel
    );
    assert_eq!(
        serde_json::from_str::<TurnSubmission>(&serde_json::to_string(&submission).unwrap())
            .unwrap(),
        submission
    );
}

#[test]
fn turn_input_uses_camel_case_for_explicit_skills() {
    let mut input = TurnInput::new(TurnInputMode::Start, "inspect");
    input.selected_skills = vec!["architect".to_string()];

    let value = serde_json::to_value(&input).unwrap();
    assert_eq!(value["selectedSkills"], serde_json::json!(["architect"]));
    assert!(value.get("selected_skills").is_none());
    assert_eq!(serde_json::from_value::<TurnInput>(value).unwrap(), input);
}

#[test]
fn turn_input_round_trips_a_skill_group_workflow() {
    let mut input = TurnInput::new(TurnInputMode::Start, "refactor");
    input.workflow = Some(TurnWorkflow {
        kind: TurnWorkflowKind::SkillGroup,
        id: "pstack".to_string(),
        mode: TurnWorkflowMode::Auto,
    });

    let value = serde_json::to_value(&input).unwrap();
    assert_eq!(
        value["workflow"],
        serde_json::json!({
            "kind": "skill_group",
            "id": "pstack",
            "mode": "auto"
        })
    );
    assert_eq!(serde_json::from_value::<TurnInput>(value).unwrap(), input);
}
