"""
Automated pytest suite for Mini Agent Python SDK advanced APIs.
"""

import os
import shutil
from pathlib import Path

import pytest
from mini_agent import (
    APP_SERVER_PROTOCOL_VERSION,
    ExecutionRecoveryInfo,
    ExecutionRecoveryPhase,
    ExecutionRecoveryRecommendation,
    ExecutionRecoveryStatus,
    MiniAgentClient,
    RuntimeStatus,
    ThreadCheckpoint,
    ThreadGoal,
    ThreadSettingsResult,
    TurnEventsResult,
    TurnReadResult,
    TurnReconcileDisposition,
)
from mini_agent.client import _redact_secrets
from mini_agent.errors import (
    AppServerError,
    ProtocolVersionMismatchError,
    ServerProcessError,
)


def has_app_server() -> bool:
    configured = os.environ.get("MINI_AGENT_APP_SERVER_PATH")
    if configured:
        return Path(configured).is_file()
    return shutil.which("mini-agent-app-server") is not None


def test_goal_result_parses_verifier_model_snapshot():
    goal = ThreadGoal.from_dict(
        {
            "thread_id": "thread-1",
            "objective": "verify",
            "status": "active",
            "verifier_model_selection": {
                "provider_id": "kimi",
                "model_id": "kimi-k2",
            },
        }
    )

    assert goal.verifier_model_selection == {
        "provider_id": "kimi",
        "model_id": "kimi-k2",
    }


def test_python_sdk_targets_app_server_protocol_v2():
    assert APP_SERVER_PROTOCOL_VERSION == 2


@pytest.mark.asyncio
async def test_sdk_model_catalog_request_and_thread_model_settings():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "collaborationMode": {"mode": "default"},
                "builtinTools": [],
                "continuationMode": "manual",
                "modelSelection": {"providerId": "deepseek", "modelId": "deepseek-v4"},
                "reasoningSelection": {"kind": "level", "value": "disabled"},
                "reasoningEffort": "high",
            }
        }

    client._send_request = fake_send
    await client.manage_model_catalog(
        "upsert_provider",
        provider={
            "id": "deepseek",
            "name": "DeepSeek",
            "kind": "deepseek",
            "baseUrl": "",
        },
        apiKey="private-key",
    )
    await client.test_model_connection("deepseek", "deepseek-v4")
    settings = await client.update_thread_settings(
        "default",
        thread_id="thread-1",
        model_selection={"providerId": "deepseek", "modelId": "deepseek-v4"},
        reasoning_effort="high",
    )
    typed_settings = await client.update_thread_model_settings(
        thread_id="thread-1",
        reasoning_selection={"kind": "level", "value": "disabled"},
    )
    thread_settings = await client.get_thread_model_settings("thread-1")

    assert calls[0] == (
        "model/catalog/manage",
        {
            "operation": "upsert_provider",
            "provider": {
                "id": "deepseek",
                "name": "DeepSeek",
                "kind": "deepseek",
                "baseUrl": "",
            },
            "apiKey": "private-key",
        },
    )
    assert calls[1] == (
        "model/catalog/manage",
        {
            "operation": "test_connection",
            "providerId": "deepseek",
            "modelId": "deepseek-v4",
        },
    )
    assert calls[2][1]["modelSelection"] == {
        "providerId": "deepseek",
        "modelId": "deepseek-v4",
    }
    assert settings.model_selection == {
        "providerId": "deepseek",
        "modelId": "deepseek-v4",
    }
    assert settings.reasoning_effort == "high"
    assert typed_settings.reasoning_selection == {"kind": "level", "value": "disabled"}
    assert calls[3] == (
        "thread/settings/update",
        {
            "threadId": "thread-1",
            "reasoningSelection": {"kind": "level", "value": "disabled"},
        },
    )
    assert calls[4] == (
        "thread/model-settings/get",
        {"threadId": "thread-1"},
    )
    assert thread_settings["value"]["modelSelection"]["providerId"] == "deepseek"
    assert thread_settings["value"]["reasoningSelection"] == {
        "kind": "level",
        "value": "disabled",
    }
    assert _redact_secrets({"provider": {"api_key": "private-key"}}) == {
        "provider": {"api_key": "[REDACTED]"}
    }


@pytest.mark.asyncio
async def test_sdk_list_skills_unwraps_action_result_and_uses_thread_id():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "skills": [
                    {"name": "installed", "qualifiedName": "installed", "enabled": True}
                ]
            },
            "actionId": "action-1",
        }

    client._send_request = fake_send
    skills = await client.list_skills("thread-42")

    assert skills == [
        {"name": "installed", "qualifiedName": "installed", "enabled": True}
    ]
    assert calls == [("skills/list", {"threadId": "thread-42"})]


@pytest.mark.asyncio
async def test_sdk_child_task_action_unwraps_action_result():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "threadId": "child-1",
                "parentThreadId": "parent-1",
                "operationId": "child:child-1",
                "action": "report",
                "status": "reported",
                "cursor": 27,
                "attempt": 2,
                "timestampMs": 1_700_000_000_000,
            },
            "actionId": "action-27",
            "actionSequence": 27,
        }

    client._send_request = fake_send

    result = await client.child_task_action(
        "child-1",
        "parent-1",
        "child:child-1",
        2,
        "report",
        report="Finished the audit pass.",
        report_id="call-27",
        request_id="parent-turn:control-1",
    )

    assert result["cursor"] == 27
    assert result["attempt"] == 2
    assert calls == [
        (
            "child/task",
            {
                "threadId": "child-1",
                "parentThreadId": "parent-1",
                "operationId": "child:child-1",
                "attempt": 2,
                "action": "report",
                "report": "Finished the audit pass.",
                "reportId": "call-27",
                "requestId": "parent-turn:control-1",
            },
        )
    ]

    calls.clear()
    await client.child_task_action(
        "child-1",
        "parent-1",
        "child:child-1",
        2,
        "pause",
        request_id="parent-turn:pause-1",
        turn_id="child-turn-2",
    )
    assert calls[0][1]["turnId"] == "child-turn-2"
    assert calls[0][1]["requestId"] == "parent-turn:pause-1"

    calls.clear()
    await client.child_task_action(
        "child-1",
        "parent-1",
        "child:child-1",
        2,
        "resume",
        request_id="parent-turn:resume-1",
        turn_id="child-turn-2",
    )
    assert calls[0][1]["turnId"] == "child-turn-2"
    assert calls[0][1]["action"] == "resume"

    calls.clear()
    await client.child_task_action(
        "child-1",
        "parent-1",
        "child:child-1",
        2,
        "start_failure",
        request_id="parent-turn:start-failure-1",
        error="App Server rejected turn/start",
    )
    assert calls[0][1]["error"] == "App Server rejected turn/start"


@pytest.mark.asyncio
async def test_sdk_resumes_the_matching_execution_checkpoint():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"value": {"turnId": "turn-resume", "status": "started"}}

    client._send_request = fake_send
    result = await client.resume_turn(
        "turn-resume",
        14,
        "resume-request-14",
        thread_id="child-1",
    )

    assert result.turn_id == "turn-resume"
    assert result.status == "started"
    assert calls == [
        (
            "turn/resume",
            {
                "threadId": "child-1",
                "turnId": "turn-resume",
                "checkpointSeq": 14,
                "requestId": "resume-request-14",
            },
        )
    ]


def test_sdk_reads_bounded_execution_recovery_metadata():
    recovery = {
        "turnId": "turn-recovery",
        "status": "waiting_for_continue",
        "phase": "model_request",
        "checkpointSeq": 14,
        "lastHeartbeatMs": 1_700_000_000_100,
        "lastProgressMs": 1_700_000_000_000,
        "reason": "temporary_model_error",
    }

    checkpoint = ThreadCheckpoint.from_dict(
        {
            "threadId": "child-1",
            "status": "idle",
            "executionRecovery": recovery,
            "pendingUserQuestion": {
                "interactionId": "uq-1",
                "currentIndex": 0,
                "questions": [{"id": "q1", "prompt": "Choose", "options": []}],
            },
        }
    )
    turn = TurnReadResult.from_dict(
        {"turnId": "turn-recovery", "status": "failed", "recovery": recovery}
    )

    assert checkpoint.execution_recovery.status is (
        ExecutionRecoveryStatus.WAITING_FOR_CONTINUE
    )
    assert checkpoint.execution_recovery.phase is ExecutionRecoveryPhase.MODEL_REQUEST
    assert checkpoint.execution_recovery.checkpoint_seq == 14
    assert checkpoint.execution_recovery.last_heartbeat_ms == 1_700_000_000_100
    assert checkpoint.execution_recovery.last_progress_ms == 1_700_000_000_000
    assert checkpoint.execution_recovery.reason == "temporary_model_error"
    assert checkpoint.pending_user_question["interactionId"] == "uq-1"
    assert checkpoint.execution_recovery.to_dict() == recovery
    assert turn.recovery.recommended_action is ExecutionRecoveryRecommendation.RESUME
    assert turn.recovery.to_dict() == recovery


def test_sdk_preserves_unknown_recovery_values_and_recommends_inspection():
    recovery = {
        "turnId": "turn-future",
        "status": "future_recovery_state",
        "phase": "future_phase",
        "checkpointSeq": 21,
        "futureField": {"bounded": True},
    }

    result = TurnReadResult.from_dict(
        {"turnId": "turn-future", "status": "in_progress", "recovery": recovery}
    )

    assert result.recovery.status is ExecutionRecoveryStatus.UNKNOWN
    assert result.recovery.phase is ExecutionRecoveryPhase.UNKNOWN
    assert result.recovery.recommended_action is ExecutionRecoveryRecommendation.INSPECT
    assert result.recovery.to_dict() == recovery


@pytest.mark.asyncio
async def test_sdk_thread_checkpoint_round_trips_from_read_to_resume():
    client = MiniAgentClient()
    calls = []
    checkpoint_value = {
        "threadId": "thread-source",
        "status": "idle",
        "messages": [],
        "contextRevision": 3,
        "nextTurnNumber": 2,
        "lastTurnId": "turn-1",
        "nextEventSequence": 5,
    }

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "thread/read":
            return {
                "value": checkpoint_value,
                "actionId": "read-action",
                "actionSequence": 7,
                "stateRevision": 8,
            }
        if method == "thread/resume":
            return {"value": {"threadId": "thread-restored"}}
        raise AssertionError(f"unexpected method: {method}")

    client._send_request = fake_send

    checkpoint = await client.read_thread("thread-source")
    await client.resume_thread("thread-restored", checkpoint)

    assert calls == [
        ("thread/read", {"threadId": "thread-source"}),
        (
            "thread/resume",
            {"threadId": "thread-restored", "checkpoint": checkpoint_value},
        ),
    ]


@pytest.mark.asyncio
async def test_start_thread_without_id_attaches_to_server_selected_thread():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"threadId": "session-thread"}

    client._send_request = fake_send

    thread_id = await client.start_thread()

    assert thread_id == "session-thread"
    assert calls == [("thread/start", {})]


@pytest.mark.asyncio
async def test_restart_reuses_provider_and_resumes_the_same_session():
    client = MiniAgentClient(env={"MINI_AGENT_SESSION_MODE": "new"})
    calls = []
    provider_selection = {"model": "selected-model-provider"}

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "initialize":
            return {"protocolVersion": 2, "capabilityManifest": {}}
        if method == "session/info":
            return {
                "value": {
                    "sessionId": "session-1",
                    "threadId": "session-thread",
                    "path": "/tmp/session.jsonl",
                    "resumed": True,
                }
            }
        if method == "thread/start":
            return {"threadId": params["threadId"]}
        if method == "world/set_execution":
            return {"value": {"changed": False}}
        raise AssertionError(f"unexpected method: {method}")

    async def fake_stop():
        return None

    async def fake_start():
        assert client.env["MINI_AGENT_SESSION_MODE"] == "resume"
        assert client.env["MINI_AGENT_SESSION_ID"] == "session-1"

    client._send_request = fake_send
    client.stop = fake_stop
    client.start = fake_start

    await client.initialize(
        client_name="custom-client",
        client_version="1.2.3",
        providers=provider_selection,
    )
    await client.set_world_execution(access="full_machine", policy="automatic")
    client._thread_settings["session-thread"] = ThreadSettingsResult.from_dict(
        {
            "collaborationMode": {"mode": "plan"},
            "builtinTools": ["read_file"],
            "continuationMode": "manual",
            "stateRevision": 9,
        }
    )

    await client.restart()

    initialize_calls = [params for method, params in calls if method == "initialize"]
    assert initialize_calls == [
        {
            "protocolVersion": 2,
            "clientName": "custom-client",
            "clientVersion": "1.2.3",
            "capabilities": {"userQuestions": False},
            "providers": provider_selection,
        },
        {
            "protocolVersion": 2,
            "clientName": "custom-client",
            "clientVersion": "1.2.3",
            "capabilities": {"userQuestions": False},
            "providers": provider_selection,
        },
    ]
    assert client._active_thread_id == "session-thread"
    assert client._thread_settings["session-thread"].state_revision is None
    assert calls[-1] == ("thread/start", {"threadId": "session-thread"})
    assert [params for method, params in calls if method == "world/set_execution"] == [
        {"access": "full_machine", "policy": "automatic"},
        {"access": "full_machine", "policy": "automatic"},
    ]


@pytest.mark.skipif(
    not has_app_server(),
    reason="Live SDK test requires mini-agent-app-server binary (set MINI_AGENT_APP_SERVER_PATH)",
)
@pytest.mark.asyncio
async def test_restart_resumes_durable_session_with_app_server(tmp_path: Path):
    client = MiniAgentClient(
        cwd=str(tmp_path),
        env={
            "MINI_AGENT_SESSION_MODE": "new",
            "MINI_AGENT_THREAD_ID": "restart-thread",
        },
    )
    await client.start()
    try:
        await client.initialize()
        before = await client.get_session_info()
        assert before is not None

        await client.restart()

        after = await client.get_session_info()
        assert after is not None
        assert after.session_id == before.session_id
        assert after.thread_id == before.thread_id
        assert after.resumed is True
    finally:
        await client.stop()


@pytest.mark.asyncio
async def test_initialize_tolerates_server_without_session_info():
    client = MiniAgentClient()

    async def fake_send(method, params=None):
        if method == "initialize":
            return {"protocolVersion": 2, "capabilityManifest": {}}
        if method == "session/info":
            raise AppServerError(-32601, "method not found")
        raise AssertionError(f"unexpected method: {method}")

    client._send_request = fake_send

    result = await client.initialize()

    assert result["protocolVersion"] == 2
    assert client._session_info is None


@pytest.mark.asyncio
async def test_initialize_rejects_protocol_v1():
    client = MiniAgentClient()

    async def fake_send(method, params=None):
        assert method == "initialize"
        return {"protocolVersion": 1, "capabilityManifest": {}}

    client._send_request = fake_send

    with pytest.raises(
        ProtocolVersionMismatchError, match="Unsupported protocol version 1"
    ):
        await client.initialize()


@pytest.mark.asyncio
async def test_reconcile_turn_sends_bounded_operator_decision():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "turnId": "turn-1",
                "checkpointSeq": 12,
                "status": "applied",
            }
        }

    client._send_request = fake_send
    result = await client.reconcile_turn(
        "turn-1",
        12,
        "call-1",
        "request-1",
        TurnReconcileDisposition.COMPLETED,
        "verified against the receiver",
        result_status="completed",
        result_content="receipt 123",
        thread_id="thread-1",
    )

    assert result.status == "applied"
    assert calls == [
        (
            "turn/reconcile",
            {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "checkpointSeq": 12,
                "toolCallId": "call-1",
                "requestId": "request-1",
                "disposition": "completed",
                "evidenceSummary": "verified against the receiver",
                "result": {"status": "completed", "content": "receipt 123"},
            },
        )
    ]


@pytest.mark.asyncio
async def test_read_context_manifest_returns_session_owned_provenance():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "data": [
                    {
                        "threadId": "thread-1",
                        "turnId": "turn-1",
                        "sourceId": "project-instructions",
                        "sourceName": "AGENTS.md",
                        "kind": "project_instructions",
                        "versionFingerprint": "sha256:abc",
                        "appliesTo": "workspace",
                        "permissionBasis": "workspace instruction policy",
                        "injectionReason": "include applicable project instructions",
                        "bytes": 128,
                        "reused": False,
                        "injectedAtMs": 100,
                    }
                ]
            }
        }

    client._send_request = fake_send
    manifest = await client.read_context_manifest("thread-1")

    assert len(manifest.data) == 1
    assert manifest.data[0].source_id == "project-instructions"
    assert manifest.data[0].permission_basis == "workspace instruction policy"
    assert calls == [
        ("session/context_manifest", {"threadId": "thread-1"}),
    ]


def test_execution_recovery_decodes_uncertain_tool_identities_without_arguments():
    recovery = ExecutionRecoveryInfo.from_dict(
        {
            "turnId": "turn-1",
            "status": "needs_reconciliation",
            "phase": "tool_batch",
            "checkpointSeq": 9,
            "uncertainToolCalls": [{"toolCallId": "call-1", "name": "send_message"}],
        }
    )

    assert [
        (call.tool_call_id, call.name) for call in recovery.uncertain_tool_calls
    ] == [("call-1", "send_message")]
    assert "arguments" not in recovery.to_dict()["uncertainToolCalls"][0]


@pytest.mark.asyncio
async def test_web_client_can_negotiate_user_question_capability():
    client = MiniAgentClient(user_questions=True)
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "initialize":
            return {"protocolVersion": 2, "capabilityManifest": {}}
        if method == "session/info":
            raise AppServerError(-32601, "method not found")
        return {"accepted": True}

    client._send_request = fake_send
    await client.initialize()
    result = await client.respond_user_question(
        interaction_id="uq-1",
        thread_id="thread-1",
        turn_id="turn-1",
        call_id="call-1",
        question_id="q1",
        answer={"type": "option", "optionId": "q1-o1"},
    )

    assert calls[0][1]["capabilities"] == {"userQuestions": True}
    assert calls[-1] == (
        "user-question/respond",
        {
            "interactionId": "uq-1",
            "threadId": "thread-1",
            "turnId": "turn-1",
            "callId": "call-1",
            "questionId": "q1",
            "answer": {"type": "option", "optionId": "q1-o1"},
        },
    )
    assert result["accepted"] is True


@pytest.mark.asyncio
async def test_restart_keeps_disabled_session_mode_without_durable_session():
    client = MiniAgentClient(env={"MINI_AGENT_SESSION_MODE": "disabled"})
    client._active_thread_id = "thread-1"
    client._thread_settings["thread-1"] = ThreadSettingsResult.from_dict(
        {"collaborationMode": {"mode": "plan"}, "stateRevision": 8}
    )

    async def fake_send(method, params=None):
        if method == "initialize":
            return {"protocolVersion": 2, "capabilityManifest": {}}
        if method == "session/info":
            return {"value": None}
        if method == "thread/start":
            return {"threadId": params["threadId"]}
        raise AssertionError(f"unexpected method: {method}")

    async def fake_stop():
        return None

    async def fake_start():
        return None

    client._send_request = fake_send
    client.stop = fake_stop
    client.start = fake_start

    await client.restart()

    assert client.env["MINI_AGENT_SESSION_MODE"] == "disabled"
    assert "MINI_AGENT_SESSION_ID" not in client.env
    assert client._thread_settings == {}


@pytest.mark.parametrize(
    ("status", "recommendation"),
    [
        ("running", ExecutionRecoveryRecommendation.WAIT),
        (
            "needs_reconciliation",
            ExecutionRecoveryRecommendation.VERIFY_TOOL_RESULT,
        ),
        ("settled", ExecutionRecoveryRecommendation.NONE),
    ],
)
def test_sdk_recovery_recommendations_are_informational(status, recommendation):
    recovery = TurnReadResult.from_dict(
        {
            "turnId": "turn-contract",
            "status": "in_progress",
            "recovery": {
                "turnId": "turn-contract",
                "status": status,
                "phase": "tool_batch",
                "checkpointSeq": 1,
            },
        }
    ).recovery

    assert recovery.recommended_action is recommendation


@pytest.mark.asyncio
async def test_sdk_exposes_event_gap_before_snapshot_reconciliation():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "turn/events":
            return {
                "value": {
                    "data": [],
                    "nextCursor": 8,
                    "oldestSequence": 5,
                    "hasGap": True,
                }
            }
        if method == "thread/read":
            return {
                "value": {
                    "threadId": "thread-gap",
                    "status": "idle",
                    "executionRecovery": None,
                }
            }
        if method == "thread/items/list":
            return {"value": {"data": [], "nextCursor": None}}
        raise AssertionError(f"unexpected method: {method}")

    client._send_request = fake_send

    replay = await client.replay_events("thread-gap", after_sequence=2)
    assert replay.has_gap is True
    checkpoint = await client.read_thread("thread-gap")
    items = await client.list_thread_items("thread-gap")

    assert checkpoint.thread_id == "thread-gap"
    assert items.data == []
    assert [method for method, _ in calls] == [
        "turn/events",
        "thread/read",
        "thread/items/list",
    ]


@pytest.mark.asyncio
async def test_sdk_wait_for_turn_keeps_polling_while_execution_is_recoverable():
    client = MiniAgentClient()
    reads = []
    results = [
        TurnReadResult.from_dict(
            {
                "turnId": "turn-recovery",
                "status": "in_progress",
                "recovery": {
                    "status": "running",
                    "phase": "model_request",
                    "checkpointSeq": 14,
                },
            }
        ),
        TurnReadResult.from_dict(
            {"turnId": "turn-recovery", "status": "completed", "steps": 3}
        ),
    ]

    async def fake_read_turn(turn_id, request_timeout=None):
        reads.append((turn_id, request_timeout))
        return results.pop(0)

    client._read_turn = fake_read_turn

    result = await client.wait_for_turn("turn-recovery", timeout=1, poll_interval=0)

    assert result.status == "completed"
    assert len(reads) == 2


@pytest.mark.asyncio
async def test_search_notebook_filters_the_bounded_read_projection_locally():
    client = MiniAgentClient()
    calls = []
    notebook = {
        "sessionId": "session-1",
        "entries": [
            {
                "key": "architecture.runtime",
                "content": "Thread recovery",
                "keywords": ["checkpoint"],
                "evidence": [{"project": "mini-agent", "subject": "Session flow"}],
            },
            {
                "key": "unrelated",
                "content": "No match",
                "keywords": [],
                "evidence": [],
            },
        ],
    }

    async def fake_read_notebook(thread_id=None, scope="self"):
        calls.append((thread_id, scope))
        return notebook

    async def unsupported_rpc(*args, **kwargs):
        raise AssertionError("Notebook search must not send a protocol RPC")

    client.read_notebook = fake_read_notebook
    client._send_request = unsupported_rpc

    result = await client.search_notebook(
        " SESSION ", scope="parent", limit=1, thread_id="thread-1"
    )

    assert calls == [("thread-1", "parent")]
    assert result == {**notebook, "entries": notebook["entries"][:1]}


@pytest.mark.asyncio
async def test_sdk_wait_for_turn_propagates_terminal_json_rpc_errors():
    client = MiniAgentClient()

    async def missing_turn(turn_id, request_timeout=None):
        raise AppServerError(-32000, f"Turn not found: {turn_id}")

    client._read_turn = missing_turn

    with pytest.raises(AppServerError, match="Turn not found"):
        await client.wait_for_turn("missing-turn", timeout=5, poll_interval=0)


@pytest.mark.asyncio
async def test_start_turn_sends_effort_inside_turn_input():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"value": {"turnId": "turn-1", "status": "started"}}

    client._send_request = fake_send

    await client.start_turn("inspect", effort="high")

    assert calls == [
        (
            "turn/start",
            {
                "threadId": "default",
                "input": {
                    "mode": "start",
                    "text": "inspect",
                    "reasoningEffort": "high",
                },
            },
        )
    ]


@pytest.mark.asyncio
async def test_start_turn_rejects_modes_not_supported_by_turn_start():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {}

    client._send_request = fake_send

    with pytest.raises(ValueError, match="start or start_if_idle"):
        await client.start_turn("inspect", mode="continue")

    assert calls == []


@pytest.mark.asyncio
async def test_sdk_session_control_uses_the_durable_session_control_method():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "threadId": "parent-1",
                "sessionId": "session-1",
                "status": "frozen",
                "requestId": "freeze-1",
                "updatedAtMs": 1_700_000_000_000,
            },
            "actionId": 8,
        }

    client._send_request = fake_send

    result = await client.session_control(
        "freeze", thread_id="parent-1", request_id="freeze-1"
    )

    assert result["status"] == "frozen"
    assert calls == [
        (
            "session/control",
            {
                "threadId": "parent-1",
                "action": "freeze",
                "requestId": "freeze-1",
            },
        )
    ]


@pytest.mark.skipif(
    not has_app_server(),
    reason="Live SDK test requires mini-agent-app-server binary (set MINI_AGENT_APP_SERVER_PATH)",
)
@pytest.mark.asyncio
async def test_advanced_thread_and_workflow_apis(tmp_path: Path):
    # Workflow files belong to the App Server SessionStore. Keep this live SDK
    # test from exercising the disabled-session cwd fallback and polluting the
    # repository under test.
    async with MiniAgentClient(
        cwd=str(tmp_path),
        env={
            "MINI_AGENT_SESSION_MODE": "new",
            "MINI_AGENT_THREAD_ID": "default",
        },
        log_dir="logs",
    ) as client:
        # 1. Initialize
        init_res = await client.initialize()
        assert init_res.get("protocolVersion") == 2
        assert init_res.get("serverVersion") == "1.0.0"

        # 2. Thread lifecycle
        tid1 = await client.start_thread()
        assert tid1 == "default"

        # List threads
        thread_list = await client.list_threads()
        assert "default" in thread_list.data

        wf_state = await client.get_workflow_state(thread_id="default")
        assert wf_state.collaboration_mode.mode in ("default", "plan")
        assert hasattr(wf_state, "builtin_tools")
        assert wf_state.builtin_tools == [
            "read_file",
            "apply_patch",
            "shell",
            "read_image",
            "scheduled_task",
        ]

        plan_res = await client.update_thread_settings(
            "plan", builtin_tools=["read_file", "shell"], thread_id="default"
        )
        assert plan_res.collaboration_mode.mode == "plan"
        assert "read_file" in plan_res.builtin_tools

        empty_res = await client.update_thread_settings(
            "plan", builtin_tools=[], thread_id="default"
        )
        assert empty_res.builtin_tools == []

        plan_off = await client.set_collaboration_mode("default", thread_id="default")
        assert plan_off.collaboration_mode.mode == "default"

        # Read and fork thread history after workflow operations. The App
        # Server binds the workflow service to the initially opened Thread.
        cp = await client.read_thread("default")
        assert isinstance(cp, ThreadCheckpoint)
        assert cp.thread_id == "default"

        fork_res = await client.fork_thread("default", "thread-forked")
        assert fork_res.thread_id == "thread-forked"
        closed = await client.close_thread("thread-forked")
        assert closed is True

        session_fork = await client.fork_session("default", "thread-session-fork")
        assert session_fork.thread_id == "thread-session-fork"
        assert session_fork.session_id
        assert session_fork.parent_session_id
        assert session_fork.context_after_bytes <= session_fork.context_before_bytes

        notebook = await client.write_notebook(
            "architecture.child_session",
            "Child state is independent.",
            importance="high",
            keywords=["child", "checkpoint"],
            evidence=[
                {
                    "kind": "commit",
                    "project": "mini-codex",
                    "commit": "3941fcc",
                    "subject": "feat: implement session notebook and subagent policy",
                    "committedAt": "2026-09-18T10:00:00Z",
                }
            ],
        )
        assert notebook["entries"][0]["importance"] == "high"
        assert notebook["entries"][0]["keywords"] == ["child", "checkpoint"]
        notebook = await client.read_notebook()
        assert notebook["entries"][0]["key"] == "architecture.child_session"
        notebook = await client.forget_notebook("architecture.child_session")
        assert notebook["entries"] == []

        # 4. World Governance & MCP
        world = await client.get_world_state()
        assert hasattr(world, "context") and bool(world.context)

        refresh_res = await client.refresh_world()
        assert hasattr(refresh_res, "changed")

        exec_res = await client.set_world_execution(
            access="project", policy="interactive"
        )
        assert hasattr(exec_res, "changed")

        mcp_res = await client.get_mcp_status()
        assert hasattr(mcp_res, "tool_count")

        mcp_retry = await client.retry_mcp()
        assert hasattr(mcp_retry, "tool_count")

        # 6. Session Info (None in ephemeral mode or SessionInfo when session database is active)
        session_info = await client.get_session_info()
        assert session_info is None or hasattr(session_info, "session_id")

    assert not (tmp_path / "plan").exists()
    assert not (tmp_path / "goal").exists()
    assert not (tmp_path / "plan_mode.json").exists()


@pytest.mark.asyncio
async def test_thread_goal_api_mapping_without_starting_goal_runtime():
    client = MiniAgentClient()
    calls = []
    goal = {
        "threadId": "default",
        "objective": "Build a resilient distributed cache",
        "status": "active",
        "tokenBudget": 4096,
        "tokensUsed": 0,
        "timeUsedSeconds": 0,
        "createdAt": 1,
        "updatedAt": 1,
    }

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "thread/goal/set":
            return {"value": {"goal": goal}, "stateRevision": 4}
        if method == "thread/goal/get":
            return {"value": {"goal": goal}, "stateRevision": 4}
        if method == "thread/goal/clear":
            return {"value": {"cleared": True}, "stateRevision": 5}
        raise AssertionError(f"unexpected method: {method}")

    client._send_request = fake_send

    goal_res = await client.set_goal(goal["objective"], token_budget=4096)
    current_goal = await client.get_goal()
    cleared_goal = await client.clear_goal()

    assert goal_res.goal.objective == goal["objective"]
    assert goal_res.goal.token_budget == 4096
    assert goal_res.state_revision == 4
    assert current_goal.goal == goal_res.goal
    assert current_goal.state_revision == 4
    assert cleared_goal.cleared is True
    assert cleared_goal.state_revision == 5
    assert calls == [
        (
            "thread/goal/set",
            {
                "threadId": "default",
                "objective": goal["objective"],
                "tokenBudget": 4096,
            },
        ),
        ("thread/goal/get", {"threadId": "default"}),
        ("thread/goal/clear", {"threadId": "default"}),
    ]


@pytest.mark.asyncio
async def test_sdk_runtime_observation_api_mapping():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "runtime/status":
            return {
                "phase": "goal_verification",
                "threadId": "thread-1",
                "turnId": "turn-2",
                "operationId": "goal-verification:g-1:9",
                "checkpointSeq": 9,
                "stateRevision": 12,
                "timestampMs": 1234,
            }
        if method == "turn/events":
            return {
                "data": [
                    {
                        "threadId": "thread-1",
                        "sequence": 8,
                        "eventType": "run_started",
                    }
                ],
                "nextCursor": 8,
                "oldestSequence": 1,
                "hasGap": False,
            }
        raise AssertionError(f"unexpected method: {method}")

    client._send_request = fake_send

    status = await client.get_runtime_status("thread-1")
    events = await client.replay_events("thread-1", after_sequence=7, limit=16)

    assert isinstance(status, RuntimeStatus)
    assert status.phase == "goal_verification"
    assert status.operation_id == "goal-verification:g-1:9"
    assert isinstance(events, TurnEventsResult)
    assert events.next_cursor == 8
    assert events.data[0]["sequence"] == 8
    assert "event" not in events.data[0]
    assert calls == [
        ("runtime/status", {"threadId": "thread-1"}),
        (
            "turn/events",
            {"threadId": "thread-1", "afterSequence": 7, "limit": 16},
        ),
    ]


@pytest.mark.asyncio
async def test_sdk_web_search_settings_omits_unsupplied_credentials():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"value": {"settings": {"provider": "exa", "exaApiKeyConfigured": True}}}

    client._send_request = fake_send

    current = await client.get_web_search_settings()
    updated = await client.update_web_search_settings("exa", exa_api_key="new-key")

    assert current["value"]["settings"]["provider"] == "exa"
    assert updated["value"]["settings"]["exaApiKeyConfigured"] is True
    assert calls == [
        ("web/search/settings/read", {}),
        ("web/search/settings/update", {"provider": "exa", "exaApiKey": "new-key"}),
    ]


@pytest.mark.asyncio
async def test_sdk_session_fork_api_mapping():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {
            "value": {
                "sessionId": "s-child",
                "threadId": "thread-child",
                "path": "sessions/s-child/session.jsonl",
                "parentSessionId": "s-parent",
                "parentCheckpointSeq": 7,
                "sessionBytes": 2048,
                "contextBeforeBytes": 8192,
                "contextAfterBytes": 4096,
                "compacted": True,
                "method": "model_summary",
            }
        }

    client._send_request = fake_send
    result = await client.fork_session("thread-parent", "thread-child")

    assert result.session_id == "s-child"
    assert result.parent_checkpoint_seq == 7
    assert result.context_after_bytes == 4096
    assert result.compacted is True
    assert calls == [
        (
            "session/fork",
            {
                "sourceThreadId": "thread-parent",
                "newThreadId": "thread-child",
                "contextPolicy": "exact",
            },
        )
    ]


@pytest.mark.asyncio
async def test_sdk_scheduled_task_api_mapping():
    client = MiniAgentClient()
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        if method == "scheduled-task/list":
            return {
                "value": {
                    "data": [
                        {
                            "taskId": "check-action",
                            "ownerThreadId": "thread-1",
                            "state": "ready",
                            "triggerType": "delay",
                            "summary": "Check GitHub Action",
                            "createdAt": 100,
                            "dueAt": 130,
                            "readyAt": 131,
                        }
                    ]
                }
            }
        return {
            "value": {
                "taskId": "check-action",
                "ownerThreadId": "thread-1",
                "state": "cancelled",
                "triggerType": "delay",
                "summary": "Check GitHub Action",
                "createdAt": 100,
                "dueAt": 130,
                "cancelledAt": 140,
            }
        }

    client._send_request = fake_send
    listed = await client.list_scheduled_tasks("thread-1")
    cancelled = await client.cancel_scheduled_task("check-action", "thread-1")

    assert listed[0].state == "ready"
    assert listed[0].summary == "Check GitHub Action"
    assert cancelled.state == "cancelled"
    assert calls == [
        ("scheduled-task/list", {"threadId": "thread-1"}),
        (
            "scheduled-task/cancel",
            {"threadId": "thread-1", "taskId": "check-action"},
        ),
    ]


@pytest.mark.asyncio
async def test_sdk_approval_response_uses_typed_decision():
    async def approval_handler(params):
        assert params["requestId"] == "approval-1"
        assert params["actionSummary"] == "shell"
        assert params["callId"] == "call-1"
        return {"decision": "approve", "grantScope": "once"}

    client = MiniAgentClient(approval_handler=approval_handler)
    calls = []

    async def fake_send(method, params=None):
        calls.append((method, params))
        return {"accepted": True}

    client._send_request = fake_send

    await client._handle_approval_request(
        {
            "requestId": "approval-1",
            "actionSummary": "shell",
            "access": "project",
            "allowedGrantScopes": ["once"],
            "threadId": "thread-1",
            "turnId": "turn-1",
            "callId": "call-1",
        }
    )

    assert calls == [
        (
            "approval/respond",
            {
                "requestId": "approval-1",
                "decision": "approve",
                "grantScope": "once",
            },
        )
    ]


@pytest.mark.asyncio
async def test_sdk_request_timeout_cleans_pending_request():
    class FakeStdin:
        def write(self, _data):
            return None

        async def drain(self):
            return None

    class FakeProcess:
        stdin = FakeStdin()
        returncode = None

    client = MiniAgentClient(request_timeout=0.01)
    client._proc = FakeProcess()

    with pytest.raises(ServerProcessError, match="request 'initialize' timed out"):
        await client._send_request("initialize")

    assert client._pending_requests == {}
