use super::InputQueueError;
use super::PendingInputQueue;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;

#[test]
fn steering_is_selected_before_follow_up_without_reordering_follow_ups() {
    let queue = PendingInputQueue::new(3);
    queue
        .submit(TurnInput::new(TurnInputMode::FollowUp, "queued"))
        .unwrap();
    queue
        .submit(TurnInput::new(TurnInputMode::Steer, "correct"))
        .unwrap();

    assert_eq!(queue.take_steer().unwrap().text, "correct");
    assert_eq!(queue.take_follow_up().unwrap().text, "queued");
    assert!(queue.is_empty());
}

#[test]
fn queue_rejects_start_modes_and_enforces_capacity() {
    let queue = PendingInputQueue::new(1);
    assert_eq!(
        queue.submit(TurnInput::new(TurnInputMode::Start, "new")),
        Err(InputQueueError::UnsupportedMode(TurnInputMode::Start))
    );
    queue
        .submit(TurnInput::new(TurnInputMode::FollowUp, "first"))
        .unwrap();
    assert_eq!(
        queue.submit(TurnInput::new(TurnInputMode::FollowUp, "second")),
        Err(InputQueueError::Full { capacity: 1 })
    );
}
