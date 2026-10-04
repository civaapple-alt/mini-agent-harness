"""
Custom Exceptions for the Mini Agent Python SDK.
"""

from __future__ import annotations

from typing import Any

SESSION_FORK_CONFLICT_CODE = -32001


class MiniAgentError(Exception):
    """Base exception for all Mini Agent SDK errors."""


class AppServerError(MiniAgentError):
    """Raised when the App Server returns a JSON-RPC error response."""

    def __init__(self, code: int, message: str, data: Any = None):
        super().__init__(f"[{code}] {message} (data={data})")
        self.code = code
        self.message = message
        self.data = data


class ProtocolVersionMismatchError(MiniAgentError):
    """Raised when the App Server protocol version does not match expected version."""


class ServerProcessError(MiniAgentError):
    """Raised when the App Server subprocess fails to spawn or crashes unexpectedly."""


class AppServerRequestTimeoutError(ServerProcessError):
    """Raised when one JSON-RPC request exceeds the SDK transport timeout."""

    def __init__(self, method: str, timeout: float):
        super().__init__(f"App Server request '{method}' timed out after {timeout:g}s")
        self.method = method
        self.timeout = timeout


class TurnTimeoutError(MiniAgentError):
    """Raised when a turn execution exceeds the configured timeout."""


class StreamEventOverflowError(MiniAgentError):
    """Raised when a stream consumer falls behind the bounded event buffer."""

    def __init__(self, thread_id: str, limit: int):
        super().__init__(
            f"event stream for Thread {thread_id} exceeded its bounded buffer; "
            "reconcile with replay_events()"
        )
        self.thread_id = thread_id
        self.limit = limit
