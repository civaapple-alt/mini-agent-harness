# Subprocess CLI Subagent Execution

Status: historical — subprocess subagent path retired from the current mainline

## Current status update (2026-08-31)

This note records the former subprocess-subagent design. The current CLI no
longer contains the `subagent.rs` implementation; the mainline now uses the
CLI → App Server → Host → Core path, while ACP/subagent experiments remain
outside the supported runtime. The bounded process and result-recording
constraints below are retained as historical design rationale only.

## Decision

Subagent delegation used a bounded child process rather than an in-process
multi-tenant scheduler. spawn_agent launches the current executable with:

    mini-agent ask <prompt> --json --auto --max-steps 50

It forwards the parent security preset and sandbox selection, applies a
10–600 second timeout (300 seconds by default), parses the bounded JSON result,
and returns the child’s final output to the parent. A persistent child receives
an ID of the form sub-<timestamp>-<task-name> and writes its own durable
session. The parent records bounded meta.json and output.json files under its
session’s subagents/ directory.

The child process gives crash and memory isolation at the OS-process level, but
it still shares the workspace unless --sandbox docker is explicitly selected.
Native process cleanup is platform-specific: Windows uses a Job Object; macOS
and Linux use process-group termination.

## Multi-turn follow-up

send_subagent_message starts another mini-agent ask process with the existing
--session-id. It resumes the child’s settled checkpoint and uses the same
50-step and timeout bounds. list_subagents reads the parent’s child metadata.

## Non-goals

There is no mini-agent app-server, ACP daemon, JSON-RPC streaming transport,
live token multiplexing, or in-process scheduler in the current release.
Streaming ACP remains a separate proposal requiring its own protocol and
integration tests.

## Verification

The implementation is in subagent.rs. The CLI tests cover schema, persistence,
metadata, follow-up, timeout, and structured result handling. End-to-end
provider calls are not part of the offline test contract.
