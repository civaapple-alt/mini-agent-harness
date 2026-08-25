# Changelog

All notable changes to Mini Agent Harness are documented here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Minimal project-scoped Agent Skills and Agent Plugins 1.0.0 discovery with
  progressive skill disclosure and approval-gated stdio MCP tools via RMCP.
- Cloned skill collections, Claude/Grok plugins and explicitly selected local
  plugin marketplaces, including compatible plugin-agent instructions.
- Standalone project MCP configuration and approval-gated streamable HTTP MCP
  with bounded headers, environment placeholders, and connection timeouts.
- Individual immediate skills can be selected from cloned marketplaces without
  enabling their containing plugin bundle.
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

### Fixed

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
