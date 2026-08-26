# Core Harness Boundary and Separation of Concerns

Status: implemented

## Context

Many agent frameworks accumulate premature abstractions: policy frameworks, storage backends, dependency injection, execution hooks, and complex scheduling lanes. This obscures what the model actually decides versus what framework magic imposes, making it hard to measure whether an architectural change helps or hinders the model.

## Decision

The project defines `agent = model + harness` with an uncompromising boundary between two crates:

1. **`mini-agent-core`**:
   - Owns the explicit agent run loop (`prepare -> model -> tool -> observer`).
   - Owns pure contracts for [`Model`](crates/mini-agent-core/src/model.rs), [`Tool`](crates/mini-agent-core/src/tool.rs), and [`Observer`](crates/mini-agent-core/src/event.rs).
   - Enforces hard limits, stop classification, and immutable observation event emitting.
   - Strictly contains **no** provider HTTP clients, filesystem access, process spawning, MCP protocol implementations, TUI frameworks, or persistence storage.

2. **`mini-agent-cli`**:
   - Acts as the host adapter and CLI edge.
   - Manages OpenAI API streaming adapters, workspace tool implementations (file I/O, subprocesses), MCP client connections, approval UI, and session storage.

3. **External Effect Boundary**:
   - Every external action has three explicit moments:
     ```text
     prepare intent -> perform uncertain effect -> settle outcome
     ```
   - Observers receive immutable events and cannot intercept or alter execution.

## Consequences

- The core loop is fully testable deterministically in memory without mocking complex storage or external networks.
- Adding a feature to core requires passing a four-part admission test (hypothesis, trace evidence, why edge cannot own it, complexity budget).
- Host-specific capabilities (like TTY approval, interactive terminal colors, and MCP protocols) remain at the edge without polluting portable execution contracts.
