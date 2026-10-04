"""Demo 07: Offline Recovery and Projection.

This deterministic example uses no App Server process and no model provider.
It exercises the SDK's public recovery reads against bounded fixtures. It keeps
project, Thread, and Turn identity stable, filters late or duplicate data, and
does not infer completion while the authoritative runtime is still stopping.
"""

from __future__ import annotations

import asyncio
from typing import Any

from mini_agent import (
    SESSION_FORK_CONFLICT_CODE,
    AppServerError,
    MiniAgentClient,
    RuntimeStatus,
    SessionForkResult,
    ThreadCheckpoint,
    ThreadItemsListResult,
    TurnEventsResult,
)

PROJECT_ID = "project-1"
THREAD_ID = "thread-1"
TURN_ID = "turn-1"
LAST_SEQUENCE = 7

RUNTIME_STATUS_FIXTURE = {
    "value": {
        "phase": "stopping",
        "threadId": THREAD_ID,
        "turnId": TURN_ID,
        "operationId": "interrupt-1",
        "checkpointSeq": 12,
        "stateRevision": 4,
        "timestampMs": 100,
    }
}

EVENTS_FIXTURE = {
    "value": {
        "data": [
            {
                "threadId": THREAD_ID,
                "turnId": TURN_ID,
                "sequence": LAST_SEQUENCE,
                "eventType": "model_responded",
                "itemId": "assistant-1",
            },
            {
                "threadId": THREAD_ID,
                "turnId": "old-turn",
                "sequence": 99,
                "eventType": "turn_finished",
            },
            {
                "threadId": THREAD_ID,
                "turnId": TURN_ID,
                "sequence": 8,
                "eventType": "tool_finished",
                "toolCallId": "call-1",
            },
            {
                "threadId": THREAD_ID,
                "turnId": TURN_ID,
                "sequence": 8,
                "eventType": "tool_finished",
                "toolCallId": "call-1",
            },
        ]
    }
}

ITEMS_FIXTURE = {
    "value": {
        "data": [
            {
                "turnId": TURN_ID,
                "item": {
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "web_fetch",
                    "status": "inProgress",
                },
            },
            {
                "turnId": TURN_ID,
                "item": {
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "web_fetch",
                    "status": "failed",
                    "outcome": "retryable",
                    "output": "transient network error",
                },
            },
        ]
    }
}

CHECKPOINT_FIXTURE = {
    "value": {
        "threadId": THREAD_ID,
        "status": "running",
        "nextTurnNumber": 3,
        "lastTurnId": TURN_ID,
        "nextEventSequence": 9,
    }
}

SESSION_FORK_FIXTURE = {
    "value": {
        "sessionId": "session-child",
        "threadId": "thread-child",
        "path": "sessions/session-child/session.jsonl",
        "parentSessionId": "session-parent",
        "parentCheckpointSeq": 12,
        "sessionBytes": 500,
        "contextBeforeBytes": 1000,
        "contextAfterBytes": 1000,
        "compacted": False,
        "method": "exact",
    }
}

FORK_CONFLICT_FIXTURE = {
    "code": SESSION_FORK_CONFLICT_CODE,
    "message": "session fork conflicts with existing child",
    "data": {
        "kind": "contextPolicy",
        "childThreadId": "thread-child",
        "requestedContextPolicy": "compact",
        "existingContextPolicy": "exact",
    },
}


def recorded_method(name: str, result: Any, calls: list[str]):
    async def call(*_args: Any, **_kwargs: Any) -> Any:
        calls.append(name)
        return result

    return call


async def reconcile_after_reconnect(
    client: MiniAgentClient,
    project_id: str,
    thread_id: str,
    turn_id: str,
    after_sequence: int,
) -> dict[str, Any]:
    """Use authoritative reads to rebuild one bounded Thread projection."""
    runtime = await client.get_runtime_status(thread_id=thread_id)
    events = await client.replay_events(
        thread_id=thread_id, after_sequence=after_sequence, limit=128
    )

    accepted_events: list[dict[str, Any]] = []
    seen_sequences: set[int] = set()
    for envelope in events.data:
        sequence = int(envelope.get("sequence", 0))
        if (
            envelope.get("threadId") != thread_id
            or envelope.get("turnId") != turn_id
            or sequence <= after_sequence
            or sequence in seen_sequences
        ):
            continue
        seen_sequences.add(sequence)
        accepted_events.append(envelope)

    items_page = await client.list_thread_items(
        thread_id=thread_id, turn_id=turn_id, limit=128
    )
    latest_items = {}
    for entry in items_page.data:
        if entry.turn_id == turn_id:
            latest_items[entry.item.id] = entry.item

    checkpoint = await client.read_thread(thread_id=thread_id)
    settled = any(
        envelope.get("eventType") == "turn_finished" for envelope in accepted_events
    )
    return {
        "identity": {
            "projectId": project_id,
            "threadId": thread_id,
            "turnId": turn_id,
        },
        "runtime": runtime,
        "events": accepted_events,
        "items": latest_items,
        "checkpoint": checkpoint,
        "settled": settled,
    }


async def main() -> None:
    client = MiniAgentClient()
    calls: list[str] = []
    client.get_runtime_status = recorded_method(
        "runtime/status", RuntimeStatus.from_dict(RUNTIME_STATUS_FIXTURE), calls
    )
    client.replay_events = recorded_method(
        "turn/events", TurnEventsResult.from_dict(EVENTS_FIXTURE), calls
    )
    client.list_thread_items = recorded_method(
        "thread/items/list", ThreadItemsListResult.from_dict(ITEMS_FIXTURE), calls
    )
    client.read_thread = recorded_method(
        "thread/read", ThreadCheckpoint.from_dict(CHECKPOINT_FIXTURE), calls
    )

    projection = await reconcile_after_reconnect(
        client, PROJECT_ID, THREAD_ID, TURN_ID, LAST_SEQUENCE
    )
    assert calls == [
        "runtime/status",
        "turn/events",
        "thread/items/list",
        "thread/read",
    ]
    assert projection["identity"] == {
        "projectId": PROJECT_ID,
        "threadId": THREAD_ID,
        "turnId": TURN_ID,
    }
    assert projection["runtime"].phase == "stopping"
    assert len(projection["events"]) == 1
    assert projection["events"][0]["sequence"] == 8
    assert len(projection["items"]) == 1
    assert projection["items"]["call-1"].status == "failed"
    assert projection["items"]["call-1"].outcome == "retryable"
    assert projection["checkpoint"].status == "running"
    assert projection["settled"] is False

    fork = SessionForkResult.from_dict(SESSION_FORK_FIXTURE)
    assert fork.session_id == "session-child"
    assert fork.thread_id == "thread-child"
    try:
        raise AppServerError(**FORK_CONFLICT_FIXTURE)
    except AppServerError as error:
        assert error.code == SESSION_FORK_CONFLICT_CODE
        assert error.data["kind"] == "contextPolicy"

    print(f"Recovery identity: {PROJECT_ID}/{THREAD_ID}/{TURN_ID}")
    print("Runtime phase: stopping")
    print("Accepted current-turn events: 1; duplicate and late data ignored")
    print("Current Turn is not settled; no turn_finished event was observed")
    print(f"Latest item outcome: {projection['items']['call-1'].outcome}")
    print(f"Session fork: {fork.session_id}; conflict: contextPolicy")


if __name__ == "__main__":
    asyncio.run(main())
