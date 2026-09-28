use super::PendingInputQueue;
use super::{InputQueueError, MAX_PENDING_INPUT_BYTES, MAX_PENDING_INPUT_ITEM_BYTES};
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

#[test]
fn queue_rejects_oversized_items_without_retaining_them() {
    let queue = PendingInputQueue::default();
    assert_eq!(
        queue.submit(TurnInput::new(
            TurnInputMode::FollowUp,
            "x".repeat(MAX_PENDING_INPUT_ITEM_BYTES + 1),
        )),
        Err(InputQueueError::ByteLimit {
            limit: MAX_PENDING_INPUT_ITEM_BYTES,
            actual: MAX_PENDING_INPUT_ITEM_BYTES + 1,
        })
    );
    let mut metadata = TurnInput::new(TurnInputMode::FollowUp, "");
    metadata
        .selected_skills
        .push("x".repeat(MAX_PENDING_INPUT_ITEM_BYTES));
    assert!(matches!(
        queue.submit(metadata),
        Err(InputQueueError::ByteLimit { .. })
    ));
    assert!(queue.is_empty());
}

#[test]
fn queue_bounds_total_retained_text_bytes() {
    let queue = PendingInputQueue::new(20);
    for _ in 0..16 {
        queue
            .submit(TurnInput::new(
                TurnInputMode::FollowUp,
                "x".repeat(MAX_PENDING_INPUT_ITEM_BYTES / 2),
            ))
            .unwrap();
    }
    assert_eq!(
        queue.submit(TurnInput::new(TurnInputMode::FollowUp, "x")),
        Err(InputQueueError::ByteLimit {
            limit: MAX_PENDING_INPUT_BYTES,
            actual: MAX_PENDING_INPUT_BYTES + 1,
        })
    );
    assert_eq!(queue.len(), 16);
}
