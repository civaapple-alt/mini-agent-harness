# mini-agent-harness Architecture and Design Decisions (Agent Notes)


This directory records architectural decision records (ADRs), technology selections, and trade-off analyses for the **mini-agent-harness** project.

---

## 1. Multi-Level Directory Semantics & Layout

Every Agent Note has two orthogonal axes encoded directly in its file path: `{lifecycle}/{class}/yyyy-mm-dd-topic-title.md`.

```text
.agents/notes/
├── proposed/                  # Proposals under review; not yet built (or partially built)
├── implemented/               # Decisions implemented and shipped in production
│   ├── architecture/          # Structural design, microkernel, capability seams, services
│   ├── feature/               # User- or model-facing agent capabilities
│   ├── simplification/        # Removing dead code/complexity without losing capabilities
│   ├── process/               # Development workflows, gates, tooling
│   ├── testing/               # Testing strategies and infrastructure
│   └── bug-fix/               # Postmortem fixes and architectural defect closure
├── rejected/                  # Proposals considered and declined (retained only if guardrail)
└── archived/                  # Completed historical snapshots that no longer guide future work
    └── experiments/
```

### Primary Axis: Lifecycle (`{lifecycle}/`)
- **`proposed/`**: Proposals under review prior to implementation. Free-form future tense (`## Proposal`, `## Acceptance criteria`, `## Risks`).
- **`implemented/`**: Shipped reality. Kept current with actual code (facts, paths, names). Uses present tense (`## Decision`, `## Consequences`).
- **`rejected/`**: Proposals declined during review. Carries `Status: rejected — <why, in one line>`. Kept only when its rationale prevents a tempting mistake.
- **`archived/`**: Shipped decisions that are 100% complete and whose rationale is superseded or unlikely to guide future changes. Permanently frozen.

### Secondary Axis: Class (`{class}/`)
| Class | Scope & Coverage |
| :--- | :--- |
| `architecture` | Structural decisions about shipped source code: microkernel, service matrices, capability seams, protocols. |
| `feature` | New user-facing or model-facing capabilities (e.g. new plugin, TUI component). |
| `simplification` | Removing code, behavior, or cognitive overhead without sacrificing capability. |
| `process` | Tooling, workflow, package management, and linting around the codebase. |
| `testing` | Test frameworks, mocking strategies, and regression gates. |
| `bug-fix` | Postmortem fixes for subtle architectural defects. |

---

## 2. Lifecycle Progression & Evolution Methodology

```mermaid
graph LR
    P["proposed/<br/>(Proposal)"] -->|Implementation Shipped| I["implemented/<br/>(Active Decision)"]
    P -->|Declined / Impractical| R["rejected/<br/>(Guardrail)"]
    I -->|Superseded / Closed History| A["archived/<br/>(Frozen Snapshot)"]
```

### 1. Advancing from `proposed/` to `implemented/`
When code for a proposal is built, verified, and merged:
1. **Move File**: Move from `proposed/{topic}.md` to `implemented/{class}/{topic}.md`.
2. **Update Status**: Change `Status: proposed` to `Status: implemented`.
3. **Rewrite Body**:
   - Replace `## Proposal` with `## Decision` (written in present-tense fact).
   - Fold `## Acceptance criteria` and `## Risks` into `## Consequences` or `## Verification`.
   - Remove hypothetical or migration planning text in favor of what actually shipped.

### 2. Archiving from `implemented/` to `archived/`
When a decision is completely shipped, stable, and its rationale has been superseded or absorbed into newer notes:
1. **Move File**: Move from `implemented/{class}/{topic}.md` to `archived/{class}/{topic}.md` (`implemented` is omitted in the archive path).
2. **Add Archive Header**: Insert `Archived: YYYY-MM-DD` immediately below `Status: implemented`.
3. **Freeze Content**: Do not edit, translate, or refactor archived notes; they remain historical records.

### 3. Rejecting a Proposal
If a proposed approach is rejected during review:
1. **Move File**: Move from `proposed/{topic}.md` to `rejected/{class}/{topic}.md`.
2. **Update Status**: Set `Status: rejected — <concise rationale in one line>`.
3. **Retention Policy**: Retain only if the rejected proposal prevents a recurring, tempting mistake; otherwise delete.

---

## 3. Working Inventory of Agent Notes

### Proposed Notes Under Review (`proposed/`)

#### Architecture (`proposed/architecture/`)

| Date | Title | Focus |
|---|---|---|
| 2026-08-28 | [CLI Through App Server: Unified Execution Base](proposed/architecture/2026-08-28-cli-through-app-server-unified-runtime.md) | Make App Server the single CLI, JSON-RPC, and ACP execution base and remove parallel turn orchestration |

#### Bug Fixes (`proposed/bug-fix/`)

| Date | Title | Focus |
|---|---|---|
| 2026-08-28 | [Goal Mode Runtime Repair and End-to-End Evidence](proposed/bug-fix/2026-08-28-goal-runtime-and-verifier-evidence.md) | Repair worker timer construction and verifier history admission; gate Goal readiness on the actual CLI workflow |

#### Development Process (`proposed/process/`)

| Date | Title | Focus |
|---|---|---|
| 2026-08-27 | [Stabilization and Evidence Gates](proposed/process/2026-08-27-stabilization-and-evidence-gates.md) | Close runtime boundary defects, verify complete workflows, and align release claims with evidence before expanding scope |

---

### Active Implemented Notes (`implemented/`)

#### Architecture (`implemented/architecture/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-28 | [In-Process Agent Protocol](../../crates/mini-agent-protocol) | Portable model, tool, message, event, stop, and limit contracts shared by hosts |
| 2026-08-28 | [Codex-Style Core and Protocol Reorganization](implemented/architecture/2026-08-28-codex-style-core-and-protocol-reorganization.md) | Core-owned runtime semantics, structured tool outcomes, storage-neutral checkpoints, and a thin app-server control plane |
| 2026-08-28 | [Current Harness Execution Flow](implemented/architecture/2026-08-28-harness-execution-flow.md) | Actual one-shot, interactive, and app-server paths through Thread, Harness, model/tool steps, events, control, and persistence |
| 2026-08-28 | [External Harness and ACP Boundary](implemented/architecture/2026-08-28-external-harness-and-acp-boundary.md) | Four-layer CLI → App Server → Host/Workflows → Core/Protocol boundary with JSON-RPC, stdio, approvals, lifecycle, and experimental ACP mapping |
| 2026-08-29 | [App Server Session, World, and MCP Management](implemented/architecture/2026-08-29-app-server-session-world-mcp-management.md) | Shared local and JSON-RPC management service for session metadata, workspace state, execution policy, and MCP status/retry |
| 2026-08-29 | [CLI Without Direct Host and Capabilities Dependencies](implemented/architecture/2026-08-29-cli-without-host-capabilities-dependencies.md) | App Server frontend facade removes direct CLI compilation dependencies; provider experiments move to a separate crate |
| 2026-08-24 | [Core Harness Boundary](implemented/architecture/2026-08-24-core-harness-boundary.md) | Strict boundary between pure microkernel core and host CLI adapters |
| 2026-08-24 | [Hard Limits System](implemented/architecture/2026-08-24-hard-limits-system.md) | Hard bounds on context, responses, step count, and UTF-8 head/tail truncation |
| 2026-08-26 | [Event-Driven Reactive Loop](implemented/architecture/2026-08-26-event-driven-reactive-loop.md) | Passive immutable observers driving live client UI and session-backed durable history |
| 2026-08-28 | [Session as the Single Durable Runtime Store](implemented/architecture/2026-08-28-session-single-source-of-truth.md) | Session JSONL owns settled history and result handles; live events remain in-process |
| 2026-08-26 | [Session Branching Lanes](implemented/architecture/2026-08-26-session-branching-lanes.md) | Multi-branch tree conversations and speculative exploration lanes |
| 2026-08-26 | [Local Sandbox Adapter](implemented/architecture/2026-08-26-local-sandbox-adapter.md) | Approval rules, path checks, Docker execution, and native process-tree cleanup |
| 2026-08-27 | [Model Context Boundaries & CLI Decoupling](implemented/architecture/2026-08-27-model-context-boundaries-and-cli-decoupling.md) | Model item ceilings, atomic turn trimming, CLI decoupling, and session continuity |
| 2026-08-27 | [Subprocess CLI & ACP Subagent Execution](implemented/architecture/2026-08-27-subprocess-cli-and-acp-subagent-execution.md) | Headless `mini-agent ask --json` subprocess spawning and `spawn_agent` |
| 2026-08-27 | [Multi-turn Interactive Subagent Sessions](implemented/architecture/2026-08-27-multi-turn-interactive-subagent-sessions.md) | Session-backed resumption through `send_subagent_message` |
| 2026-08-27 | [Subagent Trace Replay & Session Lineage](implemented/architecture/2026-08-27-subagent-trace-replay-and-session-lineage.md) | Parent-session subagent records and bounded result rollup |
| 2026-08-27 | [Session Directory & Metadata Architecture](implemented/architecture/2026-08-27-session-directory-and-metadata-architecture.md) | Actual session files, goal/plan state, attachments, and subagent records |
| 2026-08-27 | [Builtin Agent & Persona Prompt System](implemented/architecture/2026-08-27-builtin-agent-personas-and-file-contracts.md) | Builtin agent/persona prompts, dual-mode file contracts (review/summary), and issue state tracking |
| 2026-08-27 | [Goal and Plan Subsystem Architecture](implemented/architecture/2026-08-27-goal-and-plan-subsystem-architecture.md) | Explicit triggers, Living Plan protocol (plan.md), and autonomous verification state machine (goal/) |
| 2026-08-27 | [web_fetch / read_image session impact](implemented/architecture/2026-08-27-web-fetch-and-read-image-session-impact.md) | Envelope-only history; resume/fork attachments; compact empty tools; prefix-cache misses |
| 2026-08-28 | [Interactive Steer and Follow-up Run Control](implemented/architecture/2026-08-28-steer-and-follow-up-run-control.md) | `/steer` priority correction, FIFO follow-up queue, cooperative safe checkpoints, and durable `steered` turns |
| 2026-08-28 | [Host Capability Profiles and Runtime Seams](implemented/architecture/2026-08-28-host-capability-profiles-and-runtime-seams.md) | CLI/App Server/ACP profile selection, regular-agent prompt/rule scope, bounded manifests, startup profile resolution, and selected MCP loading |

#### Features & Extensions (`implemented/feature/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-24 | [Context Compaction](implemented/feature/2026-08-24-context-compaction.md) | Prefix compaction preserving latest world state and recent tool work verbatim |
| 2026-08-24 | [Durable Sessions & Recovery](implemented/feature/2026-08-24-durable-sessions-and-recovery.md) | Append-only JSONL session checkpoints with torn-tail auto recovery |
| 2026-08-24 | [Independent Mentor System](implemented/feature/2026-08-24-independent-mentor-system.md) | Tool-free independent verification model with isolated derived items |
| 2026-08-24 | [MCP & Skills Integration](implemented/feature/2026-08-24-mcp-and-skills-integration.md) | Stdio and HTTP MCP support with progressive skill discovery |
| 2026-08-24 | [Explicit World State](implemented/feature/2026-08-24-explicit-world-state.md) | Deterministic host environment detection and context injection |
| 2026-08-26 | [Fail-Closed Approval](implemented/feature/2026-08-26-fail-closed-approval-and-tool-orchestration.md) | Permission matrix, interactive TTY approval, and path containment |
| 2026-08-26 | [Autonomous Goal Mode](implemented/feature/2026-08-26-autonomous-goal-mode.md) | Long-running goal execution with convergence gates and loop detection |
| 2026-08-26 | [MCP Circuit Breaker](implemented/feature/2026-08-26-mcp-circuit-breaker.md) | Circuit breaking and graceful degradation for failing remote HTTP MCP servers |

#### Simplification (`implemented/simplification/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-24 | [Source Code Line Budget](implemented/simplification/2026-08-24-source-code-line-budget.md) | Strict line budgets (20k runtime layers / 30k workspace) to prevent abstraction bloat |

#### Testing (`implemented/testing/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-27 | [Real LLM Scenario Runner](implemented/testing/2026-08-27-real-llm-scenario-runner.md) | Explicit real-provider scenarios with request, output, and wall-time budgets |

---

### Rejected Proposals (Guardrails) (`rejected/`)

#### Architecture (`rejected/architecture/`)
| Date | Title | Rationale |
|---|---|---|
| 2026-08-24 | [Generic Persistence in Core](rejected/architecture/2026-08-24-generic-persistence-in-core.md) | Cannot settle non-idempotent external effects; persistence belongs at edge |
| 2026-08-27 | [Subagent Task Scheduling & Delegation](rejected/architecture/2026-08-27-subagent-task-scheduling-and-delegation.md) | In-process multi-tenant scheduler adds unnecessary core complexity; replaced by Subprocess CLI execution and session-backed multi-turn architecture |
| 2026-08-26 | [Event Stream Rollout Replay](rejected/architecture/2026-08-26-event-stream-rollout-replay.md) | Detailed external trace replay was prompt-weight-specific; session JSONL is the mainline durable record |

#### Features & Extensions (`rejected/feature/`)
| Date | Title | Rationale |
|---|---|---|
| 2026-08-24 | [Un-Settled Effect Replay](rejected/feature/2026-08-24-un-settled-effect-replay.md) | Replaying interrupted effects produces duplicate non-idempotent actions |
| 2026-08-24 | [Unrestricted Whole-File Rewrite](rejected/feature/2026-08-24-unrestricted-whole-file-rewrite.md) | Full rewrites drop unrelated code in long contexts; exact replacement is safer |
| 2026-08-24 | [Prompt Weight Protocol](rejected/feature/2026-08-24-prompt-weight.md) | Prompt-weight benchmarking is optional and outside the project mainline |
| 2026-08-26 | [Prompt Weight Evaluation](rejected/feature/2026-08-26-prompt-weight-evaluation.md) | Real-model prompt comparison is optional and outside the project mainline |

---

### Historical Archive (`archived/`)

#### Experiments (`archived/experiments/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-24 | [Unknown-Tool Recovery](archived/experiments/2026-08-24-unknown-tool.md) | Recovery-capable model passes by projecting tool failure back to context |
| 2026-08-24 | [Edit Surface Comparison](archived/experiments/2026-08-24-edit-surface.md) | Exact unique replacement preserves collateral content over full rewrite |
| 2026-08-24 | [Tool-Output Retention](archived/experiments/2026-08-24-tool-output-retention.md) | Head-plus-tail truncation preserves both orientation and final verdict |
| 2026-08-24 | [Effect Recovery Boundary](archived/experiments/2026-08-24-effect-recovery.md) | Replay safety simulation across non-idempotent crash boundaries |

#### Features (`archived/feature/`)
| Date | Title | Focus |
|---|---|---|
| 2026-08-27 | [DeepSeek and GLM Provider Seams](archived/feature/2026-08-27-deepseek-and-glm-provider-seams.md) | Historical provider seam record superseded by the single Responses protocol |
