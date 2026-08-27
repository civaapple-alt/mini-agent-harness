# Subagent Task Scheduling and Delegation Architecture

Status: proposed

## 1. Context & Motivation

As agent tasks grow in complexity (e.g., repository-wide auditing, concurrent sub-module refactoring, independent research pipelines), single-threaded sequential execution suffers from context pollution, token exhaustion, and lack of parallelism.

To study multi-agent orchestration within `mini-agent-harness`, we analyzed two production architectures:
1. **Codex (`codex-rs`)**: Multi-tool suite architecture (`spawn_agent`, `send_message`, `followup_task`, `wait_agent`, `interrupt_agent`, `list_agents`) with path-based addressing (`/root/task_name`), flexible context forking (`fork_turns: "none" | "all" | N`), and Tokio-event activity waiting.
2. **fx (`src/core/subagent`)**: Single polymorphic tool architecture (`subagent { command: create | inspect | message | relationship | configure | lifecycle }`) backed by a durable state machine on disk (`idle -> queued -> running -> awaiting_approval -> completed/failed`), message queue idempotency (`operation_id`), dynamic DAG hierarchy (`attach`/`detach`/`reparent`), and fine-grained `NotificationPolicy`.

---

## 2. Comparative Analysis: Codex vs. fx

| Dimension | Codex (`codex-rs`) | fx (`src/core/subagent`) | Trade-off Analysis |
| :--- | :--- | :--- | :--- |
| **Tool Interface** | Multi-tool suite (`spawn_agent`, `send_message`, `wait_agent`, etc.) | Single polymorphic tool (`subagent { command: ... }`) | Multi-tool is more intuitive for standard LLM function calling; polymorphic tool reduces tool definition count but increases argument schema complexity. |
| **Addressing & Naming** | POSIX path hierarchy (`/root`, `/root/worker`) | UUID / ID + human name | Path hierarchy makes parent-child lineage and routing visually explicit to both models and humans. |
| **Context Propagation** | Explicit `fork_turns` (`"none"`, `"all"`, or `N` turns) | Fresh prompt + queued message history | `fork_turns` provides precise control over token inheritance vs. context isolation. |
| **Synchronization** | Dedicated `wait_agent` listening on input queue activity | `inspect { wait: { until: "settled", timeout_ms } }` | Event-driven wait avoids busy polling loops and prevents model context explosion. |
| **State & Lifecycle** | Turn-based execution over shared session store | Explicit 9-state machine with disk control records | Turn-based execution fits lightweight harnesses; explicit state machine is ideal for long-running detached daemons. |
| **Notifications** | Structured analysis-channel injected messages (`NEW_TASK`, `FINAL_ANSWER`) | Configurable `NotificationPolicy` (`terminal`, `milestones`, `interval_ms`) | Structured messages fit LLM conversation flow; milestone policies allow fine-grained progress reporting. |
| **Security & Limits** | Shared workspace root, strict depth limit (`max_depth`), concurrency slots | Per-child `permission_mode` (`yolo`, `ask`, `plan`), cycle & graph depth validation | Hard depth and concurrency ceilings prevent runaway recursive spawns and token exhaustion. |

---

## 3. Harness Hypothesis

1. **Explicit Multi-Tool Delegation vs Single Context**:
   Delegating bounded subtasks to child agents with `fork_turns: "none"` or `fork_turns: N` produces higher verification pass rates on large tasks than packing entire project investigations into a single growing context window.
2. **Event-Driven Non-Busy Waiting**:
   Providing an asynchronous `wait_agent` tool that suspends the parent agent until child completion or timeout eliminates degenerative polling loops (`check_status` loops) and saves 30–70% of parent turn tokens.
3. **Strict Lineage & Hard Depth Limits**:
   Enforcing a hard ceiling on child depth ($D \le 3$) and active concurrent child slots ($C \le 4$) in core prevents runaway fork bombs while maintaining predictable resource utilization within the 20,000-line core budget.

---

## 4. Proposed Architecture for mini-agent-harness

```mermaid
graph TD
    Root["Parent Agent (/root)<br/>crates/mini-agent-cli"] -->|spawn_agent (task_name, fork_turns, message)| Manager["Subagent Manager / Host Adapter"]
    Manager -->|fork context & spawn| Child1["Child Agent (/root/researcher)"]
    Manager -->|fork context & spawn| Child2["Child Agent (/root/tester)"]
    Root -->|wait_agent (timeout_ms)| WaitEngine["Event Activity Waiter"]
    Child1 -->|Tool Execution & Completion| Channel["Activity Channel (mpsc)"]
    Child2 -->|Tool Execution & Completion| Channel
    Channel -->|Wakeup Notification| WaitEngine
    WaitEngine -->|Resolved Status & Output| Root
```

### 4.1. Core vs. Host Seam Boundaries

Following the `AGENTS.md` hard boundaries:
- `mini-agent-core` owns:
  - Portable tool specifications for collaboration primitives (`spawn_agent`, `send_message`, `wait_agent`, `list_agents`).
  - Limits on tree depth (`max_agent_depth`), child count per turn (`max_spawn_per_turn`), and message length.
  - Turn-atomic event emission (`Event::SubAgentStarted`, `Event::SubAgentFinished`, `Event::SubAgentMessage`).
- `mini-agent-cli` / Host Adapter owns:
  - Subagent lifecycle management, thread/task spawning (`tokio::spawn` or background harness instances).
  - Durable session and trace persistence under `.agents/sessions/` / `~/.mini-agent/sessions/`.
  - Workspace root propagation and approval controller delegation.

### 4.2. Tool Specifications

```json
{
  "spawn_agent": {
    "description": "Spawn a child agent to perform a delegated subtask in the workspace.",
    "parameters": {
      "task_name": { "type": "string", "description": "Unique identifier (e.g. 'code_search', 'refactor_core')" },
      "message": { "type": "string", "description": "Initial task prompt and instruction for the child agent" },
      "fork_turns": { "type": "string", "description": "'none' (fresh context, recommended), 'all' (inherit full history), or positive integer N (last N turns)", "default": "none" },
      "model": { "type": "string", "description": "Optional model override for the child agent" }
    },
    "required": ["task_name", "message"]
  },
  "send_message": {
    "description": "Send a message or follow-up instruction to an existing child agent.",
    "parameters": {
      "recipient": { "type": "string", "description": "Path or identifier of the child agent (e.g. '/root/code_search')" },
      "message": { "type": "string", "description": "Message content" }
    },
    "required": ["recipient", "message"]
  },
  "wait_agent": {
    "description": "Wait for one or all child agents to settle or complete their current tasks.",
    "parameters": {
      "timeout_ms": { "type": "integer", "description": "Maximum time in milliseconds to wait before returning current status (default 30000, max 300000)" }
    }
  },
  "list_agents": {
    "description": "List all active, completed, or failed child agents with their current status.",
    "parameters": {}
  }
}
```

### 4.3. Context Inheritance (`fork_turns`) Semantics

1. **`fork_turns: "none"` (Default & Recommended)**:
   Child receives only the system prompt + world state + the specific delegated task message. This guarantees complete token hygiene.
2. **`fork_turns: N`**:
   Child receives system prompt + world state + the most recent $N$ user/assistant turns + the delegated task message.
3. **`fork_turns: "all"`**:
   Child inherits the full parent history. Restricted to same model/reasoning configuration.

### 4.4. Security, Isolation, and Hard Ceilings

- **Depth Limit**: Maximum subagent recursion depth of 3 (`/root -> /root/a -> /root/a/b`). Attempts to spawn at depth > 3 return a deterministic tool error.
- **Concurrency Limit**: Up to 4 active concurrent child harnesses per session.
- **Workspace Isolation**: All children operate within the validated `Workspace` root with fail-closed security policies matching the active preset (`Default`, `FullMachine`, `Turbomode`).

---

## 5. Case Study: Orchestrator Skill Validation (`code-review`)

A primary validation benchmark for subagent task scheduling is the **Orchestrator Skill** pattern, exemplified by `.codex/skills/code-review/SKILL.md`.

### 5.1. Pattern Characteristics & Requirements

The orchestrator coordinates 4 specialized reviewer sub-skills in parallel:
- `code-review-breaking-changes`: External API, CLI parameter, and configuration compatibility.
- `code-review-change-size`: Change volume, PR decomposition, and atomicity.
- `code-review-context`: Context window inflation, instruction ceilings, and token efficiency.
- `code-review-testing`: Test adequacy, edge-case coverage, and mutation safety.

```mermaid
sequenceDiagram
    autonumber
    actor User as User / Developer
    participant Root as Root Orchestrator (/root)
    participant Engine as Subagent Runtime
    participant B as /root/breaking_changes
    participant S as /root/change_size
    participant C as /root/context
    participant T as /root/testing

    User->>Root: Run code-review skill
    Note over Root: Reads code-review/SKILL.md instructions

    Root->>Engine: spawn_agent("breaking_changes", fork_turns="none", reasoning_effort="xhigh", message="...")
    Root->>Engine: spawn_agent("change_size", fork_turns="none", reasoning_effort="xhigh", message="...")
    Root->>Engine: spawn_agent("context", fork_turns="none", reasoning_effort="xhigh", message="...")
    Root->>Engine: spawn_agent("testing", fork_turns="none", reasoning_effort="xhigh", message="...")

    par Concurrent Review Execution
        Engine->>B: Execute review (clean context)
        Engine->>S: Execute review (clean context)
        Engine->>C: Execute review (clean context)
        Engine->>T: Execute review (clean context)
    end

    Root->>Engine: wait_agent(timeout_ms=120000)
    Note over Root: Parent suspends in non-busy wait; zero token drain

    B-->>Engine: Returns breaking change issues
    S-->>Engine: Returns change size & split findings
    C-->>Engine: Returns context inflation findings
    T-->>Engine: Returns missing test cases

    Engine-->>Root: All reviewers settled; deliver aggregated findings
    Root->>User: Compile and output unified Markdown report with file:line links
```

### 5.2. Capability Mapping Matrix

| Orchestrator Skill Requirement | Proposed Subagent Mechanism | System Benefit |
| :--- | :--- | :--- |
| **Concurrent Dispatch** | `spawn_agent` + `max_concurrency_slots = 4` | Total review duration drops from $4 \times T$ to $\approx 1 \times T$. |
| **Reviewer Independence** | `fork_turns: "none"` (token hygiene) | Each reviewer only loads its specialized `SKILL.md` and `git diff`, preventing cross-reviewer prompt contamination and hallucinations. |
| **Deep Reasoning Override** | `spawn_agent(..., reasoning_effort="xhigh")` | Parent can remain on standard effort while reviewers utilize high reasoning tokens. |
| **Non-Busy Aggregation** | `wait_agent(timeout_ms)` | Parent avoids polling loops, saving 30%–70% parent turn tokens. |
| **Read-Only Safety** | Read-only inspection tools (`git`, `read_file`, `grep`) | Reviewers cannot inadvertently mutate workspace code. |
| **Recursive Protection** | `max_agent_depth = 3` | Prevents runaway recursive sub-agent spawns. |

---

## 6. Acceptance Criteria

1. **Deterministic Unit Testing**:
   - `spawn_agent` creates isolated child harness instances without mutating parent message history.
   - `fork_turns: "none"` produces fresh message queues containing only the initial instruction.
   - Spawning beyond `max_agent_depth` fails closed with explicit error messages.
2. **Asynchronous Non-Busy Waiting**:
   - `wait_agent` suspends parent execution until a child reports terminal outcome or timeout expires.
   - Advancing child output sends structured activity events to parent without polling.
3. **Trace & Session Reproducibility**:
   - Every child agent execution generates a linked JSONL session trace recording parent thread ID, turn ID, and role.
4. **Line Budget Compliance**:
   - Core changes $\le 400$ lines (within 20,000-line core limit).
   - Entire workspace remains $\le 30,000$ lines.

---

## 7. Non-Goals & Guardrails

- **No Distributed / Networked Agent Cluster**: Subagents execute as in-process asynchronous tasks or localized worker threads on the same machine.
- **No Complex Dynamic Graph Routing**: We adopt a clean tree hierarchy (`/root/subagent`) rather than arbitrary peer-to-peer cyclic graphs.
- **No Autonomous Runaway Loops**: Child agents remain bound by `max_steps` and parental lifecycle interrupts.