# Subagent Result Records and Session Lineage

Status: implemented

## Decision

The current implementation keeps parent and child model histories separate.
When a durable parent invokes spawn_agent, the parent records:

    <parent-session-dir>/subagents/<child-id>/meta.json
    <parent-session-dir>/subagents/<child-id>/output.json

meta.json contains the task name, agent type, optional persona, timestamps,
duration, step count, exit code, status, and optional review statistics.
output.json contains the child’s final output and error text. The parent
receives a bounded textual tool result rather than raw child events.

The child’s full durable conversation, when persistence is enabled, remains in
its own normal session directory. list_subagents lists recorded child IDs and
statuses for the current durable parent.

## Boundaries

This is session lineage and result accounting, not recursive trace aggregation.
The current trace commands do not recursively replay child event streams, do
not roll up provider token usage, and do not maintain a separate
.agents/traces/ hierarchy. There is no automatic garbage collection or
branch-on-follow-up operation for child sessions.

Subagent processes inherit the parent workspace and selected security/sandbox
settings. Process separation alone does not provide filesystem isolation.

## Verification

The implementation is in subagent.rs, with tests for parent metadata, structured
output, and child listing.
