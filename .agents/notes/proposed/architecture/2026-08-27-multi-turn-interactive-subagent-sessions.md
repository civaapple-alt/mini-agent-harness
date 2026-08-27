# Multi-turn Interactive Subagent Sessions via Durable Resumption and ACP Protocol

Status: proposed

## 1. Context & Motivation

Phase 1 established single-turn subagent delegation via the `spawn_agent` tool ([Subprocess CLI Execution](.agents/notes/implemented/architecture/2026-08-27-subprocess-cli-and-acp-subagent-execution.md)), allowing parent agents to dispatch isolated child tasks to `mini-agent ask "<prompt>" --json`.

However, advanced collaborative workflows require **multi-turn interactions**:
1. **Iterative Refinement**: A parent agent reviews partial findings from a child agent and issues follow-up queries or corrections without re-explaining the entire workspace context.
2. **Interactive Debugging & Verification**: A child test runner agent reports a failing test suite; the parent sends patch suggestions and asks the child to re-verify in the same context.
3. **Multi-Agent Deliberation**: Multiple specialized subagents exchange structured critiques over several turns before converging on a final architecture.

This RFC proposes two complementary architectural approaches for multi-turn subagent execution:
- **Phase 2A (Stateless Session-Backed Multi-Turn)**: Leveraging durable disk checkpoints (`mini-agent resume <session_id>`) for 0 MB idle memory and crash-resilient multi-turn interactions.
- **Phase 2B (Streaming ACP Daemon Mode)**: Leveraging Agent Client Protocol (ACP) over stdio JSON-RPC for real-time token streaming and mid-turn interrupts.

---

## 2. Architecture: Stateless Session-Backed Multi-Turn (Phase 2A)

```mermaid
sequenceDiagram
    autonumber
    actor User as User / Developer
    participant Parent as Parent Agent (/root)
    participant Tool as Subagent Tool Suite
    participant Store as Session Store (.agents/sessions/)
    participant Child as Subagent Process (mini-agent)

    User->>Parent: "Review auth and fix issues"
    Note over Parent: Generates subagent session_id: sub-auth-rev

    Parent->>Tool: spawn_agent(task_name="auth_reviewer", message="Audit login flow", persist=true)
    Tool->>Child: Spawns: mini-agent ask "Audit login flow" --session-id sub-auth-rev --persist --json
    Child->>Store: Writes turn 1 checkpoint to sub-auth-rev/session.jsonl
    Child-->>Tool: Returns: { session_id: "sub-auth-rev", output: "Found missing CSRF token" }
    Note over Child: Process exits immediately; OS reclaims 100% memory
    Tool-->>Parent: "Subagent 'auth_reviewer' [sub-auth-rev] completed: Found missing CSRF token"

    Parent->>Parent: Decides to ask for a code fix suggestion
    Parent->>Tool: send_subagent_message(session_id="sub-auth-rev", message="Provide a patch for CSRF")
    Tool->>Child: Spawns: mini-agent resume sub-auth-rev "Provide a patch for CSRF" --json --auto
    Child->>Store: Loads history from sub-auth-rev/session.jsonl, executes turn 2, appends checkpoint
    Child-->>Tool: Returns: { session_id: "sub-auth-rev", output: "Here is the patch diff..." }
    Note over Child: Process exits immediately
    Tool-->>Parent: "Subagent [sub-auth-rev] completed turn 2: Here is the patch diff..."
```

### 2.1. Key Principles of Session-Backed Multi-Turn

1. **Zero Idle Memory Consumption**:
   Between collaborative turns, the child process exits completely. No background daemon or idle worker thread consumes memory or OS process handles.
2. **Disaster Resilience & Restart Safety**:
   If the parent harness or host system crashes, the subagent's full conversation history remains persisted in `.agents/sessions/<session_id>/session.jsonl`. Resumption can occur at any time.
3. **Minimal Code Surface**:
   Reuses existing `session::save_checkpoint` and `session::load_latest_checkpoint` infrastructure. Requires adding only a lightweight `send_subagent_message` tool (~60 lines) in `crates/mini-agent-cli/src/subagent.rs`.

### 2.2. Implicit Persistence Encapsulation

A key design nuance is the distinction between human CLI defaults and agent-to-agent subagent delegation:

| Execution Context | Default Persistence | Rationale & Mechanical Behavior |
| :--- | :--- | :--- |
| **Standard User CLI (`mini-agent ask "..."`)** | **Ephemeral (No disk writes)** | Keeps developer environment clean and avoids polluting `.agents/sessions/` with transient one-off shell queries. |
| **Interactive Subagent (`spawn_agent` with `persist=true`)** | **Implicitly Persistent** | The `spawn_agent` tool automatically generates a deterministic `session_id` (`sub-<timestamp>-<task_name>`) and passes `--persist --session-id <id>` under the hood. The parent model only tracks the returned `session_id`. |
| **One-Off Subagent (`spawn_agent` with `persist=false`)** | **Ephemeral** | For fire-and-forget lookups or isolated grep passes, disk serialization is skipped entirely. |

---

## 3. Case Study: Orchestrator Skill Validation (`code-review` Multi-Turn Workflows)

Orchestrator skills like `.codex/skills/code-review/SKILL.md` coordinate multiple specialized subagents (`breaking-changes`, `change-size`, `context`, `testing`). In complex code reviews, the orchestrator often needs **multi-turn interactive clarification** with specific reviewers.

```mermaid
sequenceDiagram
    autonumber
    actor User as User / Developer
    participant Root as Root Orchestrator (/root)
    participant Engine as Subagent Subprocess Runner
    participant B as Subagent: breaking-changes (sub-rev-break)
    participant T as Subagent: testing (sub-rev-test)

    User->>Root: Run code-review skill
    Note over Root: Reads code-review/SKILL.md

    Root->>Engine: spawn_agent("breaking-changes", message="Audit PR integration surfaces", persist=true)
    Engine->>B: Spawns child CLI with --persist (writes turn 1 checkpoint)
    B-->>Root: Returns initial finding + session_id "sub-rev-break" (Flags CLI argument change)

    Root->>Engine: spawn_agent("testing", message="Audit test coverage", persist=true)
    Engine->>T: Spawns child CLI with --persist (writes turn 1 checkpoint)
    T-->>Root: Returns initial finding + session_id "sub-rev-test" (Flags missing unit test)

    Note over Root: Root inspects findings; decides to follow up ONLY with breaking-changes reviewer
    Root->>Engine: send_subagent_message(session_id="sub-rev-break", message="Would adding a backward-compatible alias resolve the CLI break?")
    Engine->>B: Spawns: mini-agent resume sub-rev-break "..." (loads turn 1, executes turn 2)
    B-->>Root: Returns turn 2 verdict: "Yes, alias '--auto' for '--auto-approve' maintains full compatibility."

    Note over Root: Root aggregates findings across all subagent sessions
    Root->>User: Emits consolidated Markdown review report with actionable resolution paths
```

### 3.1. Capability Mapping for Multi-Turn Orchestrator Skills

| Orchestrator Skill Requirement | Multi-Turn Subagent Mechanism | System & Operational Benefit |
| :--- | :--- | :--- |
| **Selective Turn Refinement** | `send_subagent_message(session_id, ...)` | Orchestrator only re-engages the reviewer that flagged an issue (e.g. `breaking-changes`), without re-running unrelated reviewers. |
| **Preserved Analysis State** | Durable checkpoint resume (`session.jsonl`) | Child reviewer retains its exact code diff reasoning and file inspection cache from turn 1. |
| **Zero Idle Overhead** | Process exits between turns | While Root is thinking or querying reviewer B, reviewer T consumes 0 MB RAM and 0 CPU cycles. |
| **High Reasoning Isolation** | Child runs with `--reasoning-effort xhigh` | Deep multi-turn debate happens entirely in child context, keeping Root context clean and concise. |

---

## 4. Architecture: Streaming ACP Daemon Mode (Phase 2B)

For advanced IDE or GUI use cases requiring live token streaming (`AssistantTextDelta`) and real-time interruption:

```mermaid
graph LR
    Parent["Parent Agent / IDE Host"] <-->|Stdio JSON-RPC 2.0 (ACP)| Daemon["mini-agent app-server --acp"]
    Daemon <-->|Event Stream| LLM["Responses API / Provider"]
```

### 4.1. Stdio ACP Framing Protocol

The parent communicates with `mini-agent app-server` over standard Content-Length framed JSON-RPC 2.0 messages:
- `subagent/initialize`: Initializes workspace context and security preset.
- `subagent/sendTurn`: Sends follow-up message to the active session.
- `subagent/interrupt`: Cancels the in-flight model turn without killing the daemon.
- `notifications/streamDelta`: Streams real-time assistant text deltas and tool invocation events back to the parent.

---

## 5. Comparative Evaluation

| Dimension | Phase 2A: Session-Backed Resumption | Phase 2B: Stdio ACP Daemon |
| :--- | :--- | :--- |
| **Idle Resource Usage** | **0 MB RAM, 0 background processes** | ~15–25 MB RAM per active daemon |
| **Host Crash Safety** | **100% durable on disk; resume anytime** | Requires reconnection & reload from disk |
| **Mid-Turn Streaming** | Turn-level output on completion | Real-time token streaming (`text_delta`) |
| **Turn Latency** | ~10–25 ms cold boot per turn | 0 ms process boot (instant turn start) |
| **Implementation Complexity** | **Low (< 80 lines in CLI, 0 in Core)** | Moderate (+250~350 lines in CLI) |
| **Multi-Turn Context Cap** | Governed by compaction (`compact_context`) | Governed by compaction (`compact_context`) |

---

## 6. Tool Suite Specifications

```json
{
  "spawn_agent": {
    "description": "Spawn an isolated subagent child process to perform a dedicated subtask. Returns the subagent's session_id for future follow-up interactions.",
    "parameters": {
      "task_name": { "type": "string", "description": "Descriptive task identifier (e.g. 'auth_reviewer')" },
      "message": { "type": "string", "description": "Initial task prompt and instructions" },
      "persist": { "type": "boolean", "description": "Whether to persist session for multi-turn follow-ups (default: true)", "default": true },
      "model": { "type": "string", "description": "Optional model override" },
      "timeout_seconds": { "type": "integer", "description": "Turn timeout in seconds (default: 120, max: 600)" }
    },
    "required": ["task_name", "message"]
  },
  "send_subagent_message": {
    "description": "Send a follow-up instruction or clarification to an existing subagent session.",
    "parameters": {
      "session_id": { "type": "string", "description": "Durable session identifier returned by spawn_agent" },
      "message": { "type": "string", "description": "Follow-up message or instructions" },
      "timeout_seconds": { "type": "integer", "description": "Turn timeout in seconds (default: 120, max: 600)" }
    },
    "required": ["session_id", "message"]
  },
  "list_subagents": {
    "description": "List all active and recent subagent sessions in the workspace.",
    "parameters": {}
  }
}
```

---

## 7. Security, Isolation & Limits

1. **Session ID Containment**:
   All subagent session IDs conform to `sub-[a-zA-Z0-9_-]{8,32}` and are strictly scoped within `.agents/sessions/` (preventing path traversal).
2. **Max Multi-Turn Depth**:
   A subagent cannot recursively spawn further subagents beyond depth 3 (`max_agent_depth = 3`).
3. **Turn Compaction**:
   Subagent sessions automatically inherit `mini-agent-core` prefix compaction, preventing multi-turn context explosion.

---

## 8. Acceptance Criteria

1. **Multi-Turn Continuity**:
   - Spawning a subagent with `persist=true` writes `.agents/sessions/<session_id>/session.jsonl`.
   - `send_subagent_message` successfully resumes the session, preserving memory of turn 1 and producing coherent turn 2 responses.
2. **Crash & Restart Recovery**:
   - A subagent session resumed after parent process restart continues cleanly without data loss.
3. **Line Budget**:
   - Phase 2A implementation requires $< 100$ lines in `crates/mini-agent-cli/src/subagent.rs` with 0 lines added to `mini-agent-core`.