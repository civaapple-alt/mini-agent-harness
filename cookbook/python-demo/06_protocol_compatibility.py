"""Demo 06: Protocol Compatibility Smoke Test.

This deterministic example uses no App Server process and no model provider.
It validates that the V2 SDK parses public lifecycle events, metadata-only
replay summaries, dedicated
ThreadItem notifications, bounded ThreadItem list projections, and the
cross-repository stopping/session-fork control contract.
"""

from mini_agent import (
    SESSION_FORK_CONFLICT_CODE,
    AppServerError,
    ContextCompactionFinishedEvent,
    ContextCompactionStartedEvent,
    GenericEvent,
    ItemLifecycleNotification,
    ModelRespondedEvent,
    ModelTiming,
    RunFailedEvent,
    RunFinishedEvent,
    RuntimeStatus,
    SessionForkResult,
    ThreadItem,
    ThreadItemsListResult,
    ToolFinishedEvent,
    TurnEventsResult,
    TurnReconcileDisposition,
    parse_event,
)

EVENT_FIXTURES = [
    {"type": "turn_started", "mode": "start", "prompt": "compatibility"},
    {"type": "run_started", "prompt": "compatibility"},
    {"type": "model_started", "step": 1},
    {"type": "assistant_reasoning_delta", "delta": "thinking"},
    {"type": "assistant_text_delta", "delta": "answer"},
    {
        "type": "model_responded",
        "reasoning": "",
        "text": "answer",
        "tool_calls": [],
        "usage": None,
        "model_timing": {"ttft_ms": 24, "response_ms": 130},
    },
    {
        "type": "tool_started",
        "call": {"id": "call-1", "name": "shell", "arguments": "{}"},
    },
    {
        "type": "tool_finished",
        "call_id": "call-1",
        "name": "shell",
        "content": "ok",
        "is_error": False,
        "truncated": False,
        "outcome": "completed",
    },
    {"type": "context_compaction_started", "before_bytes": 1000},
    {
        "type": "context_compaction_finished",
        "before_bytes": 1000,
        "after_bytes": 500,
        "usage": None,
    },
    {"type": "run_finished", "stop_reason": "completed", "steps": 1},
    {"type": "turn_finished", "status": "completed"},
    {
        "type": "run_failed",
        "reason": {"type": "limit_exceeded", "detail": {"actual": 2}},
    },
    {"type": "future_extension", "value": "preserved"},
]

THREAD_ITEM_FIXTURE = {
    "type": "toolCall",
    "id": "call-compat-1",
    "name": "shell",
    "arguments": {"command": "pwd"},
    "status": "completed",
    "outcome": "completed",
    "output": "workspace",
}

ITEM_COMPLETED_FIXTURE = {
    "threadId": "thread-1",
    "turnId": "turn-1",
    "completedAtMs": 20,
    "item": THREAD_ITEM_FIXTURE,
}

RUNTIME_STATUS_FIXTURE = {
    "value": {
        "phase": "stopping",
        "threadId": "thread-1",
        "turnId": "turn-1",
        "operationId": "action-1",
        "checkpointSeq": 7,
        "stateRevision": 9,
        "timestampMs": 20,
    }
}

SESSION_FORK_FIXTURE = {
    "value": {
        "sessionId": "session-child",
        "threadId": "thread-child",
        "path": "sessions/session-child/session.jsonl",
        "parentSessionId": "session-parent",
        "parentCheckpointSeq": 7,
        "sessionBytes": 500,
        "contextBeforeBytes": 1000,
        "contextAfterBytes": 1000,
        "compacted": False,
        "method": "exact",
    }
}

SESSION_FORK_CONFLICT_FIXTURE = {
    "code": SESSION_FORK_CONFLICT_CODE,
    "message": "session fork conflicts with existing child",
    "data": {
        "kind": "contextPolicy",
        "childThreadId": "thread-child",
        "requestedContextPolicy": "compact",
        "existingContextPolicy": "exact",
    },
}

KNOWN_TOOL_OUTCOMES = (
    "completed",
    "failed",
    "needs_approval",
    "deferred",
    "retryable",
)

REPLAY_FIXTURE = {
    "value": {
        "data": [
            {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "sequence": 12,
                "eventType": "tool_finished",
                "toolCallId": "call-1",
                "toolName": "shell",
                "contextSources": [],
                "recordedAtMs": 100,
            }
        ],
        "nextCursor": 18,
        "oldestSequence": 2,
        "hasGap": False,
    }
}


def main() -> None:
    for payload in EVENT_FIXTURES:
        event = parse_event(payload)
        assert event.event_type == payload["type"]
        print(f"PASS {event.event_type}")

    assert isinstance(parse_event(EVENT_FIXTURES[8]), ContextCompactionStartedEvent)
    assert isinstance(parse_event(EVENT_FIXTURES[9]), ContextCompactionFinishedEvent)
    assert isinstance(parse_event(EVENT_FIXTURES[10]), RunFinishedEvent)
    assert isinstance(parse_event(EVENT_FIXTURES[12]), RunFailedEvent)
    assert isinstance(parse_event(EVENT_FIXTURES[13]), GenericEvent)
    response = parse_event(EVENT_FIXTURES[5])
    assert isinstance(response, ModelRespondedEvent)
    assert response.model_timing == ModelTiming(ttft_ms=24, response_ms=130)

    for outcome in KNOWN_TOOL_OUTCOMES:
        tool_event = parse_event(
            {
                "type": "tool_finished",
                "call_id": f"call-{outcome}",
                "name": "fixture",
                "content": "fixture",
                "is_error": outcome in {"failed", "retryable"},
                "truncated": False,
                "outcome": outcome,
            }
        )
        assert isinstance(tool_event, ToolFinishedEvent)
        assert tool_event.outcome == outcome

    unknown_tool_event = parse_event(
        {
            "type": "tool_finished",
            "call_id": "call-future-outcome",
            "name": "fixture",
            "content": "fixture",
            "is_error": True,
            "truncated": False,
            "outcome": "server_added_outcome",
        }
    )
    assert isinstance(unknown_tool_event, ToolFinishedEvent)
    assert unknown_tool_event.outcome == "server_added_outcome"

    item = ThreadItem.from_dict(THREAD_ITEM_FIXTURE)
    assert item.id == "call-compat-1"
    assert item.arguments == {"command": "pwd"}
    assert item.outcome == "completed"
    assert item.output == "workspace"
    item_event = ItemLifecycleNotification.from_dict(
        "item/completed", ITEM_COMPLETED_FIXTURE
    )
    assert item_event.turn_id == "turn-1"
    assert item_event.item.id == "call-compat-1"
    page = ThreadItemsListResult.from_dict(
        {"value": {"data": [{"turnId": "turn-1", "item": THREAD_ITEM_FIXTURE}]}}
    )
    assert page.data[0].item.id == "call-compat-1"
    runtime_status = RuntimeStatus.from_dict(RUNTIME_STATUS_FIXTURE)
    assert runtime_status.phase == "stopping"
    assert runtime_status.turn_id == "turn-1"
    replay = TurnEventsResult.from_dict(REPLAY_FIXTURE)
    assert replay.data[0]["eventType"] == "tool_finished"
    assert "event" not in replay.data[0]
    assert replay.next_cursor == 18
    assert TurnReconcileDisposition.COMPLETED.value == "completed"
    fork = SessionForkResult.from_dict(SESSION_FORK_FIXTURE)
    assert fork.session_id == "session-child"
    assert fork.parent_session_id == "session-parent"
    assert fork.method == "exact"
    conflict = AppServerError(**SESSION_FORK_CONFLICT_FIXTURE)
    assert conflict.code == SESSION_FORK_CONFLICT_CODE
    assert conflict.data["kind"] == "contextPolicy"
    print(
        f"Validated {len(EVENT_FIXTURES)} protocol event fixtures and "
        f"{len(KNOWN_TOOL_OUTCOMES) + 1} known/unknown tool outcomes and "
        "3 control-plane fixtures."
    )


if __name__ == "__main__":
    main()
