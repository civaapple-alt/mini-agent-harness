# ADR: Python SDK Architecture & App Server Integration (Codex-Aligned)

* **Date**: 2026-08-31
* **Status**: Implemented
* **Scope**: SDK / Client Layer (`mini-agent-web/sdk/python`, `mini-agent`)
* **Context**: Aligning client integration architecture with OpenAI Codex (`sdk/python`) and providing a reusable, strongly typed Python SDK for `mini-agent-app-server`.

---

## 1. Context & Motivation

In OpenAI Codex, client integration is decoupled into standard official SDKs (e.g., `codex/sdk/python` packaging `openai-codex`), while the Rust backend exposes an App Server over JSON-RPC 2.0. 

Previously, `mini-agent-web` relied on ad-hoc, script-level JSON-RPC clients in individual demo folders. To achieve production readiness, maintainability, and clean separation of concerns:
1. A standard, standalone Python SDK package (`mini-agent`) is established in `sdk/python`.
2. Upper-layer consumers (Web frontends, Cookbooks, CLI automation, Benchmarking scripts) import and interact with the engine via the official SDK rather than reinventing IPC piping.
3. The SDK provides full typing coverage (PEP 561 `py.typed`), typed event models, automated `.env` discovery, and non-blocking streaming.

---

## 2. Architectural Design

```text
┌─────────────────────────────────────────────────────────────┐
│                 Consumer Applications                       │
│        (FastAPI Web UI / Streamlit / Cookbook / TUI)        │
└──────────────────────────────┬──────────────────────────────┘
                               │ import mini_agent
┌──────────────────────────────▼──────────────────────────────┐
│                  Official Python SDK (mini-agent)           │
│  ├── MiniAgentClient / AsyncMiniAgentClient (client.py)     │
│  ├── Typed Event Hierarchy (events.py, parse_event)         │
│  ├── Protocol Dataclasses (types.py, ThreadCheckpoint)      │
│  └── Error Hierarchy (errors.py, AppServerError)            │
└──────────────────────────────┬──────────────────────────────┘
                               │ Stdio JSON-RPC 2.0 (JSONL)
┌──────────────────────────────▼──────────────────────────────┐
│                  mini-agent-app-server.exe                  │
│    (Actor Control Plane, Revision CAS, State Management)    │
└──────────────────────────────┬──────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────┐
│                   Mini Agent Execution Core                 │
│         (Model inference, Bounded Tools, Compaction)        │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Key Components & Contracts

### 3.1 Package Structure (`sdk/python`)
* **`pyproject.toml`**: Hatchling-backed package declaring `mini-agent` with Python `>=3.10` requirement and zero mandatory external dependencies.
* **`mini_agent/client.py`**:
  * `MiniAgentClient`: Async context manager (`async with MiniAgentClient() as client`).
  * Process lifecycle management (auto-detects `mini-agent-app-server` on `PATH` or explicit binary path).
  * Background stdout JSONL reader task and non-blocking stderr logging loop.
  * `stream_turn()`: High-level async generator yielding real-time tokens and events until `turn_finished` / `run_failed`.
  * Security approval broker interceptor (`approval/request` $\rightarrow$ custom async handler $\rightarrow$ `approval/respond`).
  * Runtime steering (`turn/steer`) and cooperative cancellation (`turn/interrupt`).
* **`mini_agent/events.py`**:
  * Strongly-typed event dataclasses matching protocol internal tagging (`#[serde(tag = "type", rename_all = "snake_case")]`):
    * `TurnStartedEvent`, `RunStartedEvent`, `ModelStartedEvent`
    * `AssistantReasoningDeltaEvent`, `AssistantTextDeltaEvent`
    * `ModelRespondedEvent`, `ToolStartedEvent`, `ToolFinishedEvent`
    * `TurnFinishedEvent`, `RunFailedEvent`
  * `parse_event(dict) -> AgentEvent` factory.
* **`mini_agent/types.py`**:
  * Protocol data models: `TurnSubmissionResult`, `TurnReadResult`, `ThreadCheckpoint`, `ToolCall`, `ModelUsage`.
* **`mini_agent/errors.py`**:
  * Exception tree rooted at `MiniAgentError`, including `AppServerError` (with error code/message/data), `ProtocolVersionMismatchError`, and `ServerProcessError`.

---

## 4. Verification & Validation Evidence

1. **Static Analysis & Modern Typing**:
   * Verified with `ruff check .` with zero errors/warnings.
   * PEP 561 `py.typed` compliance and `collections.abc` / Python 3.10+ union syntax (`X | None`, `X | Y`).
2. **Subprocess & Streaming Validation**:
   * Executed `uv run python cookbook/python-demo/01_basic_turn.py`:
     * Subprocess initialized `interactive` profile.
     * Received token-by-token reasoning deltas and text deltas.
     * Settled in `completed` status with proper turn ID correlation.
3. **Workspace Line Budget**:
   * Zero Rust code added to `mini-codex`. Rust runtime line budget remains strictly within limits (20,000 runtime / 30,000 workspace).
