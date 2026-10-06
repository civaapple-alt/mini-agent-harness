"""Bounded in-memory aggregates for one App Server JSON-RPC connection."""

from __future__ import annotations

from collections import OrderedDict
from typing import Any

MAX_RPC_METHODS = 64
LATENCY_BUCKETS_MS = (1, 2, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000)
_OTHER_METHOD = "other"


def _new_method_metrics() -> dict[str, Any]:
    return {
        "requests": 0,
        "successes": 0,
        "errors": 0,
        "timeouts": 0,
        "request_bytes": 0,
        "response_bytes": 0,
        "notifications_sent": 0,
        "notifications_received": 0,
        "notification_bytes_sent": 0,
        "notification_bytes_received": 0,
        "serialization_ms": 0.0,
        "write_ms": 0.0,
        "decode_ms": 0.0,
        "dispatch_ms": 0.0,
        "handler_ms": 0.0,
        "latency_buckets": [0] * (len(LATENCY_BUCKETS_MS) + 1),
    }


class RpcMetrics:
    """Keep fixed-cardinality counters and approximate latency percentiles."""

    def __init__(self) -> None:
        self._methods: OrderedDict[str, dict[str, Any]] = OrderedDict()

    def _method(self, name: str) -> dict[str, Any]:
        method = name if isinstance(name, str) and name else _OTHER_METHOD
        if method in self._methods:
            return self._methods[method]
        if len(self._methods) >= MAX_RPC_METHODS - 1:
            method = _OTHER_METHOD
        if method not in self._methods:
            self._methods[method] = _new_method_metrics()
        return self._methods[method]

    def request_started(
        self, method: str, byte_count: int, serialization_ms: float
    ) -> None:
        metrics = self._method(method)
        metrics["requests"] += 1
        metrics["request_bytes"] += max(0, byte_count)
        metrics["serialization_ms"] += max(0.0, serialization_ms)

    def request_written(self, method: str, elapsed_ms: float) -> None:
        self._method(method)["write_ms"] += max(0.0, elapsed_ms)

    def request_finished(
        self, method: str, elapsed_ms: float, *, success: bool, timeout: bool = False
    ) -> None:
        metrics = self._method(method)
        metrics["successes" if success else "errors"] += 1
        if timeout:
            metrics["timeouts"] += 1
        latency = max(0.0, elapsed_ms)
        for index, upper_bound in enumerate(LATENCY_BUCKETS_MS):
            if latency <= upper_bound:
                metrics["latency_buckets"][index] += 1
                break
        else:
            metrics["latency_buckets"][-1] += 1

    def response_read(self, method: str, byte_count: int, decode_ms: float) -> None:
        metrics = self._method(method)
        metrics["response_bytes"] += max(0, byte_count)
        metrics["decode_ms"] += max(0.0, decode_ms)

    def notification_sent(
        self,
        method: str,
        byte_count: int,
        serialization_ms: float,
        write_ms: float,
    ) -> None:
        metrics = self._method(method)
        metrics["notifications_sent"] += 1
        metrics["notification_bytes_sent"] += max(0, byte_count)
        metrics["serialization_ms"] += max(0.0, serialization_ms)
        metrics["write_ms"] += max(0.0, write_ms)

    def notification_received(
        self, method: str, byte_count: int, decode_ms: float, dispatch_ms: float
    ) -> None:
        metrics = self._method(method)
        metrics["notifications_received"] += 1
        metrics["notification_bytes_received"] += max(0, byte_count)
        metrics["decode_ms"] += max(0.0, decode_ms)
        metrics["dispatch_ms"] += max(0.0, dispatch_ms)

    def notification_handler_finished(self, method: str, elapsed_ms: float) -> None:
        self._method(method)["handler_ms"] += max(0.0, elapsed_ms)

    @staticmethod
    def _percentile(metrics: dict[str, Any], percentile: float) -> float | None:
        buckets = metrics["latency_buckets"]
        count = sum(buckets)
        if not count:
            return None
        target = max(1, int(count * percentile + 0.999999))
        seen = 0
        for index, value in enumerate(buckets):
            seen += value
            if seen >= target:
                if index == len(LATENCY_BUCKETS_MS):
                    return None
                return float(LATENCY_BUCKETS_MS[index])
        return None

    def snapshot(self, pending_requests: int) -> dict[str, Any]:
        methods = {}
        for name, metrics in self._methods.items():
            methods[name] = {
                key: value for key, value in metrics.items() if key != "latency_buckets"
            }
            methods[name]["latency_buckets"] = list(metrics["latency_buckets"])
            methods[name]["latency_p50_ms"] = self._percentile(metrics, 0.50)
            methods[name]["latency_p95_ms"] = self._percentile(metrics, 0.95)
        return {
            "pending_requests": max(0, pending_requests),
            "latency_bucket_upper_bounds_ms": list(LATENCY_BUCKETS_MS),
            "methods": methods,
        }
