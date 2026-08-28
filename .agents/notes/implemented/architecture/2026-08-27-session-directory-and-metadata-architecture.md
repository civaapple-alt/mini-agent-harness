# Session Directory and Metadata Architecture

Status: implemented

## Decision

Durable sessions are host-owned. The CLI stores them under:

    ~/.mini-agent/sessions/<encoded-workspace-path>/<session-id>/

The project-local .agents/sessions/ layout is read only as a legacy
compatibility format when resuming or listing old sessions.

The current session directory contains these files when the corresponding
behavior is used:

    session.jsonl          append-only turn/checkpoint/result records
    summary.json           bounded discovery metadata
    signals.json           bounded step/tool counters
    prompt_context.json    OS, workspace, and AGENTS.md snapshot
    session.lock           short-lived active-session lock
    attachments/           persisted image payloads when needed
    subagents/<child-id>/  meta.json and output.json for child runs
    plan.md                living plan, when Plan Mode is enabled
    plan_mode.json         Plan Mode state
    goal/                  goal state, plan copies, and verifier verdict

summary.json and signals.json are written atomically. session.jsonl records
settled checkpoints and is recovered from a torn final line. Session IDs and
workspace-derived paths are validated before access.

## Persistence defaults

Interactive, one-shot ask, and auto sessions always persist settled records;
there is no persistence opt-out. Subagent spawn_agent uses persistence by default so
send_subagent_message can resume a settled child session.

## Parent/child records

Subagent records are children of the durable parent session:

    <parent-session-dir>/subagents/<child-id>/meta.json
    <parent-session-dir>/subagents/<child-id>/output.json

The child’s own durable conversation is resolved through its normal
~/.mini-agent/sessions/<workspace>/<child-id>/session.jsonl path. The parent
checkpoint stores the bounded tool result, not the child’s full intermediate
history.

## Scope boundary

The repository does not currently ship a .agents/bundled asset manifest,
resources_state.json, system_prompt.txt, a compaction archive, a terminal log
directory, automatic root/child garbage collection, or an ACP daemon. These
are possible future designs, not current filesystem contracts.

## Verification

The implementation lives in session.rs, goal.rs, and subagent.rs, with
session, recovery, goal, and subagent tests in the CLI crate.
