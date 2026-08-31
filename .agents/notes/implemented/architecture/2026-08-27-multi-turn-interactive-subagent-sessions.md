# Multi-turn Subagent Sessions via Durable Resumption

Status: historical — child-session path retired from the current mainline

## Current status update (2026-08-31)

The durable child-session behavior described here was retired with the
non-mainline subagent implementation. Current session continuity is owned by
the App Server/Host session boundary; this note preserves the former bounded
follow-up and persistence design for historical reference.

## Decision

Phase 2A was implemented through durable child sessions:

1. spawn_agent launches a child ask turn and persists it by default.
2. The returned session_id identifies the child checkpoint.
3. send_subagent_message launches a fresh child process with that ID.
4. The child resumes the latest settled checkpoint and appends the follow-up
   result.

The child uses --max-steps 50 and a 300-second default timeout, both bounded by
the existing CLI limits. Parent checkpoints receive the bounded tool result;
they do not inline the child’s complete tool history.

## Persistence and paths

The child conversation is stored under
~/.mini-agent/sessions/<encoded-workspace-path>/<child-id>/session.jsonl.
Parent-side lineage records are stored under
<parent-session-dir>/subagents/<child-id>/meta.json and output.json. Without a
durable parent session, the parent-side tree is not recorded.

## Security boundary

The child inherits the parent security preset and sandbox selection. A separate
process is not workspace isolation. Native mode provides process-tree cleanup;
Docker isolation requires explicit --sandbox docker.

## Not shipped

Phase 2B (streaming ACP/JSON-RPC, live deltas, and mid-turn interrupts) is not
implemented. This note records the shipped session-backed follow-up behavior,
not an ACP protocol contract.

## Verification

The implementation is in subagent.rs, with CLI tests for follow-up command
construction, persistence metadata, and result handling.
