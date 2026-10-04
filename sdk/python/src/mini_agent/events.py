"""
Event models and parsing utilities for Mini Agent Protocol event streams.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from mini_agent.types import (
    ContextByteBreakdown,
    ContextInjectionRecord,
    ModelTiming,
    ModelUsage,
    ToolCall,
    ToolOutcome,
)


@dataclass
class EventModel:
    """Common convenience API shared by parsed protocol events."""

    @property
    def event_type(self) -> str:
        """Return the wire event discriminator."""
        return self.type


@dataclass
class RunFailure:
    """Structured failure detail emitted by the App Server."""

    type: str
    detail: Any = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RunFailure:
        return cls(type=str(data.get("type", "unknown")), detail=data.get("detail"))


@dataclass
class TurnStartedEvent(EventModel):
    """Emitted when a turn starts execution."""

    mode: str
    prompt: str
    type: str = "turn_started"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnStartedEvent:
        return cls(mode=data.get("mode", "start"), prompt=data.get("prompt", ""))


@dataclass
class SkillsLoadedEvent(EventModel):
    """A Skill body started or finished loading for the current Turn."""

    skills: list[dict[str, Any]] = field(default_factory=list)
    phase: str = "loaded"
    activation: str | None = None
    type: str = "skills_loaded"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SkillsLoadedEvent:
        return cls(
            skills=data.get("skills", []),
            phase=data.get("phase", "loaded"),
            activation=data.get("activation"),
        )


@dataclass
class SkillsLoadFailedEvent(EventModel):
    """Bounded failure metadata for Skill activation or on-demand reads."""

    skills: list[str] = field(default_factory=list)
    reason_code: str = "unknown"
    activation: str | None = None
    type: str = "skills_load_failed"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SkillsLoadFailedEvent:
        return cls(
            skills=data.get("skills", []),
            reason_code=data.get("reason_code") or data.get("reasonCode", "unknown"),
            activation=data.get("activation"),
        )


@dataclass
class SkillGroupActivatedEvent(EventModel):
    """A workflow Skill Group activated for the current Turn."""

    group: str = ""
    source: str = ""
    type: str = "skill_group_activated"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> SkillGroupActivatedEvent:
        return cls(group=data.get("group", ""), source=data.get("source", ""))


@dataclass
class ContextInjectedEvent(EventModel):
    """Metadata describing Host-owned context sources added to a Session."""

    records: list[ContextInjectionRecord] = field(default_factory=list)
    type: str = "context_injected"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ContextInjectedEvent:
        return cls(
            records=[
                ContextInjectionRecord.from_dict(record)
                for record in data.get("records", [])
                if isinstance(record, dict)
            ]
        )


@dataclass
class RunStartedEvent(EventModel):
    """Emitted when an agent harness run loop starts."""

    prompt: str
    type: str = "run_started"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RunStartedEvent:
        return cls(prompt=data.get("prompt", ""))


@dataclass
class ModelStartedEvent(EventModel):
    """Emitted before a model inference step."""

    step: int
    type: str = "model_started"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ModelStartedEvent:
        return cls(step=data.get("step", 1))


@dataclass
class AssistantReasoningDeltaEvent(EventModel):
    """Streaming chunk of model's internal thinking/reasoning."""

    delta: str
    type: str = "assistant_reasoning_delta"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AssistantReasoningDeltaEvent:
        return cls(delta=data.get("delta", ""))


@dataclass
class AssistantTextDeltaEvent(EventModel):
    """Streaming chunk of model's response text."""

    delta: str
    type: str = "assistant_text_delta"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> AssistantTextDeltaEvent:
        return cls(delta=data.get("delta", ""))


@dataclass
class ModelRespondedEvent(EventModel):
    """Emitted when a full model response step is received."""

    reasoning: str
    text: str
    tool_calls: list[ToolCall] = field(default_factory=list)
    usage: ModelUsage | None = None
    model_timing: ModelTiming | None = None
    context_bytes: ContextByteBreakdown | None = None
    type: str = "model_responded"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ModelRespondedEvent:
        tool_calls = [ToolCall.from_dict(t) for t in data.get("tool_calls", [])]
        return cls(
            reasoning=data.get("reasoning", ""),
            text=data.get("text", ""),
            tool_calls=tool_calls,
            usage=(
                ModelUsage.from_dict(data["usage"])
                if isinstance(data.get("usage"), dict)
                else None
            ),
            model_timing=ModelTiming.from_dict(
                data.get("model_timing") or data.get("modelTiming")
            ),
            context_bytes=ContextByteBreakdown.from_dict(
                data.get("context_bytes") or data.get("contextBytes")
            ),
        )


@dataclass
class ToolStartedEvent(EventModel):
    """Emitted when a tool invocation begins."""

    call: ToolCall
    type: str = "tool_started"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolStartedEvent:
        return cls(call=ToolCall.from_dict(data.get("call", {})))


@dataclass
class ToolFinishedEvent(EventModel):
    """Emitted when a tool execution settles."""

    call_id: str
    name: str
    content: str
    is_error: bool
    truncated: bool
    outcome: ToolOutcome | None = None
    type: str = "tool_finished"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ToolFinishedEvent:
        raw_outcome = data.get("outcome")
        return cls(
            call_id=data.get("call_id") or data.get("callId", ""),
            name=data.get("name", ""),
            content=data.get("content", ""),
            is_error=data.get("is_error", False),
            truncated=data.get("truncated", False),
            outcome=raw_outcome if isinstance(raw_outcome, str) else None,
        )


@dataclass
class ContextCompactionStartedEvent(EventModel):
    """Emitted before the bounded context is compacted."""

    before_bytes: int
    type: str = "context_compaction_started"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ContextCompactionStartedEvent:
        return cls(before_bytes=data.get("before_bytes") or data.get("beforeBytes", 0))


@dataclass
class ContextCompactionFinishedEvent(EventModel):
    """Emitted after bounded context compaction completes."""

    before_bytes: int
    after_bytes: int
    usage: ModelUsage | None = None
    type: str = "context_compaction_finished"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ContextCompactionFinishedEvent:
        return cls(
            before_bytes=data.get("before_bytes") or data.get("beforeBytes", 0),
            after_bytes=data.get("after_bytes") or data.get("afterBytes", 0),
            usage=(
                ModelUsage.from_dict(data["usage"])
                if isinstance(data.get("usage"), dict)
                else None
            ),
        )


@dataclass
class RunFinishedEvent(EventModel):
    """Emitted when the model/tool run reaches a stop reason."""

    stop_reason: str
    steps: int
    type: str = "run_finished"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RunFinishedEvent:
        return cls(
            stop_reason=data.get("stop_reason") or data.get("stopReason", "unknown"),
            steps=data.get("steps", 0),
        )


@dataclass
class TurnFinishedEvent(EventModel):
    """Emitted when the turn settles with a final status."""

    status: str
    type: str = "turn_finished"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TurnFinishedEvent:
        return cls(status=data.get("status", "completed"))


@dataclass
class RunFailedEvent(EventModel):
    """Emitted when a fatal harness error occurs."""

    reason: RunFailure | str
    type: str = "run_failed"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> RunFailedEvent:
        reason = data.get("reason", "unknown error")
        if isinstance(reason, dict):
            reason = RunFailure.from_dict(reason)
        return cls(reason=reason)


@dataclass
class GenericEvent(EventModel):
    """Fallback representation for any unmodeled event type."""

    type: str
    data: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> GenericEvent:
        return cls(type=data.get("type", "unknown"), data=data)


AgentEvent = (
    TurnStartedEvent
    | SkillsLoadedEvent
    | SkillsLoadFailedEvent
    | SkillGroupActivatedEvent
    | ContextInjectedEvent
    | RunStartedEvent
    | ModelStartedEvent
    | AssistantReasoningDeltaEvent
    | AssistantTextDeltaEvent
    | ModelRespondedEvent
    | ToolStartedEvent
    | ToolFinishedEvent
    | ContextCompactionStartedEvent
    | ContextCompactionFinishedEvent
    | RunFinishedEvent
    | TurnFinishedEvent
    | RunFailedEvent
    | GenericEvent
)

_EVENT_TYPE_MAP = {
    "turn_started": TurnStartedEvent,
    "skills_loaded": SkillsLoadedEvent,
    "skills_load_failed": SkillsLoadFailedEvent,
    "skill_group_activated": SkillGroupActivatedEvent,
    "context_injected": ContextInjectedEvent,
    "run_started": RunStartedEvent,
    "model_started": ModelStartedEvent,
    "assistant_reasoning_delta": AssistantReasoningDeltaEvent,
    "assistant_text_delta": AssistantTextDeltaEvent,
    "model_responded": ModelRespondedEvent,
    "tool_started": ToolStartedEvent,
    "tool_finished": ToolFinishedEvent,
    "context_compaction_started": ContextCompactionStartedEvent,
    "context_compaction_finished": ContextCompactionFinishedEvent,
    "run_finished": RunFinishedEvent,
    "turn_finished": TurnFinishedEvent,
    "run_failed": RunFailedEvent,
}


def parse_event(data: dict[str, Any]) -> AgentEvent:
    """Parse raw dictionary into a typed AgentEvent instance."""
    event_type = data.get("type")
    parser = _EVENT_TYPE_MAP.get(event_type)
    if parser:
        return parser.from_dict(data)
    return GenericEvent.from_dict(data)
