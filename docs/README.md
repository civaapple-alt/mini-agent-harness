# Documentation

This directory contains the current project documentation. Use this file as
the entry point; each document is a standalone topic document and is not an
index of other Markdown files.

## Current architecture and contracts

| Document | Covers |
| --- | --- |
| [Harness framework](harness-framework.md) | Harness responsibilities, Core/Host/Capabilities/App Server layers, and the mini/Codex comparison. |
| [Harness boundaries](harness-boundaries.md) | Ownership boundaries, change admission, loop control, approval, sandbox, and deferred policy decisions. |
| [Harness tool surface](harness-tool-surface.md) | The four default Builtin tools, paged `read_file`, `apply_patch`, and extension/profile rules. |
| [App Server](app-server.md) | JSON-RPC transport, Thread/Turn operations, settings, Goal control, events, approval, and runtime ordering. |
| [World state](world-state.md) | Bounded environment snapshots, durable items, checkpoints, and resume authority. |
| [Harness evidence](harness-evidence.md) | Bounded scenarios, failure/timeout/retry coverage, and evidence gates for harness changes. |

## Usage and operations

| Document | Covers |
| --- | --- |
| [Configuration](configuration.md) | Provider settings, runtime profiles, prompt/rule sources, extensions, and environment variables. |
| [Harness limits](limits.md) | Byte, count, step, timeout, context, and Goal budget limits. |
| [Troubleshooting](troubleshooting.md) | Common setup, provider, shell, tool, session, and runtime issues. |
| [Privacy](privacy.md) | Provider requests, local session data, credentials, MCP, and Goal verification data. |
| [Release process](releasing.md) | Versioning, validation, archives, checksums, and publishing a release. |

## Historical record

| Document | Status |
| --- | --- |
| [Harness lessons history — 2026-08-31](harness-lessons-history-2026-08-31.md) | Frozen historical record; it is not a current specification. |

Current behavior belongs in the topic documents above. Dated architecture
decisions and implementation records are maintained separately under the
repository's agent notes; they should not be copied into this index.
