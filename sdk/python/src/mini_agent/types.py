"""
Type definitions and dataclasses for Mini Agent Protocol objects.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Literal

TurnStatus = Literal[
    "in_progress",
    "completed",
    "step_limit",
    "steered",
    "cancelled",
    "failed",
]

ThreadStatus = Literal[
    "idle",
    "running",
    "awaiting_input",
    "failed",
    "closed",
]

TurnInputMode = Literal[
    "start",
    "start_if_idle",
    "steer",
    "follow_up",
]

ThreadGoalStatus = Literal[
    "active",
    "paused",
    "blocked",
    "usageLimited",
    "budgetLimited",
    "complete",
]

ItemStatus = Literal["inProgress", "completed", "failed"]
KnownToolOutcome = Literal[
    "completed",
    "failed",
    "needs_approval",
    "deferred",
    "retryable",
    "cancelled",
]
# The wire is forward-compatible. Keep unknown server values instead of
# pretending they are one of the currently known outcomes.
ToolOutcome = str
ItemSortDirection = Literal["asc", "desc"]
RuntimePhase = Literal[
    "idle",
    "starting_turn",
    "model",
    "tool",
    "waiting_approval",
    "waiting_for_user_input",
    "stopping",
    "compaction",
    "persisting",
    "goal_verification",
    "goal_continuation_queued",
    "resuming",
    "completed",
    "failed",
]
BackgroundTaskState = Literal[
    "starting",
    "running",
    "stopping",
    "stopped",
    "failed",
    "lost",
]
ScheduledTaskState = Literal["scheduled", "ready", "cancelled"]

CollaborationModeKind = Literal["default", "plan"]
ContinuationMode = Literal["manual", "continuous"]
ApprovalPolicy = Literal["interactive", "automatic", "trusted"]


class ExecutionRecoveryStatus(str, Enum):
    """Known App Server execution recovery states."""

    RUNNING = "running"
    WAITING_FOR_CONTINUE = "waiting_for_continue"
    NEEDS_RECONCILIATION = "needs_reconciliation"
    SETTLED = "settled"
    UNKNOWN = "unknown"


class ExecutionRecoveryPhase(str, Enum):
    """Known phase recorded in an App Server execution checkpoint."""

    MODEL_REQUEST = "model_request"
    TOOL_BATCH = "tool_batch"
    UNKNOWN = "unknown"


class ExecutionRecoveryRecommendation(str, Enum):
    """Informational next step derived from a recovery status."""

    WAIT = "wait"
    RESUME = "resume"
    VERIFY_TOOL_RESULT = "verify_tool_result"
    NONE = "none"
    INSPECT = "inspect"


class TurnReconcileDisposition(str, Enum):
    """Explicit operator disposition for one uncertain tool invocation."""

    COMPLETED = "completed"
    NOT_EXECUTED = "not_executed"


@dataclass
class UncertainToolCall:
    """Identity and optional bounded arguments for an uncertain tool call."""

    tool_call_id: str
    name: str
    arguments: Any | None = None
    _arguments_present: bool = field(default=False, repr=False, compare=False)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UncertainToolCall:
        return cls(
            tool_call_id=str(data.get("toolCallId", data.get("tool_call_id", ""))),
            name=str(data.get("name", "")),
            arguments=data.get("arguments"),
            _arguments_present="arguments" in data,
        )


@dataclass
class UserQuestionOption:
    """One option presented by the App Server's interactive ``ask_user`` tool."""

    id: str
    label: str
    description: str | None = None
    recommended: bool = False
    recommendation_reason: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UserQuestionOption:
        return cls(
            id=str(data.get("id", "")),
            label=str(data.get("label", "")),
            description=data.get("description"),
            recommended=bool(data.get("recommended", False)),
            recommendation_reason=data.get("recommendationReason")
            or data.get("recommendation_reason"),
        )


@dataclass
class UserQuestion:
    """One bounded question in a sequential user interaction."""

    id: str
    prompt: str
    options: list[UserQuestionOption] = field(default_factory=list)
    allow_free_text: bool = False
    allow_skip: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UserQuestion:
        return cls(
            id=str(data.get("id", "")),
            prompt=str(data.get("prompt", "")),
            options=[
                UserQuestionOption.from_dict(option)
                for option in data.get("options", [])
                if isinstance(option, dict)
            ],
            allow_free_text=bool(
                data.get("allowFreeText", data.get("allow_free_text", False))
            ),
            allow_skip=bool(data.get("allowSkip", data.get("allow_skip", False))),
        )


@dataclass
class UserQuestionAnswer:
    """A structured answer sent for the current question."""

    type: str
    option_id: str | None = None
    text: str | None = None

    @classmethod
    def for_option(cls, option_id: str) -> UserQuestionAnswer:
        return cls(type="option", option_id=option_id)

    @classmethod
    def free_text(cls, text: str) -> UserQuestionAnswer:
        return cls(type="text", text=text)

    @classmethod
    def skipped(cls) -> UserQuestionAnswer:
        return cls(type="skipped")

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UserQuestionAnswer:
        return cls(
            type=str(data.get("type", "unknown")),
            option_id=data.get("optionId") or data.get("option_id"),
            text=data.get("text"),
        )

    def to_dict(self) -> dict[str, Any]:
        if self.type == "option" and self.option_id:
            return {"type": self.type, "optionId": self.option_id}
        if self.type == "text" and self.text is not None:
            return {"type": self.type, "text": self.text}
        if self.type == "skipped":
            return {"type": self.type}
        raise ValueError("answer must be an option, free-text, or skipped value")


@dataclass
class UserQuestionInteraction:
    """Persisted identity and progress for one question batch."""

    interaction_id: str
    thread_id: str
    turn_id: str
    call_id: str
    questions: list[UserQuestion] = field(default_factory=list)
    answers: list[UserQuestionAnswer | None] = field(default_factory=list)
    current_index: int = 0
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UserQuestionInteraction:
        return cls(
            interaction_id=data.get("interactionId") or data.get("interaction_id", ""),
            thread_id=data.get("threadId") or data.get("thread_id", ""),
            turn_id=data.get("turnId") or data.get("turn_id", ""),
            call_id=data.get("callId") or data.get("call_id", ""),
            questions=[
                UserQuestion.from_dict(question)
                for question in data.get("questions", [])
                if isinstance(question, dict)
            ],
            answers=[
                UserQuestionAnswer.from_dict(answer)
                if isinstance(answer, dict)
                else None
                for answer in data.get("answers", [])
            ],
            current_index=int(data.get("currentIndex", data.get("current_index", 0))),
            raw=data,
        )

    @property
    def current_question(self) -> UserQuestion | None:
        if 0 <= self.current_index < len(self.questions):
            return self.questions[self.current_index]
        return None


@dataclass
class UserQuestionNotification:
    """Typed projection of a user-question request or update notification."""

    phase: str
    interaction: UserQuestionInteraction

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UserQuestionNotification:
        interaction = data.get("interaction")
        return cls(
            phase=str(data.get("phase", "unknown")),
            interaction=UserQuestionInteraction.from_dict(
                interaction if isinstance(interaction, dict) else {}
            ),
        )


@dataclass
class ToolCall:
    """Represents a tool invocation requested by the model."""

    name: str
    arguments: Any = ""
    id: str = ""
    call_id: str = ""

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolCall:
        cid = data.get("id") or data.get("call_id") or data.get("callId", "")
        return cls(
            name=data.get("name", ""),
            arguments=data.get("arguments", ""),
            id=cid,
            call_id=cid,
        )


@dataclass
class ThreadItem:
    """Bounded client-facing projection of a Thread item."""

    type: str
    id: str = ""
    text: str = ""
    name: str = ""
    arguments: Any = None
    status: str | None = None
    outcome: ToolOutcome | None = None
    output: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadItem:
        raw_outcome = data.get("outcome")
        return cls(
            type=str(data.get("type", "unknown")),
            id=str(data.get("id", "")),
            text=str(data.get("text", "")),
            name=str(data.get("name", "")),
            arguments=data.get("arguments"),
            status=data.get("status"),
            outcome=raw_outcome if isinstance(raw_outcome, str) else None,
            output=data.get("output"),
        )


@dataclass
class ItemLifecycleNotification:
    """Typed projection of an ``item/started`` or ``item/completed`` notification."""

    method: Literal["item/started", "item/completed"]
    thread_id: str
    turn_id: str
    item: ThreadItem
    timestamp_ms: int
    raw: dict[str, Any] = field(default_factory=dict)
    turn_source: str | None = None

    @classmethod
    def from_dict(
        cls,
        method: Literal["item/started", "item/completed"],
        data: dict[str, Any],
    ) -> ItemLifecycleNotification:
        return cls(
            method=method,
            thread_id=data.get("threadId") or data.get("thread_id", ""),
            turn_id=data.get("turnId") or data.get("turn_id", ""),
            item=ThreadItem.from_dict(data.get("item", {})),
            timestamp_ms=(
                data.get("startedAtMs")
                if method == "item/started"
                else data.get("completedAtMs")
            )
            or (
                data.get("started_at_ms")
                if method == "item/started"
                else data.get("completed_at_ms", 0)
            ),
            raw=data,
            turn_source=data.get("turnSource") or data.get("turn_source"),
        )


@dataclass
class ThreadItemEntry:
    """One bounded item projection associated with its owning turn."""

    turn_id: str
    item: ThreadItem
    turn_source: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadItemEntry:
        return cls(
            turn_id=data.get("turnId") or data.get("turn_id", ""),
            item=ThreadItem.from_dict(data.get("item", {})),
            turn_source=data.get("turnSource") or data.get("turn_source"),
        )


@dataclass
class ThreadItemsListResult:
    """Cursor-bounded ThreadItem projection returned by ``thread/items/list``."""

    data: list[ThreadItemEntry] = field(default_factory=list)
    next_cursor: str | None = None
    backwards_cursor: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadItemsListResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        if not isinstance(val, dict):
            return cls(raw=data)
        return cls(
            data=[
                ThreadItemEntry.from_dict(entry)
                for entry in val.get("data", [])
                if isinstance(entry, dict)
            ],
            next_cursor=val.get("nextCursor") or val.get("next_cursor"),
            backwards_cursor=val.get("backwardsCursor") or val.get("backwards_cursor"),
            raw=data,
        )


@dataclass
class CollaborationMode:
    """Thread collaboration mode returned by the App Server."""

    mode: CollaborationModeKind = "default"

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> CollaborationMode:
        value = data or {}
        return cls(mode=value.get("mode", "default"))


@dataclass
class ModelUsage:
    """Token consumption statistics for a turn or step."""

    input_tokens: int = 0
    cached_input_tokens: int | None = None
    output_tokens: int = 0
    total_tokens: int = 0

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> ModelUsage:
        if not data:
            return cls()
        inp = data.get("input_tokens") or data.get("prompt_tokens", 0)
        cached = data.get("cached_input_tokens")
        if cached is None:
            cached = data.get("cache_read_tokens")
        out = data.get("output_tokens") or data.get("completion_tokens", 0)
        tot = data.get("total_tokens") or (inp + out)
        return cls(
            input_tokens=inp,
            cached_input_tokens=cached,
            output_tokens=out,
            total_tokens=tot,
        )


@dataclass
class ModelContextSnapshot:
    """Configured limits for the model that produced one usage report."""

    provider_id: str
    model_id: str
    context_window_tokens: int | None = None
    max_output_tokens: int | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> ModelContextSnapshot | None:
        if not isinstance(data, dict):
            return None
        selection = data.get("selection") or {}
        return cls(
            provider_id=selection.get("provider_id", selection.get("providerId", "")),
            model_id=selection.get("model_id", selection.get("modelId", "")),
            context_window_tokens=data.get(
                "context_window_tokens", data.get("contextWindowTokens")
            ),
            max_output_tokens=data.get(
                "max_output_tokens", data.get("maxOutputTokens")
            ),
        )


@dataclass
class ModelTiming:
    """Request-to-first-output and complete response latency in milliseconds."""

    ttft_ms: int | None = None
    response_ms: int | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> ModelTiming | None:
        if not isinstance(data, dict):
            return None
        return cls(
            ttft_ms=data.get("ttft_ms", data.get("ttftMs")),
            response_ms=data.get("response_ms", data.get("responseMs")),
        )


@dataclass
class ContextByteBreakdown:
    """Serialized model-input bytes by stable prompt/context category."""

    system_prompt: int = 0
    project_instructions: int = 0
    skills: int = 0
    workspace_state: int = 0
    conversation: int = 0
    tools: int = 0
    other: int = 0

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> ContextByteBreakdown | None:
        if not isinstance(data, dict):
            return None
        aliases = {
            "system_prompt": "systemPrompt",
            "project_instructions": "projectInstructions",
            "workspace_state": "workspaceState",
        }
        return cls(
            **{
                field_name: max(
                    0,
                    int(
                        data.get(
                            aliases.get(field_name, field_name), data.get(field_name, 0)
                        )
                        or 0
                    ),
                )
                for field_name in cls.__dataclass_fields__
            }
        )


@dataclass
class ContextInjectionRecord:
    """Metadata for a Host-injected context source; never contains source text."""

    id: str = ""
    kind: str = "other"
    source: str = ""
    scope: str = ""
    bytes: int = 0
    fingerprint: str = ""
    workspace: str | None = None
    path: str | None = None
    supersedes: str | None = None
    reused: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ContextInjectionRecord:
        return cls(
            id=str(data.get("id", "")),
            kind=str(data.get("kind", "other")),
            source=str(data.get("source", "")),
            scope=str(data.get("scope", "")),
            bytes=max(0, int(data.get("bytes", 0) or 0)),
            fingerprint=str(data.get("fingerprint", "")),
            workspace=data.get("workspace"),
            path=data.get("path"),
            supersedes=data.get("supersedes"),
            reused=bool(data.get("reused", False)),
        )


@dataclass
class SessionContextManifestEntry:
    """Session-owned provenance metadata; source text is never included."""

    thread_id: str
    turn_id: str | None
    source_id: str
    source_name: str
    kind: str
    version_fingerprint: str
    workspace: str | None
    path: str | None
    applies_to: str
    permission_basis: str
    injection_reason: str
    bytes: int
    reused: bool
    injected_at_ms: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SessionContextManifestEntry:
        return cls(
            thread_id=str(data.get("threadId", data.get("thread_id", ""))),
            turn_id=data.get("turnId", data.get("turn_id")),
            source_id=str(data.get("sourceId", data.get("source_id", ""))),
            source_name=str(data.get("sourceName", data.get("source_name", ""))),
            kind=str(data.get("kind", "other")),
            version_fingerprint=str(
                data.get("versionFingerprint", data.get("version_fingerprint", ""))
            ),
            workspace=data.get("workspace"),
            path=data.get("path"),
            applies_to=str(data.get("appliesTo", data.get("applies_to", ""))),
            permission_basis=str(
                data.get("permissionBasis", data.get("permission_basis", ""))
            ),
            injection_reason=str(
                data.get("injectionReason", data.get("injection_reason", ""))
            ),
            bytes=max(0, int(data.get("bytes", 0) or 0)),
            reused=bool(data.get("reused", False)),
            injected_at_ms=max(
                0, int(data.get("injectedAtMs", data.get("injected_at_ms", 0)) or 0)
            ),
        )


@dataclass
class SessionContextManifestResult:
    """Bounded Context Manifest projection restored from a Session."""

    data: list[SessionContextManifestEntry] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SessionContextManifestResult:
        value = data.get("value", data) if isinstance(data, dict) else {}
        if not isinstance(value, dict):
            return cls(raw=data if isinstance(data, dict) else {})
        return cls(
            data=[
                SessionContextManifestEntry.from_dict(item)
                for item in value.get("data", [])
                if isinstance(item, dict)
            ],
            raw=data if isinstance(data, dict) else {},
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "data": [
                {
                    "threadId": item.thread_id,
                    "turnId": item.turn_id,
                    "sourceId": item.source_id,
                    "sourceName": item.source_name,
                    "kind": item.kind,
                    "versionFingerprint": item.version_fingerprint,
                    "workspace": item.workspace,
                    "path": item.path,
                    "appliesTo": item.applies_to,
                    "permissionBasis": item.permission_basis,
                    "injectionReason": item.injection_reason,
                    "bytes": item.bytes,
                    "reused": item.reused,
                    "injectedAtMs": item.injected_at_ms,
                }
                for item in self.data
            ]
        }


@dataclass
class TurnSubmissionResult:
    """Result returned immediately when submitting a turn to the app server."""

    status: str
    turn_id: str | None = None
    reason: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnSubmissionResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        if not isinstance(val, dict):
            return cls(status="unknown", raw={"value": val})
        turn_id = (
            val.get("turn_id")
            or val.get("turnId")
            or (
                val.get("turn_id", {}).get("0")
                if isinstance(val.get("turn_id"), dict)
                else None
            )
        )
        return cls(
            status=val.get("status", "started"),
            turn_id=turn_id,
            reason=val.get("reason"),
            raw=data,
        )


@dataclass
class ExecutionRecoveryInfo:
    """Typed, bounded projection of an App Server execution checkpoint."""

    turn_id: str
    status: ExecutionRecoveryStatus
    phase: ExecutionRecoveryPhase
    checkpoint_seq: int | None = None
    last_heartbeat_ms: int | None = None
    last_progress_ms: int | None = None
    reason: str | None = None
    uncertain_tool_calls: list[UncertainToolCall] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict)

    @property
    def recommended_action(self) -> ExecutionRecoveryRecommendation:
        """Return an advisory only; this property never resumes or retries work."""
        return {
            ExecutionRecoveryStatus.RUNNING: ExecutionRecoveryRecommendation.WAIT,
            ExecutionRecoveryStatus.WAITING_FOR_CONTINUE: (
                ExecutionRecoveryRecommendation.RESUME
            ),
            ExecutionRecoveryStatus.NEEDS_RECONCILIATION: (
                ExecutionRecoveryRecommendation.VERIFY_TOOL_RESULT
            ),
            ExecutionRecoveryStatus.SETTLED: ExecutionRecoveryRecommendation.NONE,
            ExecutionRecoveryStatus.UNKNOWN: ExecutionRecoveryRecommendation.INSPECT,
        }.get(self.status, ExecutionRecoveryRecommendation.INSPECT)

    def to_dict(self) -> dict[str, Any]:
        """Return the original wire fields for Gateway JSON projections."""
        if self.raw:
            return dict(self.raw)
        payload: dict[str, Any] = {
            "turnId": self.turn_id,
            "status": self.status.value,
            "phase": self.phase.value,
        }
        if self.checkpoint_seq is not None:
            payload["checkpointSeq"] = self.checkpoint_seq
        if self.last_heartbeat_ms is not None:
            payload["lastHeartbeatMs"] = self.last_heartbeat_ms
        if self.last_progress_ms is not None:
            payload["lastProgressMs"] = self.last_progress_ms
        if self.reason is not None:
            payload["reason"] = self.reason
        if self.uncertain_tool_calls:
            calls = []
            for call in self.uncertain_tool_calls:
                item = {"toolCallId": call.tool_call_id, "name": call.name}
                if call._arguments_present:
                    item["arguments"] = call.arguments
                calls.append(item)
            payload["uncertainToolCalls"] = calls
        return payload

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ExecutionRecoveryInfo:
        payload = data if isinstance(data, dict) else {}
        status_value = payload.get("status")
        phase_value = payload.get("phase")
        try:
            status = ExecutionRecoveryStatus(status_value)
        except (TypeError, ValueError):
            status = ExecutionRecoveryStatus.UNKNOWN
        try:
            phase = ExecutionRecoveryPhase(phase_value)
        except (TypeError, ValueError):
            phase = ExecutionRecoveryPhase.UNKNOWN

        def optional_int(camel: str, snake: str) -> int | None:
            value = payload.get(camel, payload.get(snake))
            if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                return value
            return None

        reason = payload.get("reason")
        turn_id = payload.get("turnId", payload.get("turn_id", ""))
        return cls(
            turn_id=turn_id if isinstance(turn_id, str) else "",
            status=status,
            phase=phase,
            checkpoint_seq=optional_int("checkpointSeq", "checkpoint_seq"),
            last_heartbeat_ms=optional_int("lastHeartbeatMs", "last_heartbeat_ms"),
            last_progress_ms=optional_int("lastProgressMs", "last_progress_ms"),
            reason=reason if isinstance(reason, str) else None,
            uncertain_tool_calls=[
                UncertainToolCall.from_dict(item)
                for item in payload.get(
                    "uncertainToolCalls", payload.get("uncertain_tool_calls", [])
                )
                if isinstance(item, dict)
            ],
            raw=dict(payload),
        )


@dataclass
class TurnReconcileResult:
    """Acknowledgement for a durable App Server reconciliation decision."""

    turn_id: str
    checkpoint_seq: int
    status: str
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnReconcileResult:
        value = data.get("value", data) if isinstance(data, dict) else {}
        return cls(
            turn_id=str(value.get("turnId", value.get("turn_id", ""))),
            checkpoint_seq=int(
                value.get("checkpointSeq", value.get("checkpoint_seq", 0))
            ),
            status=str(value.get("status", "unknown")),
            raw=data if isinstance(data, dict) else {},
        )


@dataclass
class SteerRequestInfo:
    """Durable status of one idempotent steering request."""

    turn_id: str
    request_id: str
    text: str
    status: str
    reason: str | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SteerRequestInfo:
        return cls(
            turn_id=str(data.get("turnId", data.get("turn_id", ""))),
            request_id=str(data.get("requestId", data.get("request_id", ""))),
            text=str(data.get("text", "")),
            status=str(data.get("status", "unknown")),
            reason=data.get("reason"),
        )


@dataclass
class TurnReadResult:
    """Settled outcome and message history for a turn."""

    turn_id: str
    status: TurnStatus
    stop_reason: str | None = None
    final_text: str | None = None
    steps: int = 0
    messages: list[dict[str, Any]] = field(default_factory=list)
    items: list[ThreadItem] = field(default_factory=list)
    error: str | None = None
    recovery: ExecutionRecoveryInfo | None = None
    steer_requests: list[SteerRequestInfo] = field(default_factory=list)
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnReadResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            turn_id=val.get("turn_id") or val.get("turnId", ""),
            status=val.get("status", "completed"),
            stop_reason=val.get("stop_reason") or val.get("stopReason"),
            final_text=val.get("final_text") or val.get("finalText"),
            steps=val.get("steps", 0),
            messages=val.get("messages", []),
            items=[ThreadItem.from_dict(item) for item in val.get("items", [])],
            error=val.get("error"),
            recovery=(
                ExecutionRecoveryInfo.from_dict(val["recovery"])
                if isinstance(val.get("recovery"), dict)
                else None
            ),
            steer_requests=[
                SteerRequestInfo.from_dict(item)
                for item in val.get("steerRequests", val.get("steer_requests", []))
                if isinstance(item, dict)
            ],
            raw=data,
        )


@dataclass
class RuntimeStatus:
    """Live bounded App Server runtime snapshot."""

    phase: RuntimePhase
    thread_id: str
    turn_id: str | None = None
    operation_id: str | None = None
    checkpoint_seq: int | None = None
    state_revision: int = 0
    timestamp_ms: int = 0
    error: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RuntimeStatus:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            phase=val.get("phase", "idle"),
            thread_id=val.get("threadId") or val.get("thread_id", "default"),
            turn_id=val.get("turnId") or val.get("turn_id"),
            operation_id=val.get("operationId") or val.get("operation_id"),
            checkpoint_seq=(
                val.get("checkpointSeq")
                if "checkpointSeq" in val
                else val.get("checkpoint_seq")
            ),
            state_revision=val.get("stateRevision")
            if "stateRevision" in val
            else val.get("state_revision", 0),
            timestamp_ms=val.get("timestampMs")
            if "timestampMs" in val
            else val.get("timestamp_ms", 0),
            error=val.get("error"),
            raw=data,
        )


@dataclass
class BackgroundTask:
    """Bounded snapshot of a locally managed background Shell task."""

    task_id: str
    owner_thread_id: str
    state: BackgroundTaskState
    command_summary: str
    command_hash: str
    working_directory: str
    process_id: int | None = None
    started_at: int = 0
    stopped_at: int | None = None
    exit_code: int | None = None
    log_bytes: int = 0
    log_truncated: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> BackgroundTask:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            task_id=val.get("taskId") or val.get("task_id", ""),
            owner_thread_id=val.get("ownerThreadId") or val.get("owner_thread_id", ""),
            state=val.get("state", "lost"),
            command_summary=val.get("commandSummary") or val.get("command_summary", ""),
            command_hash=val.get("commandHash") or val.get("command_hash", ""),
            working_directory=val.get("workingDirectory")
            or val.get("working_directory", ""),
            process_id=val.get("processId")
            if "processId" in val
            else val.get("process_id"),
            started_at=val.get("startedAt", val.get("started_at", 0)),
            stopped_at=val.get("stoppedAt")
            if "stoppedAt" in val
            else val.get("stopped_at"),
            exit_code=val.get("exitCode")
            if "exitCode" in val
            else val.get("exit_code"),
            log_bytes=val.get("logBytes", val.get("log_bytes", 0)),
            log_truncated=bool(
                val.get("logTruncated", val.get("log_truncated", False))
            ),
            raw=data,
        )


@dataclass
class BackgroundTaskLogs:
    """Bounded tail of one background Shell task's combined output."""

    task_id: str
    text: str
    bytes: int = 0
    truncated: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> BackgroundTaskLogs:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            task_id=val.get("taskId") or val.get("task_id", ""),
            text=val.get("text", ""),
            bytes=val.get("bytes", 0),
            truncated=bool(val.get("truncated", False)),
            raw=data,
        )


@dataclass
class ScheduledTask:
    """A bounded delay marker; it does not wake a Thread or start a Turn."""

    task_id: str
    owner_thread_id: str
    state: ScheduledTaskState
    trigger_type: str
    summary: str
    created_at: int = 0
    due_at: int = 0
    ready_at: int | None = None
    cancelled_at: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ScheduledTask:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            task_id=val.get("taskId") or val.get("task_id", ""),
            owner_thread_id=val.get("ownerThreadId") or val.get("owner_thread_id", ""),
            state=val.get("state", "scheduled"),
            trigger_type=val.get("triggerType") or val.get("trigger_type", "delay"),
            summary=val.get("summary", ""),
            created_at=val.get("createdAt", val.get("created_at", 0)),
            due_at=val.get("dueAt", val.get("due_at", 0)),
            ready_at=val.get("readyAt") if "readyAt" in val else val.get("ready_at"),
            cancelled_at=val.get("cancelledAt")
            if "cancelledAt" in val
            else val.get("cancelled_at"),
            raw=data,
        )


@dataclass
class TurnEventsResult:
    """Bounded replay page of ordered ``turn/event`` notifications."""

    data: list[dict[str, Any]] = field(default_factory=list)
    next_cursor: int | None = None
    oldest_sequence: int | None = None
    has_gap: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnEventsResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        if not isinstance(val, dict):
            return cls(raw=data)
        return cls(
            data=[item for item in val.get("data", []) if isinstance(item, dict)],
            next_cursor=val.get("nextCursor")
            if "nextCursor" in val
            else val.get("next_cursor"),
            oldest_sequence=val.get("oldestSequence")
            if "oldestSequence" in val
            else val.get("oldest_sequence"),
            has_gap=bool(val.get("hasGap", val.get("has_gap", False))),
            raw=data,
        )


@dataclass
class ThreadCheckpoint:
    """Settled state of a thread."""

    thread_id: str
    status: ThreadStatus
    next_turn_number: int = 1
    messages: list[dict[str, Any]] = field(default_factory=list)
    context_revision: int = 0
    last_turn_id: str | None = None
    next_event_sequence: int = 1
    execution_recovery: ExecutionRecoveryInfo | None = None
    pending_user_question: dict[str, Any] | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadCheckpoint:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            thread_id=val.get("thread_id") or val.get("threadId", "default"),
            status=val.get("status", "idle"),
            next_turn_number=val.get("next_turn_number")
            or val.get("nextTurnNumber", 1),
            messages=val.get("session", {}).get("messages", [])
            or val.get("messages", []),
            context_revision=val.get("context_revision")
            or val.get("contextRevision", 0),
            last_turn_id=val.get("last_turn_id") or val.get("lastTurnId"),
            next_event_sequence=val.get("next_event_sequence")
            or val.get("nextEventSequence", 1),
            execution_recovery=(
                ExecutionRecoveryInfo.from_dict(
                    val.get("execution_recovery") or val.get("executionRecovery")
                )
                if isinstance(
                    val.get("execution_recovery") or val.get("executionRecovery"),
                    dict,
                )
                else None
            ),
            pending_user_question=(
                val.get("pending_user_question") or val.get("pendingUserQuestion")
                if isinstance(
                    val.get("pending_user_question") or val.get("pendingUserQuestion"),
                    dict,
                )
                else None
            ),
            raw=data,
        )


@dataclass
class ThreadListResult:
    """List of available threads."""

    data: list[str] = field(default_factory=list)
    next_cursor: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadListResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        threads = val.get("data", []) if isinstance(val, dict) else []
        cursor = (
            val.get("nextCursor") or val.get("next_cursor")
            if isinstance(val, dict)
            else None
        )
        return cls(data=threads, next_cursor=cursor, raw=data)


@dataclass
class ThreadForkResult:
    """Result of forking a thread."""

    thread_id: str
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadForkResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        tid = val.get("threadId") or val.get("thread_id", "")
        return cls(thread_id=tid, raw=data)


@dataclass
class SessionForkResult:
    """Result of creating an independent persisted Session fork."""

    session_id: str
    thread_id: str
    path: str = ""
    parent_session_id: str = ""
    parent_checkpoint_seq: int = 0
    session_bytes: int = 0
    context_before_bytes: int = 0
    context_after_bytes: int = 0
    compacted: bool = False
    method: str = "exact"
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SessionForkResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            session_id=val.get("sessionId") or val.get("session_id", ""),
            thread_id=val.get("threadId") or val.get("thread_id", ""),
            path=val.get("path", ""),
            parent_session_id=val.get("parentSessionId")
            or val.get("parent_session_id", ""),
            parent_checkpoint_seq=val.get("parentCheckpointSeq")
            or val.get("parent_checkpoint_seq", 0),
            session_bytes=val.get("sessionBytes") or val.get("session_bytes", 0),
            context_before_bytes=val.get("contextBeforeBytes")
            or val.get("context_before_bytes", 0),
            context_after_bytes=val.get("contextAfterBytes")
            or val.get("context_after_bytes", 0),
            compacted=bool(val.get("compacted", False)),
            method=val.get("method", "exact"),
            raw=data,
        )


@dataclass
class ThreadResumeResult:
    """Result of resuming a thread checkpoint."""

    thread_id: str
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadResumeResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        tid = val.get("threadId") or val.get("thread_id", "")
        return cls(thread_id=tid, raw=data)


@dataclass
class ThreadGoal:
    """Bounded public projection of a Thread-owned Goal."""

    thread_id: str
    objective: str
    status: ThreadGoalStatus
    token_budget: int | None = None
    tokens_used: int = 0
    time_used_seconds: int = 0
    created_at: int = 0
    updated_at: int = 0
    current_milestone: int = 0
    total_milestones: int = 0
    loop_count: int = 0
    verifier_model_selection: dict[str, str] | None = None
    last_verifier_score: int | None = None
    last_error: str | None = None
    verification_status: str = "idle"
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadGoal:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            thread_id=val.get("threadId") or val.get("thread_id", ""),
            objective=val.get("objective", ""),
            status=val.get("status", "active"),
            token_budget=val.get("tokenBudget")
            if "tokenBudget" in val
            else val.get("token_budget"),
            tokens_used=val.get("tokensUsed") or val.get("tokens_used", 0),
            time_used_seconds=val.get("timeUsedSeconds")
            or val.get("time_used_seconds", 0),
            created_at=val.get("createdAt") or val.get("created_at", 0),
            updated_at=val.get("updatedAt") or val.get("updated_at", 0),
            current_milestone=val.get("currentMilestone")
            or val.get("current_milestone", 0),
            total_milestones=val.get("totalMilestones")
            or val.get("total_milestones", 0),
            loop_count=val.get("loopCount") or val.get("loop_count", 0),
            verifier_model_selection=val.get("verifierModelSelection")
            if "verifierModelSelection" in val
            else val.get("verifier_model_selection"),
            last_verifier_score=val.get("lastVerifierScore")
            if "lastVerifierScore" in val
            else val.get("last_verifier_score"),
            last_error=val.get("lastError")
            if "lastError" in val
            else val.get("last_error"),
            verification_status=val.get("verificationStatus")
            or val.get("verification_status", "idle"),
            raw=data,
        )


DEFAULT_BUILTIN_TOOLS: list[str] = [
    "read_file",
    "apply_patch",
    "shell",
    "read_image",
    "scheduled_task",
]


@dataclass
class WorkflowState:
    """Read-only workflow projection using the Codex-shaped settings model."""

    collaboration_mode: CollaborationMode = field(default_factory=CollaborationMode)
    builtin_tools: list[str] = field(
        default_factory=lambda: list(DEFAULT_BUILTIN_TOOLS)
    )
    continuation_mode: ContinuationMode = "manual"
    goal: ThreadGoal | None = None
    state_revision: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorkflowState:
        val = data.get("value", data) if isinstance(data, dict) else data
        goal_data = val.get("goal") if isinstance(val, dict) else None
        goal = ThreadGoal.from_dict(goal_data) if goal_data else None
        mode_data = val.get("collaborationMode") or val.get("collaboration_mode")
        builtin_tools = (
            val["builtinTools"]
            if "builtinTools" in val
            else (
                val["builtin_tools"]
                if "builtin_tools" in val
                else list(DEFAULT_BUILTIN_TOOLS)
            )
        )
        return cls(
            collaboration_mode=CollaborationMode.from_dict(mode_data),
            builtin_tools=builtin_tools,
            continuation_mode=val.get("continuationMode")
            or val.get("continuation_mode", "manual"),
            goal=goal,
            state_revision=(
                data.get("stateRevision")
                if isinstance(data, dict) and "stateRevision" in data
                else data.get("state_revision")
                if isinstance(data, dict) and "state_revision" in data
                else None
            ),
            raw=data,
        )


@dataclass
class ThreadSettingsResult:
    """Result returned by ``thread/settings/update``."""

    collaboration_mode: CollaborationMode
    builtin_tools: list[str] = field(default_factory=list)
    continuation_mode: ContinuationMode = "manual"
    model_selection: dict[str, str] | None = None
    reasoning_selection: dict[str, Any] | None = None
    reasoning_effort: str | None = None
    state_revision: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadSettingsResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            collaboration_mode=CollaborationMode.from_dict(
                val.get("collaborationMode") or val.get("collaboration_mode")
            ),
            builtin_tools=(
                val["builtinTools"]
                if "builtinTools" in val
                else val.get("builtin_tools", [])
            ),
            continuation_mode=val.get("continuationMode")
            or val.get("continuation_mode", "manual"),
            model_selection=val.get("modelSelection") or val.get("model_selection"),
            reasoning_selection=val.get("reasoningSelection")
            if "reasoningSelection" in val
            else val.get("reasoning_selection"),
            reasoning_effort=val.get("reasoningEffort") or val.get("reasoning_effort"),
            state_revision=(
                data.get("stateRevision")
                if isinstance(data, dict) and "stateRevision" in data
                else data.get("state_revision")
                if isinstance(data, dict) and "state_revision" in data
                else None
            ),
            raw=data,
        )


@dataclass
class ThreadGoalSetResult:
    """Result returned by ``thread/goal/set``."""

    goal: ThreadGoal
    state_revision: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadGoalSetResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            goal=ThreadGoal.from_dict(val.get("goal", {})),
            state_revision=(
                data.get("stateRevision")
                if isinstance(data, dict) and "stateRevision" in data
                else data.get("state_revision")
                if isinstance(data, dict) and "state_revision" in data
                else None
            ),
            raw=data,
        )


@dataclass
class ThreadGoalGetResult:
    """Result returned by ``thread/goal/get``."""

    goal: ThreadGoal | None = None
    state_revision: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadGoalGetResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        goal_data = val.get("goal") if isinstance(val, dict) else None
        return cls(
            goal=ThreadGoal.from_dict(goal_data) if goal_data else None,
            state_revision=(
                data.get("stateRevision")
                if isinstance(data, dict) and "stateRevision" in data
                else data.get("state_revision")
                if isinstance(data, dict) and "state_revision" in data
                else None
            ),
            raw=data,
        )


@dataclass
class ThreadGoalClearResult:
    """Result returned by ``thread/goal/clear``."""

    cleared: bool
    state_revision: int | None = None
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ThreadGoalClearResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            cleared=bool(val.get("cleared", False)),
            state_revision=(
                data.get("stateRevision")
                if isinstance(data, dict) and "stateRevision" in data
                else data.get("state_revision")
                if isinstance(data, dict) and "state_revision" in data
                else None
            ),
            raw=data,
        )


@dataclass
class SessionInfo:
    """Metadata about active session and workspace storage."""

    session_id: str
    thread_id: str
    path: str
    resumed: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SessionInfo:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            session_id=val.get("sessionId") or val.get("session_id", ""),
            thread_id=val.get("threadId") or val.get("thread_id", ""),
            path=val.get("path", ""),
            resumed=val.get("resumed", False),
            raw=data,
        )


@dataclass
class WorldStateResult:
    """Snapshot of workspace, environment, and security preset."""

    context: str = ""
    lines: list[str] = field(default_factory=list)
    status: dict[str, Any] = field(default_factory=dict)
    workspace: str = ""
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorldStateResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            context=val.get("context", "") if isinstance(val, dict) else "",
            lines=val.get("lines", []) if isinstance(val, dict) else [],
            status=val.get("status", {}) if isinstance(val, dict) else {},
            workspace=val.get("workspace", "") if isinstance(val, dict) else "",
            raw=data,
        )


@dataclass
class WorldRefreshResult:
    """Result of refreshing workspace and environment detection."""

    changed: bool
    state: dict[str, Any] = field(default_factory=dict)
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorldRefreshResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            changed=val.get("changed", False) if isinstance(val, dict) else False,
            state=val.get("state", {}) if isinstance(val, dict) else {},
            raw=data,
        )


@dataclass
class WorldSetExecutionResult:
    """Result of modifying world execution policies."""

    changed: bool
    state: dict[str, Any] = field(default_factory=dict)
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> WorldSetExecutionResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            changed=val.get("changed", False) if isinstance(val, dict) else False,
            state=val.get("state", {}) if isinstance(val, dict) else {},
            raw=data,
        )


@dataclass
class McpStatusResult:
    """Registered MCP servers and tools status."""

    enabled_servers: list[str] = field(default_factory=list)
    inactive_servers: list[str] = field(default_factory=list)
    tool_count: int = 0
    retry_available: bool = False
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> McpStatusResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            enabled_servers=val.get("enabledServers") or val.get("enabled_servers", [])
            if isinstance(val, dict)
            else [],
            inactive_servers=val.get("inactiveServers")
            or val.get("inactive_servers", [])
            if isinstance(val, dict)
            else [],
            tool_count=val.get("toolCount") or val.get("tool_count", 0)
            if isinstance(val, dict)
            else 0,
            retry_available=val.get("retryAvailable")
            or val.get("retry_available", False)
            if isinstance(val, dict)
            else False,
            raw=data,
        )


@dataclass
class McpRetryResult:
    """Result of retrying MCP server initialization."""

    enabled_servers: list[str] = field(default_factory=list)
    inactive_servers: list[str] = field(default_factory=list)
    diagnostics: list[str] = field(default_factory=list)
    tool_count: int = 0
    raw: dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> McpRetryResult:
        val = data.get("value", data) if isinstance(data, dict) else data
        return cls(
            enabled_servers=val.get("enabledServers") or val.get("enabled_servers", [])
            if isinstance(val, dict)
            else [],
            inactive_servers=val.get("inactiveServers")
            or val.get("inactive_servers", [])
            if isinstance(val, dict)
            else [],
            diagnostics=val.get("diagnostics", []) if isinstance(val, dict) else [],
            tool_count=val.get("toolCount") or val.get("tool_count", 0)
            if isinstance(val, dict)
            else 0,
            raw=data,
        )
