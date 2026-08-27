# Session Directory Architecture: Metadata, Goal/Plan State, Subagent Trees, and Compaction Segments

Status: proposed

## 1. Context & Empirical Motivation

In-depth inspection of production session directories in Grok (specifically `01a037d1-1cd8-7c91-85ce-6a262bea483a` in `mini-codex` and `01a03bbf-d93f-7332-9d8a-eec6b9107ebc` in `llm-review`) reveals an advanced, highly modular directory layout. It successfully decouples:

1. **Fast Index Metadata & Telemetry** (`summary.json`, `signals.json`, `prompt_context.json`)
2. **Interactive Plan & Goal Engine State** (`plan.md`, `plan_mode.json`, `goal/state.json`, `goal/plan.md`)
3. **Hierarchical Subagent Trees** (`subagents/<subagent_id>/{meta.json, output.json}`)
4. **Segmented Compaction Archives** (`compaction/INDEX.md`, `compaction/segment_*.md`)
5. **Out-of-Band Process Logs** (`terminal/call-*.log`)
6. **Conversational Stream & Checkpoint History** (`chat_history.jsonl`, `events.jsonl`, `rewind_points.jsonl`)

In contrast, `mini-agent` currently uses a single append-only `session.jsonl` file per session directory. While effective for basic crash recovery, it exhibits several architectural bottlenecks as features expand to subagents and autonomous goals:
- **Listing Bottleneck**: `mini-agent sessions` must scan large JSONL files to extract basic turn counts and titles.
- **Compaction History Loss**: Old turns pruned during context compaction cannot be inspected or queried unless the full trace is parsed.
- **Subagent Flatness**: Child subagent sessions are prefixed by convention (`sub-...`) rather than grouped as a tree.
- **Goal State Co-mingling**: Autonomous goal convergence state (worker rounds, verifier feedback, baseline commits) lacks a dedicated home.

This proposal establishes a unified **Session Directory Architecture** for `mini-agent` that adopts these proven structures while staying strictly within our 20k/30k line budget.

---

## 2. Complete Anatomy of the Production Session Directory

```
.agents/sessions/<session_id>/
├── summary.json              # Fast O(1) session index & lineage metadata
├── signals.json              # Quantitative telemetry (token usage, tool call counts, latencies)
├── prompt_context.json       # Frozen environment snapshot (AGENTS.md, OS, shell, timestamp)
├── system_prompt.txt         # Exact rendered system prompt for deterministic replay
├── session.jsonl             # Append-only active conversation & checkpoint log
│
├── plan.md                   # Active architectural plan (Problem, Non-goals, Approach, Verification)
├── plan_mode.json            # Interactive planning state (Active / Inactive, awaiting_approval)
│
├── goal/                     # Autonomous Goal Mode runtime directory
│   ├── state.json            # Goal state machine (objective, worker/verify rounds, baseline_commit)
│   ├── plan.md               # Acceptance criteria, verification steps, and task checklist
│   └── verifier_verdict.md   # Adversarial classifier/verifier report
│
├── subagents/                # Nested subagent execution records
│   └── <child_session_id>/
│       ├── meta.json         # Subagent invocation params, duration, turns, tool calls
│       └── output.json       # Structured return result payload
│
├── compaction/               # Archived conversation memory segments
│   ├── INDEX.md              # Markdown index table (Segment, Turns, Approx bytes, Keywords)
│   └── segment_000.md        # Pruned conversation turns preserved for retrieval
│
└── terminal/                 # Out-of-band process execution logs
    └── call-<call_id>.log    # Full stdout/stderr for commands > 32 KiB
```

---

## 3. Core Subsystems Specification

### 3.1. Fast Index Metadata (`summary.json`) & Telemetry (`signals.json`)

`summary.json` provides $O(1)$ fast discovery for CLI listing without reading `session.jsonl`:

```json
{
  "id": "01a037d1-1cd8-7c91-85ce-6a262bea483a",
  "parent_session_id": null,
  "root_session_id": "01a037d1-1cd8-7c91-85ce-6a262bea483a",
  "title": "Smoother compact for long auto runs",
  "created_at": "2026-08-25T07:40:00Z",
  "updated_at": "2026-08-27T13:00:00Z",
  "model": "deepseek-chat",
  "security_preset": "turbomode",
  "turn_count": 23,
  "subagents_count": 5,
  "git": {
    "branch": "main",
    "commit": "32a0e74"
  }
}
```

`signals.json` tracks cumulative execution metrics:
```json
{
  "turn_count": 23,
  "tool_call_count": 352,
  "input_tokens_used": 333902,
  "tools_used": ["read_file", "edit_file", "shell", "spawn_agent", "todo_write"],
  "git_commit_count": 4,
  "session_duration_seconds": 6718
}
```

### 3.2. Subagent Tree Storage (`subagents/<subagent_id>/`)

When `spawn_agent` executes a subagent:
1. Child executes in its own isolated process with `--session-id sub-...`.
2. Parent writes invocation metadata to `.agents/sessions/<parent_id>/subagents/<child_id>/meta.json`:
   - `subagent_id`, `description`, `task_name`, `started_at`, `completed_at`, `duration_ms`, `tool_calls`, `turns`.
3. Stores child output in `output.json`.

This cleanly decouples child execution logs while preserving the complete execution hierarchy.

### 3.3. Compaction Memory Segments (`compaction/`)

When context compaction triggers:
1. Pruned prefix messages are written to `.agents/sessions/<id>/compaction/segment_NNN.md`.
2. `.agents/sessions/<id>/compaction/INDEX.md` is appended with:
   `| Segment | File | Turns | Approx bytes | Keywords |`
3. Allows models or users to retrieve historical context via `read_file` without bloating active memory.

### 3.4. Autonomous Goal State (`goal/`)

For `mini-agent auto` / goal execution:
1. `goal/state.json`: Maintains state machine (`goal_id`, `status: "running" | "user_paused" | "completed"`, `phase: "Executing" | "Verifying"`, `total_worker_rounds`, `total_verify_rounds`).
2. `goal/plan.md`: The active contract (`## Acceptance criteria`, `## Verification plan`, `## Task checklist`).
3. `goal/verifier_verdict.md`: Verification audit findings.

---

## 4. Implementation Strategy & Budget Guardrails

1. **Host-Adapter Encapsulation**:
   - `mini-agent-core` remains a pure, dependency-free microkernel owning `harness.rs`, `events.rs`, `limits.rs`.
   - All directory manipulation, `summary.json`, `signals.json`, and `compaction/` writing lives exclusively in `crates/mini-agent-cli/src/session.rs` and `crates/mini-agent-cli/src/subagent.rs`.
2. **Backward Compatibility**:
   - If a session directory only contains legacy `session.jsonl` (or a flat file `<id>.jsonl`), the loader transparently reads it and writes out `summary.json` upon next save.
3. **Line Budget Impact**:
   - Estimated CLI additions: $\approx 180$ lines in `session.rs` and `subagent.rs`. Workspace line count remains well under the 30,000 line ceiling.

---

## 5. Verification Plan

1. **Unit Tests**:
   - Verify `summary.json` generation and $O(1)$ fast discovery in `crates/mini-agent-cli/src/session.rs`.
   - Verify subagent metadata logging in `crates/mini-agent-cli/src/subagent.rs`.
   - Verify compaction segment indexing in `compaction/INDEX.md`.
2. **Integration Tests**:
   - `sessions_list_reads_summary_json_fast`
   - `subagent_writes_meta_and_output_to_parent_subagents_dir`
   - `compaction_archives_pruned_messages_to_segment_files`