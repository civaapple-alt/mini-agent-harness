"""Contract tests for the version 1.0.0 Python SDK event surface."""

from __future__ import annotations

import asyncio

import pytest
from mini_agent import (
    AssistantTextDeltaEvent,
    ContextCompactionFinishedEvent,
    ContextCompactionStartedEvent,
    ContextInjectedEvent,
    GenericEvent,
    ItemLifecycleNotification,
    MiniAgentClient,
    ModelRespondedEvent,
    ModelTiming,
    ModelUsage,
    RunFailedEvent,
    RunFailure,
    RunFinishedEvent,
    ServerProcessError,
    SkillGroupActivatedEvent,
    SkillsLoadedEvent,
    SkillsLoadFailedEvent,
    StreamEventOverflowError,
    ThreadItem,
    ToolFinishedEvent,
    TurnFinishedEvent,
    TurnReadResult,
    TurnSubmissionResult,
    parse_event,
)
from mini_agent.client import (
    APP_SERVER_STDIO_LINE_LIMIT,
    STREAM_EVENT_QUEUE_BYTE_LIMIT,
    STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT,
)


@pytest.mark.parametrize(
    ("payload", "event_class"),
    [
        (
            {"type": "assistant_text_delta", "delta": "hello"},
            AssistantTextDeltaEvent,
        ),
        (
            {"type": "context_compaction_started", "before_bytes": 1200},
            ContextCompactionStartedEvent,
        ),
        (
            {
                "type": "context_compaction_finished",
                "before_bytes": 1200,
                "after_bytes": 600,
                "usage": {"input_tokens": 10, "output_tokens": 4},
            },
            ContextCompactionFinishedEvent,
        ),
        (
            {
                "type": "context_injected",
                "records": [
                    {
                        "id": "workspace_agents_main",
                        "kind": "project_instructions",
                        "source": "AGENTS.md",
                        "workspace": "main",
                        "path": "AGENTS.md",
                        "scope": "workspace",
                        "bytes": 64,
                        "fingerprint": "abc123",
                        "reused": False,
                    }
                ],
            },
            ContextInjectedEvent,
        ),
        (
            {
                "type": "model_responded",
                "usage": {
                    "input_tokens": 1200,
                    "cached_input_tokens": 850,
                    "output_tokens": 12,
                },
                "context_bytes": {
                    "systemPrompt": 100,
                    "projectInstructions": 200,
                    "skills": 50,
                    "workspaceState": 0,
                    "conversation": 300,
                    "tools": 75,
                    "other": 0,
                },
            },
            ModelRespondedEvent,
        ),
        (
            {"type": "run_finished", "stop_reason": "completed", "steps": 2},
            RunFinishedEvent,
        ),
        (
            {
                "type": "run_failed",
                "reason": {"type": "limit_exceeded", "detail": {"actual": 9}},
            },
            RunFailedEvent,
        ),
        (
            {
                "type": "skills_loaded",
                "phase": "started",
                "skills": [
                    {"name": "architect", "source": "builtin", "group": "pstack"}
                ],
            },
            SkillsLoadedEvent,
        ),
        (
            {
                "type": "skills_load_failed",
                "skills": ["architect"],
                "reason_code": "body_read_failed",
            },
            SkillsLoadFailedEvent,
        ),
        (
            {
                "type": "skill_group_activated",
                "group": "pstack",
                "source": "builtin",
            },
            SkillGroupActivatedEvent,
        ),
        (
            {
                "type": "tool_finished",
                "call_id": "call-1",
                "name": "shell",
                "content": "ok",
                "is_error": False,
                "truncated": False,
                "outcome": "completed",
            },
            ToolFinishedEvent,
        ),
        ({"type": "turn_finished", "status": "completed"}, TurnFinishedEvent),
        ({"type": "future_event", "value": 1}, GenericEvent),
    ],
)
def test_parse_event_matches_protocol_event_surface(payload, event_class):
    event = parse_event(payload)

    assert isinstance(event, event_class)
    assert event.event_type == payload["type"]
    if isinstance(event, GenericEvent):
        assert event.data == payload

    if isinstance(event, ContextCompactionFinishedEvent):
        assert event.usage == ModelUsage(
            input_tokens=10, output_tokens=4, total_tokens=14
        )
    if isinstance(event, RunFailedEvent):
        assert event.reason == RunFailure(type="limit_exceeded", detail={"actual": 9})
    if isinstance(event, SkillsLoadedEvent):
        assert event.phase == "started"
    if isinstance(event, ContextInjectedEvent):
        assert event.records[0].source == "AGENTS.md"
        assert event.records[0].bytes == 64
        assert not hasattr(event.records[0], "body")
    if isinstance(event, ModelRespondedEvent):
        assert event.usage is not None
        assert event.usage.input_tokens == 1200
        assert event.usage.cached_input_tokens == 850
        assert event.context_bytes is not None
        assert event.context_bytes.project_instructions == 200


def test_legacy_skills_loaded_event_defaults_to_loaded_phase():
    event = parse_event({"type": "skills_loaded", "skills": []})

    assert isinstance(event, SkillsLoadedEvent)
    assert event.phase == "loaded"


def test_model_responded_event_keeps_unreported_cache_usage_unknown():
    event = parse_event(
        {
            "type": "model_responded",
            "usage": {"input_tokens": 12, "output_tokens": 3},
        }
    )

    assert isinstance(event, ModelRespondedEvent)
    assert event.usage == ModelUsage(
        input_tokens=12, cached_input_tokens=None, output_tokens=3, total_tokens=15
    )
    assert event.model_timing is None


def test_model_responded_event_parses_the_model_context_snapshot():
    event = parse_event(
        {
            "type": "model_responded",
            "model_context": {
                "selection": {"provider_id": "kimi", "model_id": "k3-256k"},
                "context_window_tokens": 262144,
                "max_output_tokens": 64000,
            },
        }
    )

    assert isinstance(event, ModelRespondedEvent)
    assert event.model_context is not None
    assert event.model_context.provider_id == "kimi"
    assert event.model_context.model_id == "k3-256k"
    assert event.model_context.context_window_tokens == 262144
    assert event.model_context.max_output_tokens == 64000


def test_model_responded_event_parses_model_timing_in_wire_and_persisted_shapes():
    event = parse_event(
        {
            "type": "model_responded",
            "model_timing": {"ttft_ms": 24, "response_ms": 130},
        }
    )

    assert isinstance(event, ModelRespondedEvent)
    assert event.model_timing == ModelTiming(ttft_ms=24, response_ms=130)

    persisted_shape = ModelTiming.from_dict({"ttftMs": None, "responseMs": 130})
    assert persisted_shape == ModelTiming(ttft_ms=None, response_ms=130)


@pytest.mark.asyncio
async def test_sdk_start_turn_deduplicates_and_bounds_selected_skills():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"status": "started", "turnId": "turn-1"}

    client._send_request = fake_send
    result = await client.start_turn(
        "重构",
        selected_skills=["architect", "architect", "why"],
    )

    assert result.turn_id == "turn-1"
    assert calls == [
        (
            "turn/start",
            {
                "threadId": "default",
                "input": {
                    "mode": "start",
                    "text": "重构",
                    "selectedSkills": ["architect", "why"],
                },
            },
        )
    ]


@pytest.mark.asyncio
async def test_sdk_start_turn_preserves_workflow_and_namespaced_skills():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"status": "started", "turnId": "turn-2"}

    client._send_request = fake_send
    await client.start_turn(
        "重构",
        selected_skills=["pstack:architect"],
        workflow={"kind": "skill_group", "id": "pstack", "mode": "auto"},
    )

    assert calls[0][1]["input"]["selectedSkills"] == ["pstack:architect"]
    assert calls[0][1]["input"]["workflow"] == {
        "kind": "skill_group",
        "id": "pstack",
        "mode": "auto",
    }


@pytest.mark.asyncio
async def test_sdk_start_turn_sends_structured_child_wakeup_source():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"status": "started", "turnId": "turn-wake"}

    client._send_request = fake_send
    await client.start_turn("Continue from child updates.", turn_source="child_wakeup")

    assert calls[0][0] == "turn/start"
    assert calls[0][1]["turnSource"] == "child_wakeup"


@pytest.mark.asyncio
async def test_sdk_start_turn_sends_child_follow_up_attempt_kind():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"status": "started", "turnId": "turn-follow-up"}

    client._send_request = fake_send
    await client.start_turn(
        "Apply the review feedback.",
        operation_id="child:child-1",
        operation_attempt=2,
        operation_attempt_kind="follow_up",
    )

    assert calls[0][0] == "turn/start"
    assert calls[0][1]["operationId"] == "child:child-1"
    assert calls[0][1]["operationAttempt"] == 2
    assert calls[0][1]["operationAttemptKind"] == "follow_up"


@pytest.mark.asyncio
async def test_sdk_start_turn_rejects_unknown_child_attempt_kind():
    client = MiniAgentClient()

    with pytest.raises(ValueError, match="operation_attempt_kind"):
        await client.start_turn("bad kind", operation_attempt_kind="replay")


@pytest.mark.asyncio
async def test_sdk_steer_turn_sends_request_id_for_idempotency():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "status": "steered",
                "requestAction": "steer",
                "duplicate": False,
            },
            "actionId": "steer-action-1",
        }

    client._send_request = fake_send
    result = await client.steer_turn(
        "turn-child-1",
        "Check the latest report.",
        "child-1",
        request_id="parent-turn:tool-call-1",
    )

    assert result == {
        "value": {
            "status": "steered",
            "requestAction": "steer",
            "duplicate": False,
        },
        "actionId": "steer-action-1",
    }
    assert calls == [
        (
            "turn/steer",
            {
                "threadId": "child-1",
                "turnId": "turn-child-1",
                "text": "Check the latest report.",
                "requestId": "parent-turn:tool-call-1",
            },
        )
    ]


@pytest.mark.asyncio
async def test_stream_turn_filters_events_by_thread_and_turn():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-1")

    client.start_turn = fake_start_turn
    stream = client.stream_turn("hello", thread_id="thread-1")

    submission = await anext(stream)
    assert submission["data"]["turn_id"] == "turn-1"

    queue = client._event_queues[0]
    await queue.put(
        {
            "type": "approval",
            "approval": {
                "phase": "requested",
                "requestId": "approval-1",
                "threadId": "thread-1",
                "action": "shell command pwd",
            },
        }
    )
    await queue.put(
        {
            "type": "notification",
            "method": "item/completed",
            "data": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "completedAtMs": 20,
                "item": {
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "shell",
                    "arguments": {"command": "pwd"},
                    "status": "completed",
                    "outcome": "completed",
                    "output": "C:/workspace",
                },
            },
        }
    )
    await queue.put(
        {
            "threadId": "other-thread",
            "turnId": "other-turn",
            "sequence": 1,
            "event": {"type": "assistant_text_delta", "delta": "wrong"},
        }
    )
    await queue.put(
        {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "sequence": 2,
            "itemId": "item-2",
            "turnSource": "session_resume",
            "items": [
                {
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "shell",
                    "arguments": {"command": "pwd"},
                    "status": "inProgress",
                }
            ],
            "event": {"type": "assistant_text_delta", "delta": "right"},
        }
    )
    await queue.put(
        {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "sequence": 3,
            "event": {"type": "turn_finished", "status": "completed"},
        }
    )

    approval_event = await anext(stream)
    item_event = await anext(stream)
    text_event = await anext(stream)
    finished_event = await anext(stream)
    with pytest.raises(StopAsyncIteration):
        await anext(stream)

    assert approval_event["approval"]["phase"] == "requested"
    assert item_event["typed_item_notification"].item.id == "call-1"
    assert text_event["event"] == {"type": "assistant_text_delta", "delta": "right"}
    assert text_event["itemId"] == "item-2"
    assert text_event["turnSource"] == "session_resume"
    assert text_event["typed_items"] == [
        ThreadItem(
            type="toolCall",
            id="call-1",
            name="shell",
            arguments={"command": "pwd"},
            status="inProgress",
            outcome=None,
        )
    ]
    assert finished_event["event"] == {
        "type": "turn_finished",
        "status": "completed",
    }


@pytest.mark.asyncio
async def test_read_loop_relays_turn_events_to_notification_handler_in_order():
    received = []

    async def handler(notification):
        received.append(notification)

    class FakeStdout:
        def __init__(self):
            self._lines = iter(
                [
                    (
                        b'{"jsonrpc":"2.0","method":"turn/event","params":'
                        b'{"threadId":"thread-1","sequence":7,"event":{"type":"run_started"}}}\n'
                    )
                ]
            )

        async def readline(self):
            return next(self._lines, b"")

    client = MiniAgentClient(notification_handler=handler)
    client._proc = type("FakeProcess", (), {"stdout": FakeStdout()})()
    thread_queue = asyncio.Queue(maxsize=8)
    other_thread_queue = asyncio.Queue(maxsize=8)
    client._event_queues.extend([thread_queue, other_thread_queue])
    client._event_queue_threads[thread_queue] = "thread-1"
    client._event_queue_threads[other_thread_queue] = "thread-2"

    await client._read_loop()

    assert received == [
        {
            "type": "event",
            "threadId": "thread-1",
            "sequence": 7,
            "event": {"type": "run_started"},
        },
        {
            "type": "runtime_error",
            "threadId": "default",
            "message": "App Server connection closed before stream settlement",
        },
    ]
    thread_event = await thread_queue.get()
    client._release_stream_message(thread_queue, thread_event)
    assert thread_event == {
        "threadId": "thread-1",
        "sequence": 7,
        "event": {"type": "run_started"},
    }
    thread_error = await thread_queue.get()
    client._release_stream_message(thread_queue, thread_error)
    other_thread_error = await other_thread_queue.get()
    client._release_stream_message(other_thread_queue, other_thread_error)
    assert thread_error["type"] == "_client_error"
    assert other_thread_error["type"] == "_client_error"


@pytest.mark.asyncio
async def test_read_loop_forwards_user_question_notifications_to_thread_stream():
    received = []
    question_delivered = asyncio.Event()

    async def handler(notification):
        received.append(notification)
        if notification.get("method") == "user-question/request":
            question_delivered.set()

    class FakeStdout:
        def __init__(self):
            self._lines = iter(
                [
                    (
                        b'{"jsonrpc":"2.0","method":"user-question/request",'
                        b'"params":{"phase":"requested","interaction":'
                        b'{"interactionId":"uq-1","threadId":"thread-1",'
                        b'"turnId":"turn-1","callId":"call-1"}}}\n'
                    )
                ]
            )

        async def readline(self):
            return next(self._lines, b"")

    client = MiniAgentClient(user_questions=True, notification_handler=handler)
    client._proc = type("FakeProcess", (), {"stdout": FakeStdout()})()
    queue = asyncio.Queue(maxsize=8)
    other_thread_queue = asyncio.Queue(maxsize=8)
    client._event_queues.append(queue)
    client._event_queues.append(other_thread_queue)
    client._event_queue_threads[queue] = "thread-1"
    client._event_queue_threads[other_thread_queue] = "thread-2"

    await client._read_loop()
    await asyncio.wait_for(question_delivered.wait(), timeout=1)

    expected = {
        "type": "notification",
        "method": "user-question/request",
        "data": {
            "phase": "requested",
            "interaction": {
                "interactionId": "uq-1",
                "threadId": "thread-1",
                "turnId": "turn-1",
                "callId": "call-1",
            },
        },
    }
    assert expected in received
    streamed = await queue.get()
    client._release_stream_message(queue, streamed)
    assert streamed == expected
    other_thread_message = await other_thread_queue.get()
    client._release_stream_message(other_thread_queue, other_thread_message)
    assert other_thread_message["type"] == "_client_error"


@pytest.mark.asyncio
async def test_read_loop_settles_event_queues_when_stdout_closes():
    """EOF wakes stream consumers so a dead App Server cannot leave a Turn hung."""

    class FakeStdout:
        async def readline(self):
            return b""

    client = MiniAgentClient()
    client._proc = type("FakeProcess", (), {"stdout": FakeStdout()})()
    event_queue = asyncio.Queue()
    client._event_queues.append(event_queue)

    await client._read_loop()

    error = await event_queue.get()
    client._release_stream_message(event_queue, error)
    assert error == {
        "type": "_client_error",
        "message": "App Server connection closed before stream settlement",
    }


@pytest.mark.asyncio
async def test_read_loop_routes_item_notifications_by_thread():
    class FakeStdout:
        def __init__(self):
            self._lines = iter(
                [
                    b'{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","startedAtMs":1,"turnSource":"child_wakeup","item":{"type":"agentMessage","id":"item-1","text":"one"}}}\n',
                    b'{"jsonrpc":"2.0","method":"item/started","params":{"threadId":"thread-2","turnId":"turn-2","startedAtMs":2,"item":{"type":"agentMessage","id":"item-2","text":"two"}}}\n',
                    b'{"jsonrpc":"2.0","method":"approval/resolved","params":{"threadId":"thread-1","requestId":"approval-1","decision":"deny"}}\n',
                    b'{"jsonrpc":"2.0","method":"approval/resolved","params":{"threadId":"thread-2","requestId":"approval-2","decision":"approve"}}\n',
                ]
            )

        async def readline(self):
            return next(self._lines, b"")

    client = MiniAgentClient()
    client._proc = type("FakeProcess", (), {"stdout": FakeStdout()})()
    first = asyncio.Queue(maxsize=8)
    second = asyncio.Queue(maxsize=8)
    client._event_queues.extend([first, second])
    client._event_queue_threads[first] = "thread-1"
    client._event_queue_threads[second] = "thread-2"
    client._event_queue_bytes[first] = 0
    client._event_queue_bytes[second] = 0

    await client._read_loop()

    first_item = first.get_nowait()
    second_item = second.get_nowait()
    client._release_stream_message(first, first_item)
    client._release_stream_message(second, second_item)
    assert first_item["data"]["threadId"] == "thread-1"
    assert first_item["typed_item_notification"].turn_source == "child_wakeup"
    assert second_item["data"]["threadId"] == "thread-2"
    first_approval = first.get_nowait()
    second_approval = second.get_nowait()
    client._release_stream_message(first, first_approval)
    client._release_stream_message(second, second_approval)
    assert first_approval["approval"]["requestId"] == "approval-1"
    assert second_approval["approval"]["requestId"] == "approval-2"
    assert (await first.get())["type"] == "_client_error"
    assert (await second.get())["type"] == "_client_error"


@pytest.mark.asyncio
async def test_read_loop_settles_event_queues_when_stdout_fails():
    """A transport read error also wakes consumers instead of leaking a task."""

    class BrokenStdout:
        async def readline(self):
            raise ConnectionResetError("pipe reset")

    client = MiniAgentClient()
    client._proc = type("FakeProcess", (), {"stdout": BrokenStdout()})()
    event_queue = asyncio.Queue()
    client._event_queues.append(event_queue)

    await client._read_loop()

    failure = await event_queue.get()
    assert failure["type"] == "_client_error"
    assert "pipe reset" in failure["message"]


@pytest.mark.asyncio
async def test_read_loop_reaps_process_after_stdout_failure():
    """A failed reader must not leave a live-looking client on a dead pipe."""

    class BrokenStdout:
        async def readline(self):
            raise ConnectionResetError("pipe reset")

    class FailedProcess:
        def __init__(self):
            self.stdout = BrokenStdout()
            self.returncode = None
            self.terminated = False

        def terminate(self):
            self.terminated = True
            self.returncode = 1

        async def wait(self):
            return self.returncode

        def kill(self):
            self.returncode = -9

    client = MiniAgentClient()
    process = FailedProcess()
    client._proc = process

    await client._read_loop()

    assert process.terminated is True
    assert client.is_running is False
    assert client._proc is None


@pytest.mark.asyncio
async def test_start_sets_stdio_line_limit_for_max_thread_item_page(monkeypatch):
    """A full bounded item page must fit the SDK stdout reader."""

    class FakeStream:
        async def readline(self):
            return b""

    class FakeStdin:
        def is_closing(self):
            return False

        def close(self):
            return None

    class FakeProcess:
        def __init__(self):
            self.pid = 1234
            self.returncode = None
            self.stdin = FakeStdin()
            self.stdout = FakeStream()
            self.stderr = FakeStream()

        def terminate(self):
            self.returncode = 0

        async def wait(self):
            return self.returncode

    process = FakeProcess()
    captured = {}

    async def fake_create_subprocess(*args, **kwargs):
        captured.update(kwargs)
        return process

    monkeypatch.setattr(
        "mini_agent.client.asyncio.create_subprocess_exec",
        fake_create_subprocess,
    )

    client = MiniAgentClient()
    await client.start()
    await client.stop()

    assert captured["limit"] == APP_SERVER_STDIO_LINE_LIMIT
    assert APP_SERVER_STDIO_LINE_LIMIT == 129 * 1024 * 1024


@pytest.mark.asyncio
async def test_sdk_settings_projection_rejects_stale_revision_notifications():
    class FakeStdout:
        def __init__(self, lines):
            self._lines = iter(lines)

        async def readline(self):
            return next(self._lines, b"")

    class FakeProcess:
        stdout = FakeStdout(
            [
                (
                    b'{"jsonrpc":"2.0","method":"thread/settings/updated",'
                    b'"params":{"threadId":"thread-1","collaborationMode":'
                    b'{"mode":"plan"},"builtinTools":["shell"],'
                    b'"continuationMode":"continuous","stateRevision":5}}\n'
                ),
                (
                    b'{"jsonrpc":"2.0","method":"thread/settings/updated",'
                    b'"params":{"threadId":"thread-1","collaborationMode":'
                    b'{"mode":"default"},"builtinTools":[],'
                    b'"continuationMode":"manual","stateRevision":4}}\n'
                ),
            ]
        )

    client = MiniAgentClient()
    client._proc = FakeProcess()
    await client._read_loop()

    settings = client._thread_settings["thread-1"]
    assert settings.state_revision == 5
    assert settings.continuation_mode == "continuous"


@pytest.mark.asyncio
async def test_stream_turn_returns_when_submission_has_no_turn_id():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="queued")

    client.start_turn = fake_start_turn

    items = [item async for item in client.stream_turn("hello")]

    assert len(items) == 1
    assert items[0]["data"]["status"] == "queued"


@pytest.mark.asyncio
async def test_stream_turn_fails_when_app_server_connection_closes():
    """A dead App Server must settle the consumer instead of leaving it hung."""
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-disconnected")

    client.start_turn = fake_start_turn
    stream = client.stream_turn("inspect", thread_id="thread-1")
    await anext(stream)
    await client._event_queues[0].put(
        {
            "type": "_client_error",
            "message": "App Server connection closed before stream settlement",
        }
    )

    with pytest.raises(ServerProcessError, match="connection closed"):
        await anext(stream)


@pytest.mark.asyncio
async def test_stop_wakes_stream_consumers_before_cancelling_reader():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-stop")

    client.start_turn = fake_start_turn
    stream = client.stream_turn("inspect", thread_id="thread-1")
    await anext(stream)

    await client.stop()

    with pytest.raises(ServerProcessError, match="App Server stopped"):
        await anext(stream)


@pytest.mark.asyncio
async def test_stream_turn_bounds_buffer_and_reports_overflow_for_replay():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-overflow")

    client.start_turn = fake_start_turn
    stream = client.stream_turn("inspect", thread_id="thread-1")
    await anext(stream)
    queue = client._event_queues[0]
    assert queue.maxsize == 512

    for sequence in range(queue.maxsize + 1):
        client._enqueue_stream_message(
            queue,
            {
                "threadId": "thread-1",
                "turnId": "turn-overflow",
                "sequence": sequence,
                "event": {"type": "assistant_text_delta", "delta": "x"},
            },
        )

    with pytest.raises(StreamEventOverflowError) as err:
        await anext(stream)

    assert err.value.thread_id == "thread-1"
    assert err.value.limit == 512


def test_stream_queue_bounds_bytes_per_stream_and_client(monkeypatch):
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUE_BYTE_LIMIT", 128)
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT", 128)
    assert STREAM_EVENT_QUEUE_BYTE_LIMIT == 2 * 1024 * 1024
    assert STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT == 8 * 1024 * 1024
    client = MiniAgentClient()
    first = asyncio.Queue(maxsize=8)
    second = asyncio.Queue(maxsize=8)
    large = asyncio.Queue(maxsize=8)
    client._event_queues.extend([first, second, large])
    for queue in client._event_queues:
        client._event_queue_bytes[queue] = 0

    client._enqueue_stream_message(first, {"payload": "x" * 80})
    client._enqueue_stream_message(second, {"payload": "y" * 80})
    client._enqueue_stream_message(large, {"payload": "z" * 200})
    client._enqueue_stream_message(large, {"payload": "later"})

    assert first.qsize() == 1
    assert second.get_nowait()["type"] == "_stream_overflow"
    assert large.qsize() == 1
    assert large.get_nowait()["type"] == "_stream_overflow"
    first_message = first.get_nowait()
    client._release_stream_message(first, first_message)
    assert client._queued_event_bytes == 0


def test_stream_queue_reserves_bounded_space_for_full_model_responses(monkeypatch):
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUE_BYTE_LIMIT", 128)
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT", 128)
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_LARGE_MESSAGE_BYTE_LIMIT", 1024)
    monkeypatch.setattr(
        "mini_agent.client.STREAM_EVENT_LARGE_QUEUES_TOTAL_BYTE_LIMIT", 1024
    )
    client = MiniAgentClient()
    queue = asyncio.Queue(maxsize=8)
    client._event_queue_large_bytes[queue] = 0

    client._enqueue_stream_message(
        queue,
        {
            "threadId": "thread-1",
            "event": {"type": "model_responded", "text": "x" * 256},
        },
    )

    assert queue.qsize() == 1
    assert client._event_queue_large_bytes[queue] > 128
    assert client._queued_large_event_bytes == client._event_queue_large_bytes[queue]
    message = queue.get_nowait()
    assert message["event"]["type"] == "model_responded"
    client._release_stream_message(queue, message)
    assert client._event_queue_large_bytes[queue] == 0
    assert client._queued_large_event_bytes == 0


def test_stream_queue_rejects_large_response_beyond_reserve_without_leaking_bytes(
    monkeypatch,
):
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUE_BYTE_LIMIT", 32)
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT", 32)
    monkeypatch.setattr("mini_agent.client.STREAM_EVENT_LARGE_MESSAGE_BYTE_LIMIT", 64)
    monkeypatch.setattr(
        "mini_agent.client.STREAM_EVENT_LARGE_QUEUES_TOTAL_BYTE_LIMIT", 64
    )
    client = MiniAgentClient()
    queue = asyncio.Queue(maxsize=8)
    client._event_queue_large_bytes[queue] = 0

    client._enqueue_stream_message(
        queue,
        {"event": {"type": "model_responded", "text": "x" * 128}},
    )

    assert queue.qsize() == 1
    assert queue.get_nowait()["type"] == "_stream_overflow"
    assert client._event_queue_large_bytes[queue] == 0
    assert client._queued_large_event_bytes == 0


@pytest.mark.asyncio
async def test_stream_turn_preserves_step_limit_as_non_completed():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-limited")

    client.start_turn = fake_start_turn
    stream = client.stream_turn("inspect", thread_id="thread-1")
    await anext(stream)

    await client._event_queues[0].put(
        {
            "threadId": "thread-1",
            "turnId": "turn-limited",
            "event": {
                "type": "turn_finished",
                "status": "step_limit",
            },
        }
    )

    finished = await anext(stream)
    assert finished["event"]["status"] == "step_limit"
    with pytest.raises(StopAsyncIteration):
        await anext(stream)


@pytest.mark.asyncio
async def test_stream_turn_waits_for_settlement_after_run_failed():
    client = MiniAgentClient()

    async def fake_start_turn(prompt, mode="start", thread_id=None):
        return TurnSubmissionResult(status="started", turn_id="turn-failed")

    async def fake_read_turn(turn_id, request_timeout=None):
        return TurnReadResult(
            turn_id=turn_id,
            status="failed",
            stop_reason="failed",
            error="model request failed: transport error",
        )

    client.start_turn = fake_start_turn
    client._read_turn = fake_read_turn
    stream = client.stream_turn("inspect", thread_id="thread-1")
    await anext(stream)

    await client._event_queues[0].put(
        {
            "threadId": "thread-1",
            "turnId": "turn-failed",
            "event": {
                "type": "run_failed",
                "reason": {"type": "model"},
            },
        }
    )
    await client._event_queues[0].put(
        {
            "threadId": "thread-1",
            "turnId": "turn-failed",
            "event": {"type": "turn_finished", "status": "failed"},
        }
    )

    failed = await anext(stream)
    finished = await anext(stream)
    with pytest.raises(StopAsyncIteration):
        await anext(stream)

    assert failed["event"]["status"] == "failed"
    assert failed["event"]["error"] == "model request failed: transport error"
    assert finished["event"] == {"type": "turn_finished", "status": "failed"}


def test_thread_item_lifecycle_and_list_projection_parse_camel_case_wire_shape():
    started = ItemLifecycleNotification.from_dict(
        "item/started",
        {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "startedAtMs": 10,
            "turnSource": "child_wakeup",
            "item": {
                "type": "toolCall",
                "id": "call-1",
                "name": "shell",
                "arguments": {"command": "pwd"},
                "status": "inProgress",
            },
        },
    )
    assert started.thread_id == "thread-1"
    assert started.timestamp_ms == 10
    assert started.turn_source == "child_wakeup"
    assert started.item.status == "inProgress"


def test_thread_item_preserves_typed_tool_outcome():
    item = ThreadItem.from_dict(
        {
            "type": "toolCall",
            "id": "call-retry",
            "name": "mcp__fixture__slow",
            "arguments": {"query": "status"},
            "status": "failed",
            "outcome": "retryable",
            "output": "MCP tool call timed out",
        }
    )

    assert item.status == "failed"
    assert item.outcome == "retryable"

    unknown = ThreadItem.from_dict(
        {
            "type": "toolCall",
            "id": "call-future",
            "status": "failed",
            "outcome": "server_added_state",
        }
    )
    assert unknown.outcome == "server_added_state"

    malformed = ThreadItem.from_dict(
        {"type": "toolCall", "id": "call-malformed", "outcome": {"name": "bad"}}
    )
    assert malformed.outcome is None

    from mini_agent import ThreadItemsListResult

    page = ThreadItemsListResult.from_dict(
        {
            "value": {
                "data": [
                    {
                        "turnId": "turn-1",
                        "turnSource": "child_wakeup",
                        "item": {
                            "type": "agentMessage",
                            "id": "message-1",
                            "text": "done",
                        },
                    }
                ],
                "nextCursor": "1",
                "backwardsCursor": "0",
            }
        }
    )
    assert page.data[0].turn_id == "turn-1"
    assert page.data[0].turn_source == "child_wakeup"
    assert page.data[0].item.text == "done"
    assert page.next_cursor == "1"
    assert page.backwards_cursor == "0"


@pytest.mark.parametrize(
    "outcome",
    [
        "completed",
        "failed",
        "needs_approval",
        "deferred",
        "retryable",
        "server_added_outcome",
    ],
)
def test_tool_finished_preserves_known_and_unknown_outcomes(outcome):
    event = parse_event(
        {
            "type": "tool_finished",
            "call_id": "call-outcome",
            "name": "fixture",
            "content": "fixture",
            "is_error": True,
            "truncated": False,
            "outcome": outcome,
        }
    )

    assert isinstance(event, ToolFinishedEvent)
    assert event.outcome == outcome
