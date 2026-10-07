from __future__ import annotations

import asyncio
import os
import signal
import sys

import pytest
from mini_agent import MiniAgentClient
from mini_agent import client as client_module
from mini_agent.rpc_metrics import MAX_RPC_METHODS, RpcMetrics


def test_rpc_metrics_are_bounded_and_never_retain_payloads():
    metrics = RpcMetrics()
    for index in range(MAX_RPC_METHODS * 4):
        method = f"method/{index}"
        metrics.request_started(method, 100, 0.25)
        metrics.request_finished(method, 4.0, success=index % 2 == 0)
        metrics.response_read(method, 80, 0.5)

    snapshot = metrics.snapshot(pending_requests=2)
    assert len(snapshot["methods"]) <= MAX_RPC_METHODS
    assert snapshot["methods"]["other"]["requests"] > 0
    assert "payload" not in repr(snapshot)
    assert snapshot["pending_requests"] == 2


def test_rpc_metrics_report_stage_times_and_latency_percentiles():
    metrics = RpcMetrics()
    metrics.request_started("thread/read", 17, 0.4)
    metrics.request_written("thread/read", 0.6)
    metrics.request_finished("thread/read", 50, success=True)
    metrics.response_read("thread/read", 25, 1.2)
    metrics.notification_sent("turn/steer", 14, 0.2, 0.3)
    metrics.notification_received("turn/event", 32, 0.7, 0.8)
    metrics.notification_handler_finished("turn/event", 2.5)

    snapshot = metrics.snapshot(0)
    request = snapshot["methods"]["thread/read"]
    assert request["latency_p50_ms"] == 50
    assert request["latency_p95_ms"] == 50
    assert request["serialization_ms"] == pytest.approx(0.4)
    assert request["write_ms"] == pytest.approx(0.6)
    assert request["decode_ms"] == pytest.approx(1.2)
    outbound = snapshot["methods"]["turn/steer"]
    assert outbound["serialization_ms"] == pytest.approx(0.2)
    assert outbound["write_ms"] == pytest.approx(0.3)
    event = snapshot["methods"]["turn/event"]
    assert event["dispatch_ms"] == pytest.approx(0.8)
    assert event["handler_ms"] == pytest.approx(2.5)


def test_latency_percentiles_are_unknown_when_the_sample_exceeds_the_last_bucket():
    metrics = RpcMetrics()
    metrics.request_started("thread/read", 1, 0)
    metrics.request_finished("thread/read", 9_000, success=True)

    request = metrics.snapshot(0)["methods"]["thread/read"]
    assert request["latency_p50_ms"] is None
    assert request["latency_p95_ms"] is None


@pytest.mark.asyncio
async def test_process_id_and_graceful_stop_keep_unconfirmed_process_attached():
    class FakeStdin:
        def __init__(self):
            self.closed = False

        def is_closing(self):
            return self.closed

        def close(self):
            self.closed = True

    class FakeProcess:
        pid = 5678
        returncode = None

        def __init__(self):
            self.stdin = FakeStdin()
            self.terminated = False

        async def wait(self):
            return None

        def terminate(self):
            self.terminated = True

    client = MiniAgentClient()
    process = FakeProcess()
    client._proc = process

    assert client.process_id == 5678
    assert await client.stop(force=False, timeout=0.1) is False
    assert client.is_running
    assert process.terminated is False


@pytest.mark.asyncio
async def test_forced_stop_signals_the_isolated_app_server_process_group(monkeypatch):
    class FakeStdin:
        def __init__(self):
            self.closed = False

        def is_closing(self):
            return self.closed

        def close(self):
            self.closed = True

    class FakeProcess:
        def __init__(self):
            self.pid = 5678
            self.returncode = None
            self.stdin = FakeStdin()
            self.exited = asyncio.Event()

        async def wait(self):
            await self.exited.wait()

        def terminate(self):
            raise AssertionError("POSIX shutdown should target the process group")

        def kill(self):
            raise AssertionError("POSIX shutdown should target the process group")

    process = FakeProcess()
    signals = []

    def signal_group(process_group_id, signal_number):
        assert process_group_id == process.pid
        signals.append(signal_number)
        if signal_number == signal.SIGKILL:
            process.returncode = -signal.SIGKILL
            process.exited.set()

    monkeypatch.setattr(client_module.os, "killpg", signal_group)
    client = MiniAgentClient()
    client._proc = process

    assert await client.stop(force=True, timeout=0.01) is True
    assert signals == [signal.SIGTERM, signal.SIGKILL]
    assert client.process_id is None


@pytest.mark.asyncio
@pytest.mark.skipif(os.name == "nt", reason="POSIX process-group behavior")
async def test_process_group_signal_stops_nested_app_server_child(tmp_path):
    marker = tmp_path / "nested-child-survived"
    child = (
        "import pathlib,sys,time; time.sleep(0.5); "
        "pathlib.Path(sys.argv[1]).write_text('alive')"
    )
    parent = (
        "import subprocess,sys,time; "
        "subprocess.Popen([sys.executable,'-c',sys.argv[2],sys.argv[1]]); "
        "time.sleep(30)"
    )
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        parent,
        str(marker),
        child,
        start_new_session=True,
    )
    try:
        await asyncio.sleep(0.05)
        client_module._signal_app_server(process, signal.SIGTERM)
        await asyncio.wait_for(process.wait(), timeout=1.0)
        await asyncio.sleep(0.6)
        assert not marker.exists()
    finally:
        if process.returncode is None:
            client_module._signal_app_server(process, signal.SIGKILL)
            await asyncio.wait_for(process.wait(), timeout=1.0)
