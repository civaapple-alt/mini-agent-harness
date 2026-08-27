# Changelog

All notable changes to Mini Agent Harness are documented here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

- Host `web_fetch` GETs a known public HTTP(S) URL and returns bounded readable text (HTML stripped, JavaScript not executed, localhost/private/credentialed URLs rejected). Host `open_file` opens a workspace file in the OS default app so local HTML can be viewed in a browser without a screenshot or browser-agent tool.
- First-class Plan Mode (`/plan`, `/plan <prompt>`, `/plan off`) with workspace modification locking and a session-directory living plan (`plan.md`).
- Autonomous Goal Mode (`/goal <objective>`) state machine (`goal/state.json`, `goal/plan.md`) with milestone tracking and independent verifier gate support.
- Builtin foundational agent prompts (`explore`, `plan`, `general`) and 7 specialized personas (`reviewer`, `implementer`, `security-auditor`, `test-writer`, `researcher`, `design-doc-writer`, `design-doc-reviewer`) in `persona.rs`.
- Dual-mode file collaboration contracts (`review_file`, `summary_file`) with automated prompt rendering, issue lifecycle state tracking (`open`, `fixed`, `wontfix`, `addressed`), and live review statistics in `SpawnAgent`.
- Modular session directory layout generating atomic `summary.json`, `signals.json`, and `prompt_context.json` snapshots alongside durable `session.jsonl` records.
- Fast $O(1)$ session discovery in `mini-agent sessions` reading lightweight `summary.json` metadata indexes without parsing full conversation streams.
- Subagent tree execution tracking recording lifecycle metrics (`started_at_ms`, `completed_at_ms`, `duration_ms`, `steps`, `exit_code`, `status`) in `meta.json` and structured deliverables in `output.json` under `.agents/sessions/<id>/`.
- Subagent preset role configurations (`explore` with read-only guidelines, `plan` for software architecture design, `general` for full execution) and `fork_context` controls in `SpawnAgent`.
- Subprocess CLI-driven subagent execution tool (`spawn_agent`), allowing parent agents to delegate bounded tasks to isolated child `mini-agent ask "<prompt>" --json` processes with zero prompt pollution, OS-level crash/memory isolation, and structured result aggregation.
- Multi-turn interactive subagent session tool suite (`send_subagent_message`, `list_subagents`) enabling stateful conversational follow-ups and refinement with child agents via durable session resumption (`--session-id`).
- Implicit persistence encapsulation for subagents: `spawn_agent` automatically provisions unique session identifiers (`sub-<time>-<task>`) and commits child checkpoints for seamless multi-turn resumption while standard CLI invocations remain clean and ephemeral.
- Hierarchical trace rollup and subagent observation tracking in `trace summary` and `trace replay`.
- Multi-item validation on durable session restore (`restore_history`) validating `Context`, `User`, `Assistant`, and `Tool` message bounds individually.
- Turn-atomic trimming (`remove_first_message_group`) ensuring function calls and settlement outputs are dropped as cohesive groups during compaction.
- Settlement-aware loop detection tracking `(name, arguments, content)` to avoid false positive stall warnings during polling.
- Session resume and fork invalidation notice informing the model that prior process IDs and result preview handles have expired.
- Backward-compatible session lookup for legacy `.agents/sessions/<id>.jsonl` and `.agents/sessions/<id>/session.jsonl` across `resume`, `fork`, and `sessions` listing.
- Canonical security action normalization and glob wildcard matching (`**/.env*`, `rm -rf /*`, `gh auth *`) in `SecurityPolicy`.
- Docker sandbox execution validation ensuring container isolation or failing closed with clear diagnostics.
- Backward-compatible `--auto` alias for `--auto-approve` in `mini-agent ask`.
- Decoupled CLI architecture with dedicated `args.rs` and `harness_builder.rs` modules, reducing `main.rs` to a lightweight dispatcher.

### Fixed

- `/plan <prompt>` now enters Plan Mode and starts drafting with that prompt instead of being rejected as an unknown command.
- Plan Mode writes go to the session living plan (`plan.md` under the session directory); relative `plan.md` is aliased there and other workspace mutations stay locked.
- REPL Plan Mode overlays the builtin `plan` Software Architect foundation plus a living-plan rider so the model plans into `plan.md` instead of emitting the deliverable.
- Plan Mode `write_file plan.md` replaces the initialized session living plan instead of failing with "file already exists".
- `/goal <objective>` now starts executing the first milestone immediately instead of only writing `goal/state.json` and returning to the prompt.
- Goal Mode can read and update session `goal/plan.md` (relative `goal/plan.md` maps there) instead of failing with "path escapes the workspace".
- Windows shell capture forces UTF-8 (`PYTHONUTF8`, PowerShell `$OutputEncoding`, and `Get-Content`/`Set-Content` default Encoding) so UTF-8 HTML previews are not shown as mojibake.
- Subagent tree records (`meta.json`, `output.json`, `parent_session_id`) live under the parent session at `~/.mini-agent/sessions/<workspace>/<parent-id>/subagents/<child-id>/`, not in the project `.agents/sessions/` directory.
- `spawn_agent` and `send_subagent_message` run child `ask` with `--max-steps 50` (and a 300s default timeout) so reviews are less likely to stop after 8 steps.
- REPL `tool>` lines for `spawn_agent` and `send_subagent_message` show task/persona/session_id and a bounded message preview.
- Security deny rules now properly match human-formatted tool action strings, preventing destructive commands from bypassing deny filters.
- Windows background managed process trees are guaranteed to terminate via `taskkill /PID <pid> /T /F` and sandbox attachment.
- CLI argument errors and root help on usage failure now print to `stderr`, keeping `stdout` pure for machine-readable JSON consumers.
- Prompted `mini-agent auto` now correctly applies `--security-preset` and `--sandbox` configurations.

### Changed

- Interactive sessions default to process-local memory; `--persist` opts in to creating durable sessions (aligning with `docs/privacy.md`).
- Strict `max_steps = 0` step limit evaluation halts immediately; unconstrained runs pass `usize::MAX`.
- Adaptive `web_search` default enables web search for official OpenAI/DeepSeek endpoints while disabling it for custom local LLM endpoints unless explicitly configured.
- Lowered default `max_model_response_bytes` to 64 KiB (~16K tokens) and bounded compaction summaries to 32 KiB (`max_user_input_bytes`).
- Reduced `MAX_PROJECT_INSTRUCTIONS_BYTES` to 16 KiB (~4K tokens) with head/tail truncation.
- Raised durable session `MAX_RECORD_BYTES` to 2 MiB to reliably serialize full 1 MiB checkpoints.
- Compaction prioritizes authoritative `WorldState` context over transient loop advisories.
- OpenAI request serialization suppresses `web_search` and function schemas when `request.tools` is empty (e.g. during auxiliary compaction and mentor requests).

## [0.2.0] - 2026-08-26

### Added

- `mini-agent --version` and the REPL welcome include the git revision, for
  example `mini-agent 0.2.0 (c0ffee12abcd)`.
- User-level provider settings at `~/.mini-agent/.env`, below process
  environment and workspace `.env`, so a PATH-installed binary can keep
  credentials out of project directories.
- Minimal project-scoped Agent Skills and Agent Plugins 1.0.0 discovery with
  progressive skill disclosure and approval-gated stdio MCP tools via RMCP.
- Cloned skill collections, Claude/Grok plugins and explicitly selected local
  plugin marketplaces, including compatible plugin-agent instructions.
- Standalone project MCP configuration and approval-gated streamable HTTP MCP
  with bounded headers, environment placeholders, and connection timeouts.
- Individual immediate skills can be selected from cloned marketplaces without
  enabling their containing plugin bundle.
- `.agents/marketplaces.json` can name `skills` and `plugins` separately.
  Explicit `skills` selectors walk a cloned collection for a nested `SKILL.md`
  directory and do not require a Claude or Grok marketplace manifest.
- Marketplace and skillset objects accept a local `path` so an existing clone
  does not need to live under `.agents/`. `.agents/skillsets.json` enables
  only the listed skills; without that file, `.agents/skillsets/` still loads
  each collection in full. `read_file` can open those configured roots.
- Interactive `/mcp` retries configured servers that were denied or unavailable
  during startup without clearing conversation history.
- The REPL welcome block lists bounded skill, plugin, and MCP summaries and
  reports MCP transitions from inactive to enabled.
- Bounded world-state detection for host, shell, project markers, execution
  mode, approval policy, and a fixed local command catalog, exposed through
  `status`, `/world`, and `/world refresh`.
- Typed context messages mapped to Responses API developer items and retained
  across context compaction.
- Opt-in project-local durable sessions with append-only session, thread, turn,
  and item records, settled checkpoints, bounded listing, torn-tail recovery,
  `/session`, `--persist`, `sessions`, and `resume SESSION_ID`.
- Independent `mentor insight` and `mentor verify` commands with a dedicated
  model profile, zero tools, bounded criteria, source-linked derived items, and
  strict isolation from primary conversation replay.
- Tool execution orchestrator with security presets (`--security-preset default|full-machine|turbomode|custom`), priority rule evaluation (`deny > ask > allow`), and session-level approval decision caching (`ApprovalStore`).
- Windows-first native Win32 `JobObjectGuard` process containment (`--sandbox native`) with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` guaranteeing atomic child process tree destruction and zero orphaned zombie processes.
- Multi-branch durable session forking (`mini-agent fork <SESSION_ID>`) to branch settled checkpoints into independent exploration lanes.
- Offline deterministic trace replay (`mini-agent trace replay <PATH> [--json]`) and metrics computation (`mini-agent trace summary <PATH> [--json]`) over JSONL observation event logs and durable `session.jsonl` files.
- Interactive `/status` slash command in REPL displaying current workspace, active security preset, sandbox containment, approval mode, copilot loop status, and durable session metadata.
- Built-in Responses API web search integration enabled by default (passed as `{"type": "web_search"}` tool to DeepSeek/OpenAI Responses API), with `--web-search`, `--no-web-search`, and `MINI_AGENT_WEB_SEARCH=true|false` configuration options.
- Non-interactive script approval flags `--auto-approve`, `-y`, and `--yes` on `mini-agent ask`.
- Ephemeral session opt-out flags `--ephemeral` / `--no-persist` for memory-only interactive REPL and auto sessions.
- Repetitive tool-call loop detection warning in the harness run loop to prevent unattended stalls in autonomous copilot runs.
- Remote HTTP MCP server circuit breaker with fail-fast open circuit and half-open probe recovery during backend outages.

### Fixed

- `read_file`, `edit_file`, and `write_file` accept in-workspace absolute paths
  (e.g. `D:\path\to\workspace\file.txt`) without error, automatically resolving
  and validating them within the workspace security boundary.
- Interactive `read_file` (and other non-shell) `tool[ok]>` lines stay on one
  bounded preview. `shell`, `process_read`, and `read_tool_result` still print
  their full stdout/stderr in the terminal.
- Oversized root `AGENTS.md` no longer aborts startup; the host keeps a bounded
  head and tail with an explicit truncation warning.
- `ask` no longer echoes streamed assistant text before printing its final
  result; terminals retain one `assistant>` tag while redirected stdout stays
  raw.
- Interactive startup no longer blocks before displaying the REPL when an MCP
  server requires connection approval.
- Tool start events now show bounded shell and managed-process commands; file
  tools show only their path and arbitrary MCP arguments remain hidden.
- `/auto` mode changes no longer replace augmented project and extension
  instructions; they append a world-state item while keeping the stable prompt.

### Changed

- Copilot/auto no longer stops at 128 model steps. `max_steps = 0` means no
  step cap; set `MINI_AGENT_MAX_STEPS` to impose one. Context compaction still
  keeps long runs inside the 1 MiB request ceiling.
- Auto context compaction keeps the latest world-state item and the last two
  model-step groups (capped at 128 KiB) verbatim. Only the older prefix is
  summarized. If the compaction request would exceed the 1 MiB ceiling, oldest
  prefix messages are dropped until it fits. Empty or unhelpful summaries fall
  back to mechanical prefix trim instead of aborting the run.
- Durable sessions live under `~/.mini-agent/sessions/<workspace>/<session-id>/`
  instead of `.agents/sessions/` in the project tree.
- Skill discovery reads only a bounded YAML frontmatter prefix, so an oversized
  `SKILL.md` can still be selected. `read_file` accepts up to 128 KiB so those
  instruction files can be opened.

- Interactive REPL and Auto sessions are durable by default under `~/.mini-agent/sessions/`; `--ephemeral` provides memory-only scratch sessions.
- Disambiguated `auto` CLI semantics: `mini-agent auto` exclusively represents the autonomous copilot loop, while `mini-agent ask` uses `--auto-approve` / `-y` for non-interactive approval bypass.
- Default interactive sessions and TTY `ask` run tools without per-step
  approval. `/auto` is the copilot loop; `/auto off` restores prompts.
  `run` is an alias of `ask`.
- Raised the default model request context from 256 KiB to 1 MiB and the model
  response ceiling from 64 KiB to 384 KiB. These remain provider-neutral byte
  bounds under DeepSeek V4's 1M-token context and 384K-token output.
- Raised the root `AGENTS.md` bound from 16 KiB to 64 KiB.
- Changed the project license from Apache-2.0 to MIT.
- Renamed the project from mini-codex to Mini Agent Harness, with the
  `mini-agent` executable and `mini-agent-core` / `mini-agent-cli` crates.
- Interactive reasoning, assistant, tool, and context tags use distinct colors
  on terminals while redirected output remains plain text.
- Plain terminal `ask` output streams assistant deltas without repeating the
  completed answer; redirected and JSON output remain completion-buffered.

## [0.1.0] - 2026-08-24

### Added

- Release automation and cross-platform verification.
- Post-build verification of every release archive checksum.
- `ask`, `status`, `doctor`, and `--version` CLI contracts.
- Per-command help and `--` option delimiters for prompts beginning with `-`.
- Machine-readable output for `ask`, `status`, and `doctor`.
- Bounded portable model/tool harness with explicit event traces.
- Streaming OpenAI Responses adapter, including DeepSeek reasoning events.
- Interactive, one-shot, automatic, and deterministic demo modes.
- Workspace-confined file tools and approval-gated shell execution.
- Managed background processes and bounded large-result handles.
- Cache-friendly context compaction for automatic mode.
