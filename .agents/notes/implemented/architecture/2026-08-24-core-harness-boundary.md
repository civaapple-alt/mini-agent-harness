# Core Harness Boundary and Separation of Concerns

Status: implemented

## Context

Many agent frameworks accumulate premature abstractions: policy frameworks, storage backends, dependency injection, execution hooks, and complex scheduling lanes. This obscures what the model actually decides versus what framework magic imposes, making it hard to measure whether an architectural change helps or hinders the model.

## Decision

The project defines `agent = model + harness` with an uncompromising boundary
between a protocol crate, an execution-kernel crate, and host adapters:

1. **`mini-agent-protocol`**:
   - Owns the in-process contracts for [`Model`](../../../../crates/mini-agent-protocol/src/model.rs), [`Tool`](../../../../crates/mini-agent-protocol/src/tool.rs), and [`Observer`](../../../../crates/mini-agent-protocol/src/event.rs).
   - Defines portable messages, model requests/responses, tool calls, events,
     stop reasons, and bounded error values.
   - Contains no harness orchestration, provider client, filesystem access,
     process spawning, MCP implementation, or persistence storage.

2. **`mini-agent-core`**:
   - Owns the explicit execution run loop (`prepare -> model -> tool -> observer`).
   - Enforces context hard limits, compaction, step control, and cooperative
     `RunControl` steering boundaries.
   - Owns storage-neutral `SessionState` and its nested `Context`, including
     ordered messages and context revision tracking. Hosts restore and persist
     these values but do not move storage I/O into core.
   - Owns `ToolRegistry` as the in-process dispatch implementation.
   - Re-exports protocol types as a compatibility facade while callers migrate
     to the protocol crate directly.
   - Strictly contains **no** provider HTTP clients, filesystem access, process
     spawning, MCP protocol implementations, TUI frameworks, or persistence storage.

3. **`mini-agent-cli`**:
   - Acts as the host adapter and CLI edge.
   - Manages OpenAI API streaming adapters, workspace tool implementations (file I/O, subprocesses), MCP client connections, approval UI, and session storage.

4. **External Effect Boundary**:
   - Every external action has three explicit moments:
     ```text
     prepare intent -> perform uncertain effect -> settle outcome
     ```
   - Observers receive immutable events and cannot intercept or alter execution.

## Consequences

- The protocol crate can be reused by another host without importing the
  execution kernel; the core loop remains fully testable deterministically in
  memory without mocking complex storage or external networks.
- Adding a feature to core requires passing a four-part admission test (hypothesis, trace evidence, why edge cannot own it, complexity budget).
- Host-specific capabilities (like TTY approval, interactive terminal colors, and MCP protocols) remain at the edge without polluting portable execution contracts.
