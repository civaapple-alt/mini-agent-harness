"""Bounded, non-sensitive fields used by approval log records."""

from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

_PATCH_COUNT_RE = re.compile(r"(新增|修改|删除)\s+(\d+)\s+个文件")
_PATCH_COUNT_ORDER = ("新增", "修改", "删除")
_MAX_LOG_COUNT = 1_000_000


def approval_log_fields(params: Mapping[str, Any]) -> tuple[str, str]:
    """Return a tool name and a bounded summary-count string for logging.

    Shell and other free-form action summaries are never copied into logs.
    Only the known structured apply_patch counts are allowed through.
    """
    tool_name = str(params.get("toolName") or params.get("tool_name") or "unknown")
    if tool_name != "apply_patch":
        return tool_name, "redacted"

    action_summary = str(params.get("actionSummary") or "")
    counts_by_kind = {
        kind: min(int(count), _MAX_LOG_COUNT)
        for kind, count in _PATCH_COUNT_RE.findall(action_summary)
    }
    if not counts_by_kind:
        return tool_name, "redacted"
    return tool_name, " ".join(
        f"{kind}={counts_by_kind[kind]}"
        for kind in _PATCH_COUNT_ORDER
        if kind in counts_by_kind
    )
