# Model Context Boundaries, CLI Decoupling, and Compatibility Guarantees

Status: implemented

## Context

During review of v0.2.0 change size, model-context integrity, and backward compatibility, several critical architectural seams and edge cases were identified:
1. `crates/mini-agent-cli/src/main.rs` had grown into a monolithic entry point (>1,400 lines) coupling CLI argument parsing, help constants, harness construction, and dispatch.
2. The former external trace path coupled CLI analysis to internal session storage details; the mainline now keeps settled history and result handles in `session.jsonl`.
3. Relaxed response limits (384 KiB) and oversized `AGENTS.md` (64 KiB) permitted individual model items to exceed the 10K-token item ceiling.
4. History restoration (`restore_history`) checked only `Message::Context`, allowing oversized assistant or tool items to bypass validation upon resume.
5. Byte-wise message trimming dropped `Assistant` messages without dropping matching `Tool` outputs, producing orphan `function_call_output` messages that violate provider schemas.
6. Security policy deny rules expected canonical identifiers (`shell:<cmd>`, `file:write:<path>`) but received human strings (``shell command `<cmd>` ``), bypassing deny filters.
7. Docker sandbox selection (`--sandbox docker`) was parsed but executed directly on the host without containerization.
8. Background managed processes were not reliably terminated on Windows due to unattached JobObject guards.
9. Interactive sessions persisted sensitive history by default without explicit opt-in, deviating from `docs/privacy.md`.
10. `max_steps = 0` was reinterpreted as unlimited steps rather than zero steps.

## Decision

We instituted strict architectural boundaries and refactored core harness algorithms and CLI host adapters:

### 1. CLI Decoupling & Modularization
- **[`crates/mini-agent-cli/src/args.rs`](../../../../crates/mini-agent-cli/src/args.rs)**: Owns `Invocation`, `Command`, `HelpTopic`, `parse_args`, help strings, and argument unit tests. Error messages and help are printed to `stderr` to keep `stdout` machine-readable.
- **[`crates/mini-agent-cli/src/harness_builder.rs`](../../../../crates/mini-agent-cli/src/harness_builder.rs)**: Owns OpenAI provider harness assembly, tool initialization, security preset application, sandbox configuration, and prompt augmentation.
- **[`crates/mini-agent-cli/src/session.rs`](../../../../crates/mini-agent-cli/src/session.rs)**: Encapsulates `try_load_session_events` to translate `session.jsonl` into portable `mini_agent_core::Event` streams, freeing [`crates/mini-agent-cli/src/trace.rs`](../../../../crates/mini-agent-cli/src/trace.rs) from storage schema dependencies.
- **[`crates/mini-agent-cli/src/main.rs`](../../../../crates/mini-agent-cli/src/main.rs)**: Reduced from 1,404 lines to ~350 lines, serving purely as a top-level dispatcher.

### 2. Model Item Ceilings & Project Context Bounds
- `max_model_response_bytes`: Bounded to `64 KiB` (~16K tokens) in [`HarnessConfig`](../../../../crates/mini-agent-core/src/harness.rs).
- `MAX_PROJECT_INSTRUCTIONS_BYTES`: Bounded to `16 KiB` (~4K tokens) in [`project_context.rs`](../../../../crates/mini-agent-cli/src/project_context.rs) with UTF-8 head and tail retention.
- Compaction summary: Explicitly truncated to `max_user_input_bytes` (`32 KiB`).

### 3. Comprehensive History Validation & Legacy Discovery
- [`restore_history`](../../../../crates/mini-agent-core/src/harness.rs) validates all message variants:
  - `Message::Context`: `<= max_context_item_bytes` (8 KiB)
  - `Message::User`: `<= max_user_input_bytes` (32 KiB)
  - `Message::Assistant`: `<= max_model_response_bytes` (64 KiB) and `<= max_tool_calls_per_step` (8)
  - `Message::Tool`: `<= max_tool_output_bytes` (16 KiB)
  - Total context: `<= max_context_bytes` (1 MiB)
- `list`, `resume`, `fork`, and `resolve_session_file` in [`crates/mini-agent-cli/src/session.rs`](../../../../crates/mini-agent-cli/src/session.rs) probe both `~/.mini-agent/sessions/<workspace>/` and legacy `.agents/sessions/<id>.jsonl` / `<id>/session.jsonl`.

### 4. Turn-Atomic Message Trimming & State Prioritization
- `remove_first_message_group` in [`crates/mini-agent-core/src/harness.rs`](../../../../crates/mini-agent-core/src/harness.rs) guarantees that an `Assistant` message containing tool calls and all its corresponding following `Tool` messages are removed atomically, eliminating orphan tool settlement items.
- `take_latest_context` ignores transient `[Loop warning:` advisories, ensuring authoritative `WorldState` snapshots are permanently preserved.
- Loop detector compares `(name, arguments, output_content)` tuples across consecutive steps. Repetitive polling with changing tool output (e.g. process monitoring) is recognized as active progress rather than a stall.

### 5. Security Normalization & Sandboxing
- [`crates/mini-agent-cli/src/security.rs`](../../../../crates/mini-agent-cli/src/security.rs) normalizes human-formatted actions into canonical patterns and applies glob wildcard matching (`**/.env*`, `rm -rf /*`, `gh auth *`).
- [`crates/mini-agent-cli/src/workspace.rs`](../../../../crates/mini-agent-cli/src/workspace.rs) validates `--sandbox docker` availability and executes within containers or fails-closed with clear diagnostics.
- `terminate_process_tree` invokes `taskkill /PID <pid> /T /F` on Windows to guarantee child tree termination.

### 6. Persistence & Step Limit Semantics
- Interactive and one-shot `ask` sessions default to process-local memory;
  Interactive, `ask`, and `auto` sessions always create durable files under
  `~/.mini-agent/sessions/`; persistence is not configurable at the CLI edge.
- `max_steps = 0` produces an immediate step limit halt; unconstrained runs pass `usize::MAX`.
- `OPENAI_BASE_URL` with custom non-official endpoints defaults `web_search` to false unless explicitly enabled.

## Consequences

- Prevents context token explosion and keeps all prompt fragments and model items deterministically bounded.
- Eliminates provider protocol errors caused by orphan tool outputs.
- Protects world-state facts across long-running autonomous runs.
- Guarantees fail-closed security rule evaluation and deterministic child process tree destruction.
