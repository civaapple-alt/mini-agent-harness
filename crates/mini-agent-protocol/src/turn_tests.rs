use super::ThreadId;
use super::ThreadStart;
use super::TurnCancel;
use super::TurnId;
use super::TurnInput;
use super::TurnInputMode;
use super::TurnStart;
use super::TurnSubmission;

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
