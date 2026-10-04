"""
Mini Agent Python Client SDK
Asynchronous JSON-RPC 2.0 Client over stdio transport.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
import sys
from collections.abc import AsyncIterator, Awaitable, Callable
from dataclasses import replace
from typing import Any, Literal, Self

from mini_agent.approval_logging import approval_log_fields
from mini_agent.errors import (
    AppServerError,
    AppServerRequestTimeoutError,
    ProtocolVersionMismatchError,
    ServerProcessError,
    StreamEventOverflowError,
    TurnTimeoutError,
)
from mini_agent.events import parse_event
from mini_agent.types import (
    DEFAULT_BUILTIN_TOOLS,
    BackgroundTask,
    BackgroundTaskLogs,
    CollaborationMode,
    CollaborationModeKind,
    ItemLifecycleNotification,
    ItemSortDirection,
    McpRetryResult,
    McpStatusResult,
    RuntimeStatus,
    ScheduledTask,
    SessionContextManifestResult,
    SessionForkResult,
    SessionInfo,
    ThreadCheckpoint,
    ThreadForkResult,
    ThreadGoalClearResult,
    ThreadGoalGetResult,
    ThreadGoalSetResult,
    ThreadGoalStatus,
    ThreadItem,
    ThreadItemsListResult,
    ThreadListResult,
    ThreadResumeResult,
    ThreadSettingsResult,
    TurnEventsResult,
    TurnReadResult,
    TurnReconcileDisposition,
    TurnReconcileResult,
    TurnSubmissionResult,
    WorkflowState,
    WorldRefreshResult,
    WorldSetExecutionResult,
    WorldStateResult,
)

logger = logging.getLogger("mini_agent")

DEFAULT_REQUEST_TIMEOUT_SECS = 30.0
APP_SERVER_PROTOCOL_VERSION = 2
STREAM_EVENT_QUEUE_LIMIT = 512
STREAM_EVENT_QUEUE_BYTE_LIMIT = 2 * 1024 * 1024
STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT = 8 * 1024 * 1024
STREAM_EVENT_LARGE_MESSAGE_BYTE_LIMIT = 128 * 1024 * 1024
STREAM_EVENT_LARGE_QUEUES_TOTAL_BYTE_LIMIT = 256 * 1024 * 1024
_STREAM_QUEUE_SIZE_KEY = "__mini_agent_sdk_queued_bytes"
_STREAM_QUEUE_LARGE_SIZE_KEY = "__mini_agent_sdk_queued_large_bytes"
# Model responses are capped at 16 MiB, but JSON can expand control characters
# up to six times. Leave room for that event and its JSON-RPC envelope.
APP_SERVER_STDIO_LINE_LIMIT = 129 * 1024 * 1024


def setup_logging(
    log_dir: str | None = "logs",
    log_file: str | None = None,
    level: int | str = logging.DEBUG,
    console: bool = False,
    format_str: str | None = None,
    mode: str = "a",
) -> logging.FileHandler | None:
    """
    Configure detailed file (and optional console) logging for mini-agent.

    :param log_dir: Target directory for log files (default: 'logs').
    :param log_file: Specific log filename (default: '{log_dir}/mini-agent.log').
    :param level: Logging level (default: logging.DEBUG).
    :param console: Whether to also attach a console stream handler.
    :param format_str: Custom logging format string.
    :param mode: File open mode ('a' for append, 'w' for overwrite/refresh on start).
    :return: The FileHandler instance, or None if no file target specified.
    """
    if isinstance(level, str):
        level = getattr(logging, level.upper(), logging.DEBUG)

    target_file: str | None = None
    if log_file:
        target_file = log_file
        target_dir = os.path.dirname(target_file)
        if target_dir:
            os.makedirs(target_dir, exist_ok=True)
    elif log_dir:
        os.makedirs(log_dir, exist_ok=True)
        # Automatically derive script-specific log name (e.g. 01_basic_turn.log)
        script_name = "mini-agent"
        if sys.argv and sys.argv[0]:
            base = os.path.basename(sys.argv[0])
            name, _ = os.path.splitext(base)
            if name and name not in ("-c", "<stdin>", "__main__", "pytest", "python"):
                script_name = name
        target_file = os.path.join(log_dir, f"{script_name}.log")

    fmt = format_str or "[%(asctime)s] [%(levelname)s] [%(name)s] %(message)s"
    formatter = logging.Formatter(fmt)

    # Configure the mini_agent logger level
    logger.setLevel(level)

    handler = None
    if target_file:
        abs_target = os.path.abspath(target_file)
        for h in list(logger.handlers):
            if (
                isinstance(h, logging.FileHandler)
                and getattr(h, "baseFilename", None) == abs_target
            ):
                if mode == "w":
                    logger.removeHandler(h)
                    h.close()
                else:
                    handler = h
                break
        if handler is None:
            handler = logging.FileHandler(target_file, mode=mode, encoding="utf-8")
            handler.setLevel(level)
            handler.setFormatter(formatter)
            logger.addHandler(handler)

    if console:
        has_console = any(
            isinstance(h, logging.StreamHandler)
            and not isinstance(h, logging.FileHandler)
            for h in logger.handlers
        )
        if not has_console:
            ch = logging.StreamHandler(sys.stderr)
            ch.setLevel(level)
            ch.setFormatter(formatter)
            logger.addHandler(ch)

    return handler


def _ensure_utf8_console() -> None:
    """Safely configure stdout/stderr for UTF-8 on Windows consoles."""
    if sys.platform == "win32":
        for stream_name in ("stdout", "stderr"):
            stream = getattr(sys, stream_name, None)
            if stream is not None and hasattr(stream, "reconfigure"):
                try:
                    stream.reconfigure(encoding="utf-8")
                except OSError:
                    pass


def _redact_secrets(value: Any) -> Any:
    """Return a log-safe copy of nested JSON-RPC parameters."""
    if isinstance(value, dict):
        redacted: dict[str, Any] = {}
        for key, item in value.items():
            normalized = str(key).replace("_", "").replace("-", "").lower()
            redacted[key] = (
                "[REDACTED]"
                if normalized in {"apikey", "authorization", "password", "secret"}
                else _redact_secrets(item)
            )
        return redacted
    if isinstance(value, list):
        return [_redact_secrets(item) for item in value]
    return value


def _env_search_dirs(cwd: str) -> list[str]:
    """Match the App Server's workspace and user configuration search paths."""
    return [cwd, os.path.expanduser("~/.mini-agent")]


def _find_and_load_env(cwd: str) -> dict[str, str]:
    """Lightweight built-in .env parser without external dependencies."""
    env_vars: dict[str, str] = {}
    for d in _env_search_dirs(cwd):
        env_path = os.path.join(d, ".env")
        if os.path.isfile(env_path):
            try:
                with open(env_path, encoding="utf-8") as f:
                    for line in f:
                        line = line.strip()
                        if not line or line.startswith("#") or "=" not in line:
                            continue
                        k, v = line.split("=", 1)
                        k = k.strip()
                        v = v.strip().strip("'\"")
                        if k and k not in env_vars:
                            env_vars[k] = v
            except OSError:
                pass
    return env_vars


class MiniAgentClient:
    """Asynchronous Client for mini-agent-app-server."""

    def __init__(
        self,
        executable: str = "mini-agent-app-server",
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        approval_handler: Callable[..., Awaitable[Any]] | None = None,
        notification_handler: Callable[[dict[str, Any]], Awaitable[None]] | None = None,
        log_dir: str | None = None,
        log_file: str | None = None,
        log_level: str | int | None = None,
        log_mode: str | None = None,
        request_timeout: float = DEFAULT_REQUEST_TIMEOUT_SECS,
        user_questions: bool = False,
    ):
        """
        Initialize the MiniAgentClient.

        :param executable: Path or command name for mini-agent-app-server binary.
        :param cwd: Working directory for the server process (defaults to current directory).
        :param env: Additional environment variables.
        :param approval_handler: Optional async callback for handling sensitive tool approvals.
                                 When omitted, the SDK denies approval requests by default
                                 while still exposing approval records in stream_turn().
        :param log_dir: Target directory for execution logs (e.g. 'logs').
        :param log_file: Specific log file path (e.g. 'logs/01_basic_turn.log').
        :param log_level: Logging level ('DEBUG', 'INFO', logging.DEBUG, etc.).
        :param log_mode: Log file open mode ('a' for append, 'w' for overwrite/fresh log).
        :param request_timeout: Timeout in seconds for one JSON-RPC request/response.
        """
        _ensure_utf8_console()
        self.executable = executable
        self.cwd = cwd or os.getcwd()
        file_env = _find_and_load_env(self.cwd)
        # Priority: explicit env arg > process os.environ > .env file
        self.env = {**file_env, **os.environ, **(env or {})}
        self.approval_handler = approval_handler
        self.notification_handler = notification_handler
        self.user_questions = user_questions
        self._access_scope = "project"
        self._policy = "interactive"

        # Configure file logging if log_dir, log_file or env specified
        eff_dir = log_dir or self.env.get("MINI_AGENT_LOG_DIR")
        eff_file = log_file or self.env.get("MINI_AGENT_LOG_FILE")
        eff_level = log_level or self.env.get("MINI_AGENT_LOG_LEVEL", "INFO")
        eff_mode = log_mode or self.env.get("MINI_AGENT_LOG_MODE", "a")
        if eff_dir or eff_file:
            setup_logging(
                log_dir=eff_dir,
                log_file=eff_file,
                level=eff_level,
                mode=eff_mode,
            )

        self._proc: asyncio.subprocess.Process | None = None
        self._reader_task: asyncio.Task[None] | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._next_id: int = 1
        self._pending_requests: dict[int, asyncio.Future[Any]] = {}
        self._event_queues: list[asyncio.Queue[dict[str, Any]]] = []
        self._event_queue_threads: dict[asyncio.Queue[dict[str, Any]], str] = {}
        self._event_queue_bytes: dict[asyncio.Queue[dict[str, Any]], int] = {}
        self._event_queue_large_bytes: dict[asyncio.Queue[dict[str, Any]], int] = {}
        self._overflowed_event_queues: set[asyncio.Queue[dict[str, Any]]] = set()
        self._queued_event_bytes = 0
        self._queued_large_event_bytes = 0
        self._active_thread_id: str = "default"
        self.capability_manifest: dict[str, Any] = {}
        self._thread_settings: dict[str, ThreadSettingsResult] = {}
        self._world_execution_configured = False
        self._session_info: SessionInfo | None = None
        self._client_name = "python-sdk"
        self._client_version = "1.0.0"
        self._provider_selection: dict[str, Any] | None = None
        if request_timeout <= 0:
            raise ValueError("request_timeout must be positive")
        self.request_timeout = request_timeout

    async def __aenter__(self) -> Self:
        await self.start()
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: object,
    ) -> None:
        await self.stop()

    async def start(self) -> None:
        """Start the mini-agent-app-server subprocess and reader loop."""
        executable = self.executable
        if executable == "mini-agent-app-server":
            executable = self.env.get("MINI_AGENT_APP_SERVER_PATH", executable)
        exe_path = shutil.which(executable, path=self.env.get("PATH"))
        if not exe_path:
            exe_path = executable

        try:
            self._proc = await asyncio.create_subprocess_exec(
                exe_path,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                cwd=self.cwd,
                env=self.env,
                limit=APP_SERVER_STDIO_LINE_LIMIT,
            )
        except OSError as err:
            raise ServerProcessError(
                f"Failed to spawn mini-agent-app-server executable '{exe_path}': {err}"
            ) from err

        self._reader_task = asyncio.create_task(self._read_loop())
        self._stderr_task = asyncio.create_task(self._stderr_loop())
        logger.debug("mini-agent-app-server started (PID: %d)", self._proc.pid)

    async def stop(self) -> None:
        """Gracefully stop the server process."""
        process = self._proc
        for queue in self._event_queues:
            self._enqueue_stream_message(
                queue,
                {
                    "type": "_client_error",
                    "message": "App Server stopped before stream settlement",
                },
            )
        # 1. Terminate/kill the child process first so stdout/stderr receive EOF immediately
        if process and process.returncode is None:
            if process.stdin and not process.stdin.is_closing():
                try:
                    process.stdin.close()
                except Exception:  # noqa: BLE001, S110
                    pass
            try:
                process.terminate()
            except Exception:  # noqa: BLE001, S110
                pass
            try:
                await asyncio.wait_for(process.wait(), timeout=1.0)
            except (asyncio.TimeoutError, Exception):  # noqa: BLE001
                try:
                    process.kill()
                    await asyncio.wait_for(process.wait(), timeout=1.0)
                except Exception:  # noqa: BLE001, S110
                    pass

        # 2. Cancel and wait for reader/stderr tasks
        for task in (self._reader_task, self._stderr_task):
            if task and not task.done():
                task.cancel()
                try:
                    await asyncio.wait_for(asyncio.shield(task), timeout=0.5)
                except (asyncio.CancelledError, asyncio.TimeoutError, Exception):  # noqa: BLE001, S110
                    pass

        # 3. Reject any pending futures
        for fut in self._pending_requests.values():
            if not fut.done():
                fut.set_exception(RuntimeError("App Server stopped"))
        self._pending_requests.clear()

        # Process.wait() reaps the child but does not always close the
        # Proactor pipe transport on Windows before pytest closes its loop.
        # Close the transport explicitly and release references so its
        # destructor cannot report an unclosed stdin/stdout/stderr pipe.
        if process:
            transport = getattr(process, "_transport", None)
            if transport is not None:
                try:
                    transport.close()
                except Exception:  # noqa: BLE001, S110
                    pass
                await asyncio.sleep(0)
        self._proc = None
        self._reader_task = None
        self._stderr_task = None

    @property
    def is_running(self) -> bool:
        """Return True if the underlying mini-agent-app-server subprocess is active."""
        return self._proc is not None and self._proc.returncode is None

    async def restart(self) -> dict[str, Any]:
        """Restart the process, resuming its durable Session when available."""
        if self._session_info is not None:
            if self.env.get("MINI_AGENT_SESSION_MODE") == "new":
                self.env["MINI_AGENT_SESSION_MODE"] = "resume"
                self.env["MINI_AGENT_SESSION_ID"] = self._session_info.session_id
            self._thread_settings = {
                thread_id: replace(settings, state_revision=None)
                for thread_id, settings in self._thread_settings.items()
            }
        else:
            self._thread_settings.clear()
        await self.stop()
        await self.start()
        res: dict[str, Any] = await self.initialize(
            client_name=self._client_name,
            client_version=self._client_version,
            providers=self._provider_selection,
        )
        if self._world_execution_configured:
            await self.set_world_execution(self._access_scope, self._policy)
        if self._active_thread_id:
            await self.start_thread(self._active_thread_id)
        return res

    # -------------------------------------------------------------------------
    # JSON-RPC Low-level Communication
    # -------------------------------------------------------------------------

    async def _send_request(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> Any:
        """Send a JSON-RPC request and wait for correlated response."""
        if not self._proc or not self._proc.stdin or self._proc.returncode is not None:
            raise ServerProcessError("App Server process is not running")

        req_id = self._next_id
        self._next_id += 1

        payload = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params if params is not None else {},
        }
        data = json.dumps(payload) + "\n"

        loop = asyncio.get_running_loop()
        request_timeout = self.request_timeout if timeout is None else timeout
        deadline = loop.time() + request_timeout
        future: asyncio.Future[Any] = loop.create_future()
        self._pending_requests[req_id] = future

        logger.debug(
            ">>> SEND: %s",
            json.dumps(_redact_secrets(payload), ensure_ascii=False),
        )
        try:
            await asyncio.wait_for(
                self._write_request(data), timeout=max(0, deadline - loop.time())
            )
            return await asyncio.wait_for(
                future, timeout=max(0, deadline - loop.time())
            )
        except asyncio.TimeoutError as err:
            raise AppServerRequestTimeoutError(method, request_timeout) from err
        finally:
            # The reader normally removes completed requests. On a timeout or
            # transport failure there is no response left to correlate.
            if self._pending_requests.get(req_id) is future:
                self._pending_requests.pop(req_id, None)

    async def _write_request(self, data: str) -> None:
        """Write one bounded JSON-RPC request with the same timeout budget."""
        assert self._proc and self._proc.stdin
        self._proc.stdin.write(data.encode("utf-8"))
        await self._proc.stdin.drain()

    async def _send_notification(
        self, method: str, params: dict[str, Any] | None = None
    ) -> None:
        """Send a JSON-RPC notification (no response expected)."""
        if not self._proc or not self._proc.stdin or self._proc.returncode is not None:
            raise ServerProcessError("App Server process is not running")

        payload = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params if params is not None else {},
        }
        data = json.dumps(payload) + "\n"
        logger.debug(">>> NOTIFY: %s", data.strip())
        self._proc.stdin.write(data.encode("utf-8"))
        await self._proc.stdin.drain()

    async def _read_loop(self) -> None:
        """Background loop reading JSONL lines from server stdout."""
        process = self._proc
        assert process and process.stdout
        error_message = "App Server connection closed before stream settlement"
        while True:
            try:
                line_bytes = await process.stdout.readline()
            except Exception as err:
                logger.exception("App Server stdout read failed")
                error_message = (
                    f"App Server connection failed before stream settlement: {err}"
                )
                break
            if not line_bytes:
                break
            line = line_bytes.decode("utf-8", errors="replace").strip()
            if not line:
                continue

            logger.debug("<<< RECV: %s", line)
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                logger.warning("Failed to decode JSON from server: %s", line)
                continue

            # 1. Correlated response (has id)
            if "id" in msg and msg["id"] is not None:
                req_id = msg["id"]
                future = self._pending_requests.pop(req_id, None)
                if future and not future.done():
                    if "error" in msg:
                        err = msg["error"]
                        future.set_exception(
                            AppServerError(
                                err.get("code", -32603),
                                err.get("message", "Unknown error"),
                                err.get("data"),
                            )
                        )
                    else:
                        future.set_result(msg.get("result"))

            # 2. Server notifications (has method, no id)
            elif "method" in msg:
                method = msg["method"]
                params = msg.get("params", {})

                if method == "turn/event":
                    for q in self._event_queues:
                        target_thread = self._event_queue_threads.get(q)
                        if (
                            target_thread is not None
                            and params.get("threadId") != target_thread
                        ):
                            continue
                        self._enqueue_stream_message(q, params)
                    if self.notification_handler is not None:
                        try:
                            await self.notification_handler({"type": "event", **params})
                        except Exception:
                            logger.exception("Runtime notification handler failed")

                elif method == "approval/request":
                    await self._publish_approval(params, "requested")
                    asyncio.create_task(self._handle_approval_request(params))

                elif method == "approval/resolved":
                    await self._publish_approval(params, "resolved")

                else:
                    notification = {
                        "type": "notification",
                        "method": method,
                        "data": params,
                    }
                    if method == "thread/settings/updated":
                        settings = ThreadSettingsResult.from_dict(params)
                        thread_id = str(
                            params.get("threadId")
                            or params.get("thread_id")
                            or self._active_thread_id
                        )
                        self._cache_thread_settings(thread_id, settings)
                    if method in ("item/started", "item/completed"):
                        notification["typed_item_notification"] = (
                            ItemLifecycleNotification.from_dict(method, params)
                        )
                    for q in self._event_queues:
                        target_thread = self._event_queue_threads.get(q)
                        notification_thread = params.get("threadId") or params.get(
                            "thread_id"
                        )
                        interaction = params.get("interaction")
                        if notification_thread is None and isinstance(
                            interaction, dict
                        ):
                            notification_thread = interaction.get(
                                "threadId"
                            ) or interaction.get("thread_id")
                        if (
                            target_thread is not None
                            and notification_thread is not None
                            and notification_thread != target_thread
                        ):
                            continue
                        self._enqueue_stream_message(q, notification)
                    if self.notification_handler is not None:
                        asyncio.create_task(self.notification_handler(notification))
                    logger.debug("Received server notification: %s", method)

        # A reader failure/EOF must not leave a live-looking client around.
        # Otherwise a later stop or request can wait on a dead pipe until the
        # generic request timeout, which is the source of long "stopping"
        # states after a transport error.
        if getattr(process, "returncode", None) is None:
            try:
                process.terminate()
            except Exception:  # noqa: BLE001, S110
                pass
            try:
                await asyncio.wait_for(process.wait(), timeout=1.0)
            except (asyncio.TimeoutError, Exception):  # noqa: BLE001
                try:
                    process.kill()
                    await asyncio.wait_for(process.wait(), timeout=1.0)
                except Exception:  # noqa: BLE001, S110
                    pass
        transport = getattr(process, "_transport", None)
        if transport is not None:
            try:
                transport.close()
            except Exception:  # noqa: BLE001, S110
                pass
        if self._proc is process and getattr(process, "returncode", None) is not None:
            self._proc = None

        for fut in self._pending_requests.values():
            if not fut.done():
                fut.set_exception(ServerProcessError(error_message))
        self._pending_requests.clear()
        for q in self._event_queues:
            self._enqueue_stream_message(
                q, {"type": "_client_error", "message": error_message}
            )
        if self.notification_handler is not None:
            try:
                await self.notification_handler(
                    {
                        "type": "runtime_error",
                        "threadId": self._active_thread_id,
                        "message": error_message,
                    }
                )
            except Exception:
                logger.exception("Runtime error notification handler failed")

    async def _stderr_loop(self) -> None:
        """Background loop reading and logging any stderr output from the server."""
        process = self._proc
        assert process and process.stderr
        while True:
            line_bytes = await process.stderr.readline()
            if not line_bytes:
                break
            line = line_bytes.decode("utf-8", errors="replace").strip()
            if line:
                logger.warning("[Server STDERR]: %s", line)

    async def _publish_approval(self, params: dict[str, Any], phase: str) -> None:
        approval = {**params, "phase": phase}
        tool_name, summary_counts = approval_log_fields(approval)
        logger.info(
            "[Approval] %s tool=%s summary=%s request_id=%s",
            phase,
            tool_name,
            summary_counts,
            approval.get("requestId", ""),
        )
        approval_thread = approval.get("threadId") or approval.get("thread_id")
        for q in self._event_queues:
            target_thread = self._event_queue_threads.get(q)
            if (
                target_thread is not None
                and approval_thread is not None
                and approval_thread != target_thread
            ):
                continue
            self._enqueue_stream_message(q, {"type": "approval", "approval": approval})

    def _enqueue_stream_message(
        self, queue: asyncio.Queue[dict[str, Any]], message: dict[str, Any]
    ) -> None:
        """Bound stream count and bytes; turn dropped history into a replay signal."""
        if queue in self._overflowed_event_queues:
            return
        message_bytes = len(
            json.dumps(
                message,
                ensure_ascii=False,
                separators=(",", ":"),
                default=str,
            ).encode("utf-8")
        )
        queue_bytes = self._event_queue_bytes.get(queue, 0)
        normal_budget_exceeded = (
            message_bytes > STREAM_EVENT_QUEUE_BYTE_LIMIT
            or queue_bytes + message_bytes > STREAM_EVENT_QUEUE_BYTE_LIMIT
            or self._queued_event_bytes + message_bytes
            > STREAM_EVENT_QUEUES_TOTAL_BYTE_LIMIT
        )
        if queue.full():
            self._discard_stream_queue(queue)
            self._overflowed_event_queues.add(queue)
            queue.put_nowait({"type": "_stream_overflow"})
            return
        if normal_budget_exceeded:
            if not self._is_large_turn_event(message):
                self._discard_stream_queue(queue)
                self._overflowed_event_queues.add(queue)
                queue.put_nowait({"type": "_stream_overflow"})
                return
            queue_large_bytes = self._event_queue_large_bytes.get(queue, 0)
            if (
                message_bytes > STREAM_EVENT_LARGE_MESSAGE_BYTE_LIMIT
                or queue_large_bytes + message_bytes
                > STREAM_EVENT_LARGE_QUEUES_TOTAL_BYTE_LIMIT
                or self._queued_large_event_bytes + message_bytes
                > STREAM_EVENT_LARGE_QUEUES_TOTAL_BYTE_LIMIT
            ):
                self._discard_stream_queue(queue)
                self._overflowed_event_queues.add(queue)
                queue.put_nowait({"type": "_stream_overflow"})
                return
            queued_message = dict(message)
            queued_message[_STREAM_QUEUE_SIZE_KEY] = 0
            queued_message[_STREAM_QUEUE_LARGE_SIZE_KEY] = message_bytes
            queue.put_nowait(queued_message)
            self._event_queue_large_bytes[queue] = queue_large_bytes + message_bytes
            self._queued_large_event_bytes += message_bytes
            return
        queued_message = dict(message)
        queued_message[_STREAM_QUEUE_SIZE_KEY] = message_bytes
        queue.put_nowait(queued_message)
        self._event_queue_bytes[queue] = queue_bytes + message_bytes
        self._queued_event_bytes += message_bytes

    @staticmethod
    def _is_large_turn_event(message: dict[str, Any]) -> bool:
        """Identify App Server events that carry full model response content."""
        event = message.get("event")
        return isinstance(event, dict) and event.get("type") in {
            "assistant_reasoning_delta",
            "assistant_text_delta",
            "model_responded",
            "tool_started",
        }

    def _release_stream_message(
        self, queue: asyncio.Queue[dict[str, Any]], message: dict[str, Any]
    ) -> None:
        """Release the serialized-byte budget when a queued message is consumed."""
        message_bytes = message.pop(_STREAM_QUEUE_SIZE_KEY, 0)
        if message_bytes:
            self._event_queue_bytes[queue] = max(
                0, self._event_queue_bytes.get(queue, 0) - message_bytes
            )
            self._queued_event_bytes = max(0, self._queued_event_bytes - message_bytes)
        large_message_bytes = message.pop(_STREAM_QUEUE_LARGE_SIZE_KEY, 0)
        if large_message_bytes:
            self._event_queue_large_bytes[queue] = max(
                0, self._event_queue_large_bytes.get(queue, 0) - large_message_bytes
            )
            self._queued_large_event_bytes = max(
                0, self._queued_large_event_bytes - large_message_bytes
            )

    def _discard_stream_queue(self, queue: asyncio.Queue[dict[str, Any]]) -> None:
        """Discard buffered messages while releasing their byte budget."""
        while not queue.empty():
            try:
                message = queue.get_nowait()
            except asyncio.QueueEmpty:
                break
            self._release_stream_message(queue, message)
        self._event_queue_bytes[queue] = 0
        self._event_queue_large_bytes[queue] = 0

    async def _handle_approval_request(self, params: dict[str, Any]) -> None:
        """Handle server approval/request notification."""
        request_id = str(params.get("requestId") or "")
        response: dict[str, Any] = {
            "requestId": request_id,
            "decision": "deny",
            "grantScope": None,
        }
        try:
            if self.approval_handler is not None:
                res = await self.approval_handler(params)

                if not isinstance(res, dict):
                    raise TypeError(
                        "approval handler must return a typed decision object"
                    )
                decision = str(res.get("decision", "")).lower()
                if decision not in ("approve", "deny"):
                    raise ValueError("approval decision must be approve or deny")
                response["requestId"] = request_id
                response["decision"] = decision
                response["grantScope"] = res.get("grantScope")
                if "reason" in res:
                    response["reason"] = res["reason"]
                allowed_scopes = params.get("allowedGrantScopes", [])
                if decision == "approve":
                    if response.get("grantScope") not in allowed_scopes:
                        raise ValueError("grant scope is not allowed for the request")
                elif response.get("grantScope") is not None:
                    raise ValueError("denied approval cannot grant a scope")
            else:
                response["reason"] = "No approval handler configured"
        except Exception as err:  # noqa: BLE001
            logger.error("Approval handler error: %s. Denying by default.", err)
            response["decision"] = "deny"
            response["reason"] = str(err)

        try:
            await self._send_request("approval/respond", response)
        except Exception as err:  # noqa: BLE001
            logger.error("Failed to send approval response: %s", err)

    # -------------------------------------------------------------------------
    # High-level Protocol Methods
    # -------------------------------------------------------------------------

    async def initialize(
        self,
        client_name: str = "python-sdk",
        client_version: str = "1.0.0",
        providers: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Negotiate App Server protocol version 2 and receive capabilities."""
        self._client_name = client_name
        self._client_version = client_version
        self._provider_selection = dict(providers) if providers else None
        params: dict[str, Any] = {
            "protocolVersion": APP_SERVER_PROTOCOL_VERSION,
            "clientName": client_name,
            "clientVersion": client_version,
            "capabilities": {"userQuestions": self.user_questions},
        }
        if providers:
            params["providers"] = providers

        res = await self._send_request("initialize", params)
        if res.get("protocolVersion") != APP_SERVER_PROTOCOL_VERSION:
            raise ProtocolVersionMismatchError(
                f"Unsupported protocol version {res.get('protocolVersion')}"
            )
        self.capability_manifest = res.get("capabilityManifest") or {}
        try:
            self._session_info = await self.get_session_info()
        except AppServerError as err:
            if err.code != -32601:
                raise
            logger.debug("App Server does not expose session/info")
            self._session_info = None
        if self._session_info is not None:
            self._active_thread_id = self._session_info.thread_id
        return res

    # -------------------------------------------------------------------------
    # Thread Management
    # -------------------------------------------------------------------------

    async def start_thread(self, thread_id: str | None = None) -> str:
        """Start or attach to a conversation thread.

        When ``thread_id`` is omitted, attach to the Thread selected by the
        App Server for the current Session.
        """
        params = {"threadId": thread_id} if thread_id is not None else {}
        res = await self._send_request("thread/start", params)
        self._active_thread_id = res.get("threadId", thread_id or "default")
        return self._active_thread_id

    async def list_threads(
        self,
        cursor: str | None = None,
        limit: int | None = None,
    ) -> ThreadListResult:
        """List active and persisted threads."""
        params: dict[str, Any] = {}
        if cursor is not None:
            params["cursor"] = cursor
        if limit is not None:
            params["limit"] = limit
        res = await self._send_request("thread/list", params)
        return ThreadListResult.from_dict(res)

    async def list_skills(self, thread_id: str | None = None) -> list[dict[str, Any]]:
        """Refresh and return the bounded effective Skill catalog for a Thread."""
        res = await self._send_request(
            "skills/list", {"threadId": thread_id or self._active_thread_id}
        )
        value = res.get("value", res) if isinstance(res, dict) else res
        skills = value.get("skills", []) if isinstance(value, dict) else []
        return [skill for skill in skills if isinstance(skill, dict)]

    async def read_thread(self, thread_id: str | None = None) -> ThreadCheckpoint:
        """Read settled checkpoint for thread."""
        res = await self._send_request(
            "thread/read",
            {"threadId": thread_id or self._active_thread_id},
        )
        return ThreadCheckpoint.from_dict(res)

    async def respond_user_question(
        self,
        *,
        interaction_id: str,
        thread_id: str,
        turn_id: str,
        call_id: str,
        question_id: str,
        answer: dict[str, Any],
    ) -> dict[str, Any]:
        """Submit one answer to the active App Server user-question request."""
        return await self._send_request(
            "user-question/respond",
            {
                "interactionId": interaction_id,
                "threadId": thread_id,
                "turnId": turn_id,
                "callId": call_id,
                "questionId": question_id,
                "answer": answer,
            },
        )

    async def get_runtime_status(self, thread_id: str | None = None) -> RuntimeStatus:
        """Read the live App Server runtime status without waiting on the worker."""
        res = await self._send_request(
            "runtime/status",
            {"threadId": thread_id or self._active_thread_id},
        )
        return RuntimeStatus.from_dict(res)

    async def list_background_tasks(
        self, thread_id: str | None = None
    ) -> list[BackgroundTask]:
        """List locally managed background Shell tasks for a Thread runtime."""
        res = await self._send_request(
            "background-task/list", {"threadId": thread_id or self._active_thread_id}
        )
        value = res.get("value", res) if isinstance(res, dict) else res
        return [BackgroundTask.from_dict(item) for item in value.get("data", [])]

    async def read_background_task(
        self, task_id: str, thread_id: str | None = None
    ) -> BackgroundTask:
        res = await self._send_request(
            "background-task/read",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return BackgroundTask.from_dict(res)

    async def read_background_task_logs(
        self, task_id: str, thread_id: str | None = None
    ) -> BackgroundTaskLogs:
        res = await self._send_request(
            "background-task/logs",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return BackgroundTaskLogs.from_dict(res)

    async def stop_background_task(
        self, task_id: str, thread_id: str | None = None
    ) -> BackgroundTask:
        res = await self._send_request(
            "background-task/stop",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return BackgroundTask.from_dict(res)

    async def restart_background_task(
        self, task_id: str, thread_id: str | None = None
    ) -> BackgroundTask:
        res = await self._send_request(
            "background-task/restart",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return BackgroundTask.from_dict(res)

    async def list_scheduled_tasks(
        self, thread_id: str | None = None
    ) -> list[ScheduledTask]:
        """List bounded delay markers for a Thread runtime."""
        res = await self._send_request(
            "scheduled-task/list", {"threadId": thread_id or self._active_thread_id}
        )
        value = res.get("value", res) if isinstance(res, dict) else res
        return [ScheduledTask.from_dict(item) for item in value.get("data", [])]

    async def read_scheduled_task(
        self, task_id: str, thread_id: str | None = None
    ) -> ScheduledTask:
        res = await self._send_request(
            "scheduled-task/read",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return ScheduledTask.from_dict(res)

    async def cancel_scheduled_task(
        self, task_id: str, thread_id: str | None = None
    ) -> ScheduledTask:
        res = await self._send_request(
            "scheduled-task/cancel",
            {"threadId": thread_id or self._active_thread_id, "taskId": task_id},
        )
        return ScheduledTask.from_dict(res)

    async def replay_events(
        self,
        thread_id: str | None = None,
        after_sequence: int | None = None,
        limit: int | None = None,
    ) -> TurnEventsResult:
        """Replay a bounded event page after a per-Thread sequence cursor."""
        params: dict[str, Any] = {"threadId": thread_id or self._active_thread_id}
        if after_sequence is not None:
            params["afterSequence"] = after_sequence
        if limit is not None:
            params["limit"] = limit
        res = await self._send_request("turn/events", params)
        return TurnEventsResult.from_dict(res)

    async def list_thread_items(
        self,
        thread_id: str | None = None,
        turn_id: str | None = None,
        cursor: str | None = None,
        limit: int | None = None,
        sort_direction: ItemSortDirection | None = None,
    ) -> ThreadItemsListResult:
        """Read the bounded Session-backed ThreadItem projection."""
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
        }
        if turn_id is not None:
            params["turnId"] = turn_id
        if cursor is not None:
            params["cursor"] = cursor
        if limit is not None:
            params["limit"] = limit
        if sort_direction is not None:
            params["sortDirection"] = sort_direction
        res = await self._send_request("thread/items/list", params)
        return ThreadItemsListResult.from_dict(res)

    async def close_thread(self, thread_id: str | None = None) -> bool:
        """Close an active thread."""
        res = await self._send_request(
            "thread/close",
            {"threadId": thread_id or self._active_thread_id},
        )
        val = res.get("value", res) if isinstance(res, dict) else res
        return val.get("closed", True) if isinstance(val, dict) else True

    async def fork_thread(
        self,
        source_thread_id: str,
        new_thread_id: str,
    ) -> ThreadForkResult:
        """Fork an existing thread history into a new branched thread."""
        res = await self._send_request(
            "thread/fork",
            {
                "sourceThreadId": source_thread_id,
                "newThreadId": new_thread_id,
            },
        )
        return ThreadForkResult.from_dict(res)

    async def fork_session(
        self,
        source_thread_id: str,
        new_thread_id: str,
        context_policy: str = "exact",
        operation_id: str | None = None,
        operation_attempt: int | None = None,
        operation_prompt: str | None = None,
        operation_group_id: str | None = None,
        execution_mode: str | None = None,
        group_sequence: int | None = None,
    ) -> SessionForkResult:
        """Create an independent persisted Session from a settled checkpoint."""
        params: dict[str, Any] = {
            "sourceThreadId": source_thread_id,
            "newThreadId": new_thread_id,
            "contextPolicy": context_policy,
        }
        if operation_id:
            params["operationId"] = operation_id
        if operation_attempt is not None:
            params["operationAttempt"] = operation_attempt
        if operation_prompt:
            params["operationPrompt"] = operation_prompt
        if operation_group_id:
            params["operationGroupId"] = operation_group_id
        if execution_mode:
            params["executionMode"] = execution_mode
        if group_sequence is not None:
            params["groupSequence"] = group_sequence
        res = await self._send_request(
            "session/fork",
            params,
        )
        return SessionForkResult.from_dict(res)

    async def resume_thread(
        self,
        thread_id: str,
        checkpoint: ThreadCheckpoint | dict[str, Any],
    ) -> ThreadResumeResult:
        """Resume a thread from a serialized checkpoint."""
        cp_dict = (
            checkpoint.raw if isinstance(checkpoint, ThreadCheckpoint) else checkpoint
        )
        if isinstance(cp_dict, dict):
            cp_dict = cp_dict.get("value", cp_dict)
        res = await self._send_request(
            "thread/resume",
            {
                "threadId": thread_id,
                "checkpoint": cp_dict,
            },
        )
        return ThreadResumeResult.from_dict(res)

    # -------------------------------------------------------------------------
    # Turn Execution & Real-Time Control
    # -------------------------------------------------------------------------

    async def start_turn(
        self,
        prompt: str,
        mode: Literal["start", "start_if_idle"] = "start",
        thread_id: str | None = None,
        effort: Literal["low", "medium", "high"] | None = None,
        selected_skills: list[str] | None = None,
        workflow: dict[str, Any] | None = None,
        operation_id: str | None = None,
        operation_attempt: int | None = None,
        operation_group_id: str | None = None,
        execution_mode: str | None = None,
        group_sequence: int | None = None,
        operation_attempt_kind: str | None = None,
        turn_source: str | None = None,
    ) -> TurnSubmissionResult:
        """Submit a turn prompt to the App Server with optional reasoning effort ('low', 'medium', 'high')."""
        if mode not in ("start", "start_if_idle"):
            raise ValueError("mode must be start or start_if_idle")
        payload: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
            "input": {
                "mode": mode,
                "text": prompt,
            },
        }
        if selected_skills:
            payload["input"]["selectedSkills"] = list(
                dict.fromkeys(selected_skills[:8])
            )
        if workflow:
            payload["input"]["workflow"] = dict(workflow)
        if operation_id:
            payload["operationId"] = operation_id
        if operation_attempt is not None:
            payload["operationAttempt"] = operation_attempt
        if operation_attempt_kind is not None:
            if operation_attempt_kind not in {"initial", "retry", "follow_up"}:
                raise ValueError(
                    "operation_attempt_kind must be initial, retry, or follow_up"
                )
            payload["operationAttemptKind"] = operation_attempt_kind
        if operation_group_id:
            payload["operationGroupId"] = operation_group_id
        if execution_mode:
            payload["executionMode"] = execution_mode
        if group_sequence is not None:
            payload["groupSequence"] = group_sequence
        if turn_source is not None:
            if turn_source not in {"child_wakeup", "session_resume"}:
                raise ValueError("turn_source must be child_wakeup or session_resume")
            payload["turnSource"] = turn_source
        if effort is not None:
            if effort not in ("low", "medium", "high"):
                raise ValueError("effort must be low, medium, or high")
            payload["input"]["reasoningEffort"] = effort
        res = await self._send_request("turn/start", payload)
        return TurnSubmissionResult.from_dict(res)

    async def steer_turn(
        self,
        turn_id: str,
        text: str,
        thread_id: str | None = None,
        request_id: str | None = None,
    ) -> dict[str, Any]:
        """Steer an active turn with a corrective instruction."""
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
            "turnId": turn_id,
            "text": text,
        }
        if request_id is not None:
            params["requestId"] = request_id
        return await self._send_request("turn/steer", params)

    async def interrupt_turn(
        self,
        turn_id: str,
        thread_id: str | None = None,
    ) -> None:
        """Cooperatively cancel/interrupt an active turn."""
        await self._send_request(
            "turn/interrupt",
            {
                "threadId": thread_id or self._active_thread_id,
                "turnId": turn_id,
            },
        )

    async def session_control(
        self,
        action: str,
        *,
        request_id: str | None = None,
        thread_id: str | None = None,
    ) -> dict[str, Any]:
        """Read or transition the durable control state for one Session."""
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
            "action": action,
        }
        if request_id is not None:
            params["requestId"] = request_id
        result = await self._send_request("session/control", params)
        value = result.get("value", result) if isinstance(result, dict) else result
        return value if isinstance(value, dict) else {}

    async def child_task_action(
        self,
        thread_id: str,
        parent_thread_id: str,
        operation_id: str,
        attempt: int,
        action: str,
        *,
        report: str | None = None,
        report_id: str | None = None,
        prompt: str | None = None,
        request_id: str | None = None,
        turn_id: str | None = None,
        error: str | None = None,
        control_source: str | None = None,
    ) -> dict[str, Any]:
        """Persist one parent-authorized child-task action in its Session."""
        params: dict[str, Any] = {
            "threadId": thread_id,
            "parentThreadId": parent_thread_id,
            "operationId": operation_id,
            "attempt": attempt,
            "action": action,
        }
        if report is not None:
            params["report"] = report
        if report_id is not None:
            params["reportId"] = report_id
        if prompt is not None:
            params["prompt"] = prompt
        if request_id is not None:
            params["requestId"] = request_id
        if turn_id is not None:
            params["turnId"] = turn_id
        if error is not None:
            params["error"] = error
        if control_source is not None:
            params["controlSource"] = control_source
        result = await self._send_request("child/task", params)
        value = result.get("value", result) if isinstance(result, dict) else result
        return value if isinstance(value, dict) else {}

    async def read_turn(self, turn_id: str) -> TurnReadResult:
        """Read settled result and history of a turn."""
        return await self._read_turn(turn_id)

    async def resume_turn(
        self,
        turn_id: str,
        checkpoint_seq: int,
        request_id: str,
        thread_id: str | None = None,
    ) -> TurnSubmissionResult:
        """Explicitly continue one persisted Turn from its execution checkpoint."""
        if checkpoint_seq < 1:
            raise ValueError("checkpoint_seq must be positive")
        if not request_id or len(request_id.encode("utf-8")) > 128:
            raise ValueError("request_id must be non-empty and at most 128 bytes")
        result = await self._send_request(
            "turn/resume",
            {
                "threadId": thread_id or self._active_thread_id,
                "turnId": turn_id,
                "checkpointSeq": checkpoint_seq,
                "requestId": request_id,
            },
        )
        return TurnSubmissionResult.from_dict(result)

    async def reconcile_turn(
        self,
        turn_id: str,
        checkpoint_seq: int,
        tool_call_id: str,
        request_id: str,
        disposition: TurnReconcileDisposition | str,
        evidence_summary: str,
        *,
        result_status: Literal["completed", "failed"] | None = None,
        result_content: str | None = None,
        thread_id: str | None = None,
    ) -> TurnReconcileResult:
        """Record an operator decision for one uncertain tool call."""
        if checkpoint_seq < 1:
            raise ValueError("checkpoint_seq must be positive")
        if not request_id or len(request_id.encode("utf-8")) > 128:
            raise ValueError("request_id must be non-empty and at most 128 bytes")
        if not tool_call_id or len(tool_call_id.encode("utf-8")) > 128:
            raise ValueError("tool_call_id must be non-empty and at most 128 bytes")
        if not evidence_summary.strip() or len(evidence_summary.encode("utf-8")) > 1024:
            raise ValueError(
                "evidence_summary must be non-empty and at most 1024 bytes"
            )
        disposition_value = TurnReconcileDisposition(disposition).value
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
            "turnId": turn_id,
            "checkpointSeq": checkpoint_seq,
            "toolCallId": tool_call_id,
            "requestId": request_id,
            "disposition": disposition_value,
            "evidenceSummary": evidence_summary,
        }
        if disposition_value == TurnReconcileDisposition.COMPLETED.value:
            if result_status not in {"completed", "failed"} or result_content is None:
                raise ValueError(
                    "completed disposition requires a completed or failed result"
                )
            if len(result_content.encode("utf-8")) > 64 * 1024:
                raise ValueError("result_content must be at most 65536 bytes")
            params["result"] = {"status": result_status, "content": result_content}
        elif result_status is not None or result_content is not None:
            raise ValueError("not_executed disposition must not include a tool result")
        result = await self._send_request("turn/reconcile", params)
        return TurnReconcileResult.from_dict(result)

    async def _read_turn(
        self, turn_id: str, request_timeout: float | None = None
    ) -> TurnReadResult:
        res = await self._send_request(
            "turn/read", {"turnId": turn_id}, timeout=request_timeout
        )
        return TurnReadResult.from_dict(res)

    async def wait_for_turn(
        self,
        turn_id: str,
        timeout: float = 60.0,
        poll_interval: float = 0.5,
    ) -> TurnReadResult:
        """
        Wait/poll until a turn settles (completes, cancels, or fails),
        and return its TurnReadResult.
        """
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            remaining = deadline - loop.time()
            if remaining <= 0:
                raise TurnTimeoutError(
                    f"Turn {turn_id} did not complete within {timeout}s"
                )
            try:
                result = await self._read_turn(
                    turn_id, request_timeout=min(self.request_timeout, remaining)
                )
                if result.status != "in_progress":
                    return result
                remaining = deadline - loop.time()
                if remaining <= 0:
                    raise TurnTimeoutError(
                        f"Turn {turn_id} did not complete within {timeout}s"
                    )
                await asyncio.sleep(min(poll_interval, remaining))
            except AppServerRequestTimeoutError as err:
                remaining = deadline - loop.time()
                if remaining <= 0:
                    raise TurnTimeoutError(
                        f"Turn {turn_id} could not be observed within {timeout}s"
                    ) from err
                logger.warning(
                    "Transient App Server turn/read timeout for %s; retrying for %.1fs",
                    turn_id,
                    remaining,
                )
                await asyncio.sleep(min(poll_interval, remaining))

    wait_turn = wait_for_turn

    async def stream_turn(
        self,
        prompt: str,
        mode: Literal["start", "start_if_idle"] = "start",
        thread_id: str | None = None,
        effort: Literal["low", "medium", "high"] | None = None,
        selected_skills: list[str] | None = None,
        workflow: dict[str, Any] | None = None,
    ) -> AsyncIterator[dict[str, Any]]:
        """
        Convenience generator that starts a turn and yields event payloads in real-time
        until the turn finishes.

        :param prompt: User instruction or task prompt.
        :param mode: Turn mode ('start' or 'start_if_idle').
        :param thread_id: Conversation thread identifier.
        :param effort: Optional reasoning effort ('low', 'medium', 'high').
        """
        target_thread = thread_id or self._active_thread_id
        queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue(
            maxsize=STREAM_EVENT_QUEUE_LIMIT
        )
        self._event_queues.append(queue)
        self._event_queue_threads[queue] = target_thread
        self._event_queue_bytes[queue] = 0
        self._event_queue_large_bytes[queue] = 0

        try:
            start_kwargs: dict[str, Any] = {
                "mode": mode,
                "thread_id": target_thread,
            }
            if effort is not None:
                start_kwargs["effort"] = effort
            if selected_skills:
                start_kwargs["selected_skills"] = selected_skills
            if workflow:
                start_kwargs["workflow"] = workflow
            start_resp = await self.start_turn(prompt, **start_kwargs)
            yield {
                "type": "_turn_submission",
                "data": {
                    "status": start_resp.status,
                    "turn_id": start_resp.turn_id,
                    "reason": start_resp.reason,
                },
                "submission": start_resp,
            }

            if not start_resp.turn_id:
                return

            active_turn_id = start_resp.turn_id
            steered = False

            while True:
                envelope = await queue.get()
                self._release_stream_message(queue, envelope)
                if envelope.get("type") == "_stream_overflow":
                    raise StreamEventOverflowError(
                        target_thread, STREAM_EVENT_QUEUE_LIMIT
                    )
                if envelope.get("type") == "_client_error":
                    raise ServerProcessError(
                        envelope.get("message")
                        or "App Server connection closed before stream settlement"
                    )
                if envelope.get("type") == "approval":
                    approval = envelope.get("approval", {})
                    approval_thread = approval.get("threadId") or approval.get(
                        "thread_id"
                    )
                    if approval_thread and approval_thread != target_thread:
                        continue
                    yield envelope
                    continue
                if envelope.get("type") == "notification":
                    if (
                        envelope.get("method") in ("item/started", "item/completed")
                        and "typed_item_notification" not in envelope
                    ):
                        envelope = {
                            **envelope,
                            "typed_item_notification": ItemLifecycleNotification.from_dict(
                                envelope["method"], envelope.get("data", {})
                            ),
                        }
                    notification_data = envelope.get("data", {})
                    notification_thread = None
                    if isinstance(notification_data, dict):
                        notification_thread = notification_data.get(
                            "threadId"
                        ) or notification_data.get("thread_id")
                    if notification_thread and notification_thread != target_thread:
                        continue
                    yield envelope
                    continue
                if envelope.get("threadId") != target_thread:
                    continue
                turn_id = envelope.get("turnId")
                if active_turn_id and turn_id != active_turn_id:
                    if steered:
                        active_turn_id = turn_id
                        steered = False
                    else:
                        continue

                event_dict = envelope.get("event", {})
                sequence = envelope.get("sequence", 0)
                item_dicts = envelope.get("items", [])
                event_type = event_dict.get("type")
                if event_type == "run_failed" and active_turn_id:
                    # RunFailed is a diagnostic emitted before the App Server
                    # persists the authoritative turn settlement. Wait for
                    # that settlement so consumers can see the real provider
                    # or context-limit error and still receive turn_finished.
                    try:
                        settled = await self.wait_for_turn(active_turn_id)
                        event_dict = {
                            **event_dict,
                            "status": settled.status,
                            "stop_reason": settled.stop_reason or settled.status,
                            "steps": settled.steps,
                            "error": settled.error,
                        }
                    except Exception as err:  # noqa: BLE001
                        logger.warning(
                            "Failed to enrich run_failed for turn %s: %s",
                            active_turn_id,
                            err,
                        )
                typed_items = [ThreadItem.from_dict(item) for item in item_dicts]

                yield {
                    "type": "event",
                    "threadId": envelope.get("threadId"),
                    "turnId": turn_id,
                    "itemId": envelope.get("itemId"),
                    "turnSource": envelope.get("turnSource"),
                    "sequence": sequence,
                    "event": event_dict,
                    "items": item_dicts,
                    "typed_items": typed_items,
                    "typed_event": parse_event(event_dict),
                }

                # turn_finished carries the durable TurnStatus, while
                # run_finished carries the Core StopReason. Normalize both so
                # callers do not mistake a step-limited turn for completion.
                status = event_dict.get("status")
                stop_reason = event_dict.get("stop_reason") or status
                if event_type == "turn_finished":
                    if status == "steered" or stop_reason == "steered":
                        steered = True
                        continue
                    break
                elif event_type == "run_failed":
                    # The following turn_finished is the lifecycle boundary.
                    continue
        finally:
            self._discard_stream_queue(queue)
            self._event_queues.remove(queue)
            self._event_queue_threads.pop(queue, None)
            self._event_queue_bytes.pop(queue, None)
            self._event_queue_large_bytes.pop(queue, None)
            self._overflowed_event_queues.discard(queue)

    # -------------------------------------------------------------------------
    # Thread Settings, Goals, and Read-Only Workflow Projection
    # -------------------------------------------------------------------------

    def _cache_thread_settings(
        self, thread_id: str, settings: ThreadSettingsResult
    ) -> None:
        """Keep the local projection monotonic with the App Server revision."""
        current = self._thread_settings.get(thread_id)
        if current is not None:
            if settings.state_revision is None:
                return
            if (
                current.state_revision is not None
                and settings.state_revision < current.state_revision
            ):
                return
        self._thread_settings[thread_id] = settings

    async def get_workflow_state(self, thread_id: str | None = None) -> WorkflowState:
        """Get the read-only collaboration mode and active Thread Goal."""
        thread_id = thread_id or self._active_thread_id
        settings = self._thread_settings.get(thread_id)
        goal = await self.get_goal(thread_id=thread_id)
        mode = settings.collaboration_mode.mode if settings else "default"
        builtin_tools = (
            list(settings.builtin_tools)
            if settings is not None
            else list(DEFAULT_BUILTIN_TOOLS)
        )
        revisions = [
            revision
            for revision in (
                settings.state_revision if settings is not None else None,
                goal.state_revision,
            )
            if revision is not None
        ]
        state_revision = max(revisions) if revisions else None
        return WorkflowState(
            collaboration_mode=(
                settings.collaboration_mode
                if settings is not None
                else CollaborationMode()
            ),
            builtin_tools=builtin_tools,
            continuation_mode=(
                settings.continuation_mode if settings is not None else "manual"
            ),
            state_revision=state_revision,
            goal=goal.goal,
            raw={
                "value": {
                    "collaborationMode": {"mode": mode},
                    "builtinTools": builtin_tools,
                    "continuationMode": (
                        settings.continuation_mode if settings else "manual"
                    ),
                    "stateRevision": state_revision,
                    "goal": goal.goal.raw if goal.goal else None,
                }
            },
        )

    async def update_thread_settings(
        self,
        mode: CollaborationModeKind | None,
        builtin_tools: list[str] | None = None,
        thread_id: str | None = None,
        continuation_mode: str | None = None,
        model_selection: dict[str, str] | None | object = ...,
        reasoning_effort: str | None | object = ...,
        reasoning_selection: dict[str, Any] | None | object = ...,
    ) -> ThreadSettingsResult:
        """Update Thread collaboration mode and optional Builtin selection."""
        params: dict[str, Any] = {"threadId": thread_id or self._active_thread_id}
        if mode is not None:
            params["collaborationMode"] = {"mode": mode}
        if builtin_tools is not None:
            params["builtinTools"] = builtin_tools
        if continuation_mode is not None:
            if continuation_mode not in ("manual", "continuous"):
                raise ValueError("continuation_mode must be manual or continuous")
            params["continuationMode"] = continuation_mode
        if model_selection is not ...:
            params["modelSelection"] = model_selection
        if reasoning_effort is not ...:
            params["reasoningEffort"] = reasoning_effort
        if reasoning_selection is not ...:
            params["reasoningSelection"] = reasoning_selection
        res = await self._send_request(
            "thread/settings/update",
            params,
        )
        result = ThreadSettingsResult.from_dict(res)
        self._cache_thread_settings(params["threadId"], result)
        return result

    async def update_thread_model_settings(
        self,
        model_selection: dict[str, str] | None | object = ...,
        reasoning_effort: str | None | object = ...,
        thread_id: str | None = None,
        reasoning_selection: dict[str, Any] | None | object = ...,
    ) -> ThreadSettingsResult:
        """Update this Thread's model choice without changing its workflow mode."""
        return await self.update_thread_settings(
            None,
            thread_id=thread_id,
            model_selection=model_selection,
            reasoning_effort=reasoning_effort,
            reasoning_selection=reasoning_selection,
        )

    async def get_thread_model_settings(
        self, thread_id: str | None = None
    ) -> dict[str, Any]:
        """Read the persisted Thread model reference and reasoning effort."""
        return await self._send_request(
            "thread/model-settings/get",
            {"threadId": thread_id or self._active_thread_id},
        )

    async def manage_model_catalog(
        self, operation: str, **params: Any
    ) -> dict[str, Any]:
        """Read or update Host-owned machine-wide provider and model settings."""
        if operation not in {
            "get",
            "test_connection",
            "upsert_provider",
            "delete_provider",
            "upsert_model",
            "delete_model",
            "set_defaults",
            "set_project_default",
        }:
            raise ValueError("unsupported model catalog operation")
        return await self._send_request(
            "model/catalog/manage",
            {"operation": operation, **params},
        )

    async def get_web_search_settings(self) -> dict[str, Any]:
        """Read the Host-owned machine-wide web search provider settings."""
        return await self._send_request("web/search/settings/read", {})

    async def update_web_search_settings(
        self,
        provider: str,
        *,
        deepseek_api_key: str | None = None,
        exa_api_key: str | None = None,
        kimi_api_key: str | None = None,
    ) -> dict[str, Any]:
        """Select one web search provider and optionally replace its credentials."""
        params: dict[str, Any] = {"provider": provider}
        for name, value in (
            ("deepseekApiKey", deepseek_api_key),
            ("exaApiKey", exa_api_key),
            ("kimiApiKey", kimi_api_key),
        ):
            if value is not None:
                params[name] = value
        return await self._send_request("web/search/settings/update", params)

    async def test_web_search(
        self, query: str, provider: str | None = None
    ) -> dict[str, Any]:
        """Run one bounded search using a selected or explicitly requested provider."""
        params: dict[str, Any] = {"query": query}
        if provider is not None:
            params["provider"] = provider
        return await self._send_request("web/search/test", params, timeout=40.0)

    async def test_model_connection(
        self, provider_id: str, model_id: str
    ) -> dict[str, Any]:
        """Run one bounded, tool-free provider request. The provider may charge for it."""
        return await self.manage_model_catalog(
            "test_connection", providerId=provider_id, modelId=model_id
        )

    async def set_collaboration_mode(
        self,
        mode: CollaborationModeKind,
        thread_id: str | None = None,
    ) -> ThreadSettingsResult:
        """Set the Thread collaboration mode to ``default`` or ``plan``."""
        return await self.update_thread_settings(mode, thread_id=thread_id)

    async def set_goal(
        self,
        objective: str | None = None,
        status: ThreadGoalStatus | None = None,
        token_budget: int | None = None,
        thread_id: str | None = None,
    ) -> ThreadGoalSetResult:
        """Set or replace the active Thread Goal."""
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
        }
        if objective is not None:
            params["objective"] = objective
        if status is not None:
            params["status"] = status
        if token_budget is not None:
            params["tokenBudget"] = token_budget
        res = await self._send_request(
            "thread/goal/set",
            params,
        )
        return ThreadGoalSetResult.from_dict(res)

    async def update_goal(
        self,
        objective: str,
        token_budget: int | None = None,
        thread_id: str | None = None,
    ) -> ThreadGoalSetResult:
        """Update a Thread Goal objective without changing its active state."""
        current = await self.get_goal(thread_id=thread_id)
        return await self.set_goal(
            objective=objective,
            status=(current.goal.status if current.goal else "active"),
            token_budget=token_budget
            if token_budget is not None
            else (current.goal.token_budget if current.goal else None),
            thread_id=thread_id,
        )

    async def get_goal(self, thread_id: str | None = None) -> ThreadGoalGetResult:
        """Read the active Goal owned by a Thread."""
        res = await self._send_request(
            "thread/goal/get",
            {"threadId": thread_id or self._active_thread_id},
        )
        return ThreadGoalGetResult.from_dict(res)

    async def clear_goal(self, thread_id: str | None = None) -> ThreadGoalClearResult:
        """Clear the active Goal and stop future automatic continuation."""
        res = await self._send_request(
            "thread/goal/clear",
            {"threadId": thread_id or self._active_thread_id},
        )
        return ThreadGoalClearResult.from_dict(res)

    # -------------------------------------------------------------------------
    # Session, World Governance & MCP Management
    # -------------------------------------------------------------------------

    async def get_session_info(self) -> SessionInfo | None:
        """Get active session storage and identifier metadata."""
        res = await self._send_request("session/info", {})
        val = res.get("value", res) if isinstance(res, dict) else res
        if not val:
            return None
        return SessionInfo.from_dict(res)

    async def read_context_manifest(
        self, thread_id: str | None = None
    ) -> SessionContextManifestResult:
        """Read the bounded Session-owned provenance manifest for a Thread."""
        result = await self._send_request(
            "session/context_manifest",
            {"threadId": thread_id or self._active_thread_id},
        )
        return SessionContextManifestResult.from_dict(result)

    async def read_notebook(
        self,
        thread_id: str | None = None,
        scope: str = "self",
    ) -> dict[str, Any]:
        """Read the current session notebook or its parent read-only snapshot."""
        if scope not in ("self", "parent"):
            raise ValueError("scope must be self or parent")
        res = await self._send_request(
            "session/notebook/read",
            {
                "threadId": thread_id or self._active_thread_id,
                "scope": scope,
            },
        )
        return res.get("value", res) if isinstance(res, dict) else res

    async def write_notebook(
        self,
        key: str,
        content: str,
        append: bool = False,
        importance: str = "normal",
        keywords: list[str] | None = None,
        evidence: list[dict[str, Any]] | None = None,
        thread_id: str | None = None,
    ) -> dict[str, Any]:
        """Upsert one entry in the current session notebook."""
        if importance not in ("critical", "high", "normal", "temporary"):
            raise ValueError("importance must be critical, high, normal, or temporary")
        params: dict[str, Any] = {
            "threadId": thread_id or self._active_thread_id,
            "key": key,
            "content": content,
            "append": append,
            "importance": importance,
        }
        if keywords is not None:
            params["keywords"] = keywords
        if evidence is not None:
            params["evidence"] = evidence
        res = await self._send_request(
            "session/notebook/write",
            params,
        )
        return res.get("value", res) if isinstance(res, dict) else res

    async def search_notebook(
        self,
        query: str,
        scope: str = "self",
        limit: int = 8,
        thread_id: str | None = None,
    ) -> dict[str, Any]:
        """Search the bounded Notebook projection returned by ``read_notebook``."""
        if scope not in ("self", "parent"):
            raise ValueError("scope must be self or parent")
        normalized = query.strip().lower()
        if not normalized:
            raise ValueError("notebook search query must not be empty")
        if len(query) > 128:
            raise ValueError("notebook search query must be at most 128 characters")
        notebook = await self.read_notebook(thread_id=thread_id, scope=scope)
        if not isinstance(notebook, dict):
            return notebook
        matches: list[dict[str, Any]] = []
        for entry in notebook.get("entries", []):
            if not isinstance(entry, dict):
                continue
            keywords = entry.get("keywords", [])
            searchable = [entry.get("key", ""), entry.get("content", "")]
            if isinstance(keywords, list):
                searchable.extend(keywords)
            evidence = entry.get("evidence", [])
            if isinstance(evidence, list):
                for item in evidence:
                    if isinstance(item, dict):
                        searchable.extend(
                            item.get(field, "")
                            for field in ("project", "commit", "path", "subject")
                        )
            if any(normalized in str(value).lower() for value in searchable):
                matches.append(entry)
        return {**notebook, "entries": matches[: max(1, min(limit, 8))]}

    async def forget_notebook(
        self,
        key: str,
        thread_id: str | None = None,
    ) -> dict[str, Any]:
        """Forget one entry from the current session notebook."""
        res = await self._send_request(
            "session/notebook/forget",
            {
                "threadId": thread_id or self._active_thread_id,
                "key": key,
            },
        )
        return res.get("value", res) if isinstance(res, dict) else res

    async def get_world_state(self) -> WorldStateResult:
        """Get snapshot of current workspace, sandbox, and execution policy."""
        res = await self._send_request("world/state", {})
        return WorldStateResult.from_dict(res)

    async def refresh_world(self) -> WorldRefreshResult:
        """Refresh workspace and detect newly installed commands or toolchains."""
        res = await self._send_request("world/refresh", {})
        return WorldRefreshResult.from_dict(res)

    async def set_world_execution(
        self,
        access: str = "project",
        policy: str = "interactive",
    ) -> WorldSetExecutionResult:
        """Set independent access and approval policy."""
        if access not in ("project", "full_machine"):
            raise ValueError("access must be project or full_machine")
        if policy not in ("interactive", "automatic", "trusted"):
            raise ValueError("policy must be interactive, automatic, or trusted")
        res = await self._send_request(
            "world/set_execution",
            {
                "access": access,
                "policy": policy,
            },
        )
        self._access_scope = access
        self._policy = policy
        self._world_execution_configured = True
        return WorldSetExecutionResult.from_dict(res)

    async def get_mcp_status(self) -> McpStatusResult:
        """Get status of registered MCP servers and tools."""
        res = await self._send_request("mcp/status", {})
        return McpStatusResult.from_dict(res)

    async def retry_mcp(self) -> McpRetryResult:
        """Retry connection to failed or inactive MCP servers."""
        res = await self._send_request("mcp/retry", {})
        return McpRetryResult.from_dict(res)


# Convenient alias matching Codex convention
AsyncMiniAgentClient = MiniAgentClient
