# Model Context Boundaries, CLI Decoupling, and Session Continuity

Status: implemented

## Context

During review of v0.2.0 change size and model-context integrity, several critical architectural seams and edge cases were identified:
1. \crates/mini-agent-cli/src/main.rs\ had grown into a monolithic entry point (>1,400 lines) coupling CLI argument parsing, help constants, harness construction, and dispatch.
2. \crates/mini-agent-cli/src/trace.rs\ directly deserialized \session.jsonl\ schema records, coupling trace analysis to internal session storage details.
3. Relaxed response limits (384 KiB) and oversized \AGENTS.md\ (64 KiB) permitted individual model items to exceed the 10K-token item ceiling.
4. History restoration (estore_history\) checked only \Message::Context\, allowing oversized assistant or tool items to bypass validation upon resume.
5. Byte-wise message trimming dropped \Assistant\ messages without dropping matching \Tool\ outputs, producing orphan \unction_call_output\ messages that violate provider schemas.
6. Transient loop advisories displaced authoritative environment and world state during context compaction.
7. Auxiliary compaction requests inherited model-level \web_search\, exposing internal summarization to external side-effects.
8. Process-local handles (\process_id\, esult-1\) were left un-invalidated when resuming durable sessions across CLI process restarts.

## Decision

We instituted strict architectural boundaries and refactored core harness algorithms and CLI host adapters:

### 1. CLI Decoupling & Modularization
- **[\crates/mini-agent-cli/src/args.rs\](crates/mini-agent-cli/src/args.rs)**: Owns \Invocation\, \Command\, \HelpTopic\, \parse_args\, help strings, and argument unit tests.
- **[\crates/mini-agent-cli/src/harness_builder.rs\](crates/mini-agent-cli/src/harness_builder.rs)**: Owns OpenAI provider harness assembly, tool initialization, and configuration.
- **[\crates/mini-agent-cli/src/session.rs\](crates/mini-agent-cli/src/session.rs)**: Encapsulates \	ry_load_session_events\ to translate \session.jsonl\ into portable \mini_agent_core::Event\ streams, freeing [\crates/mini-agent-cli/src/trace.rs\](crates/mini-agent-cli/src/trace.rs) from storage schema dependencies.
- **[\crates/mini-agent-cli/src/main.rs\](crates/mini-agent-cli/src/main.rs)**: Reduced from 1,404 lines to ~350 lines, serving purely as a top-level dispatcher.

### 2. Model Item Ceilings & Project Context Bounds
- \max_model_response_bytes\: Bounded to 4 KiB\ (~16K tokens) in [\HarnessConfig\](crates/mini-agent-core/src/harness.rs).
- \MAX_PROJECT_INSTRUCTIONS_BYTES\: Bounded to  KiB\ (~4K tokens) in [\project_context.rs\](crates/mini-agent-cli/src/project_context.rs) with UTF-8 head and tail retention.
- Compaction summary: Explicitly truncated to \max_user_input_bytes\ ( KiB\).

### 3. Comprehensive History Validation on Resume
- [estore_history\](crates/mini-agent-core/src/harness.rs) validates all message variants:
  - \Message::Context\: \<= max_context_item_bytes\ (8 KiB)
  - \Message::User\: \<= max_user_input_bytes\ (32 KiB)
  - \Message::Assistant\: \<= max_model_response_bytes\ (64 KiB) and \<= max_tool_calls_per_step\ (8)
  - \Message::Tool\: \<= max_tool_output_bytes\ (16 KiB)
  - Total context: \<= max_context_bytes\ (1 MiB)

### 4. Turn-Atomic Message Trimming
- emove_first_message_group\ in [\crates/mini-agent-core/src/harness.rs\](crates/mini-agent-core/src/harness.rs) guarantees that an \Assistant\ message containing tool calls and all its corresponding following \Tool\ messages are removed atomically, eliminating orphan tool settlement items.

### 5. Compaction Headroom & State Prioritization
- \	ake_latest_context\ ignores transient \[Loop warning:\ advisories, ensuring authoritative \WorldState\ snapshots are permanently preserved.
- Mechanical compaction and model summarization target \< compact_at\ (512 KiB) to provide stable headroom and prevent immediate re-compaction loops.

### 6. Settlement-Aware Loop Detection
- Loop detector compares \(name, arguments, output_content)\ tuples across consecutive steps. Repetitive polling with changing tool output (e.g. process monitoring) is recognized as active progress rather than a stall.

### 7. Request-Scoped Tool Suppression
- [\crates/mini-agent-cli/src/openai.rs\](crates/mini-agent-cli/src/openai.rs) only injects \web_search\ and function definitions when \!request.tools.is_empty()\. Auxiliary compaction and mentor requests are strictly isolated from tool execution and web access.

### 8. Durable Session Boundaries & Legacy Continuity
- Session resume and fork in [\crates/mini-agent-cli/src/repl.rs\](crates/mini-agent-cli/src/repl.rs) append an explicit boundary notice informing the model that prior process IDs and result store handles have expired.
- \MAX_RECORD_BYTES\ is raised to  MiB\ in [\crates/mini-agent-cli/src/session.rs\](crates/mini-agent-cli/src/session.rs) to safely serialize 1 MiB checkpoints.
- esolve_session_file\ provides backward-compatible fallback for \.agents/sessions/<id>.jsonl\ and \.agents/sessions/<id>/session.jsonl\.

## Consequences

- Prevents context token explosion and keeps all prompt fragments and model items deterministically bounded.
- Eliminates provider protocol errors caused by orphan tool outputs.
- Protects world-state facts across long-running autonomous runs.
- Improves code maintainability and testability through clean separation of CLI arguments, harness building, session persistence, and trace evaluation.
