# World state and durable conversation direction

Skills, plugins, and MCP preserve experience that people publish
and share. World state answers a different question: what is true in this
specific execution environment right now?

## Current experiment

At startup the CLI builds one bounded `WorldState` snapshot without executing
discovered commands. It inspects only the current workspace and `PATH` and
records:

- operating system, architecture, workspace, and the actual host shell;
- `default` (8 steps) or `auto` (unlimited steps unless `MINI_AGENT_MAX_STEPS`, compact) loop mode, per-action
  or automatic approval, and the selected native or Docker process sandbox;
- root project markers for Rust, Maven/Gradle Java, Go, Python, Node, and .NET;
- availability of a fixed catalog of common navigation, VCS, build, runtime,
  and package-manager commands;
- workspace Maven and Gradle wrappers.

The catalog is fixed, the workspace path is capped, and the complete rendered
item has an 8 KiB hard limit. No environment values, command output, versions,
or credentials are included.

The snapshot is sent as a typed `Message::Context` item, mapped to a Responses
API `developer` message. It is not concatenated into `instructions`. The first
request therefore keeps project instructions and extension metadata in a
stable prefix while placing local facts immediately before conversation input.
App Server clients can inspect it with `world/state` and refresh it with
`world/refresh`; the core REPL does not duplicate this management dashboard.

Mode changes are also append-only. `/auto` (copilot loop) and `/auto off`
(restore prompts) keep `instructions` byte-stable, update execution limits and
approval, and append an authoritative full world snapshot. `/new` restores the current snapshot after clearing conversation
history. Compaction retains the newest context item next to its summary.

Full snapshots are deliberate at this scale. They avoid requiring old deltas
to reconstruct current authority, and remain far below the item limit. If the
state grows, the next experiment should introduce typed section snapshots and
diff rendering like Codex rather than mutable system-prompt rewriting.

## Durable item boundary

World state makes the need for durable ordered items visible. The JSONL store
lives in the CLI host rather than `mini-agent-core`; it is used by persisted
sessions and preserves
these identities and relationships:

```text
session
  thread
    turn
      ordered item
```

A session identifies one append-only project log. A thread identifies one
conversation lineage. A turn owns one user-initiated run and its settled
status. Message items preserve user input, context snapshots, reasoning,
assistant output, tool proposals, and tool settlements. Goal verifier verdicts
are kept in the Goal workspace and are not replayable conversation messages.

Records have a strictly increasing sequence and bounded payload. Message items
have stable item IDs, thread identity, kind, and timestamp; turn-owned messages
carry a turn ID. A full bounded checkpoint is appended only
after a turn settles and is the sole resume authority. Torn final writes fall
back to the previous checkpoint.

This is conversation persistence, not operation recovery. A process crash can
still occur between tool intent and effect settlement; the interrupted turn is
not replayed. A future Pi-style operation register must make that uncertain
state explicit before safe/unsafe replay policies are introduced. Compaction
lineage, branch indexes, and live operation recovery are not
implemented.

## Goal verifier boundary

A Goal verifier is a separately configured model profile, not a hidden second
voice inside the primary turn. It reads the latest settled checkpoint. The
Runtime Actor associates its result with the Goal, source Thread, verifier
turn, and authoritative checkpoint sequence before applying it. The
`goal/verifier_verdict.md` artifact stores only the source checkpoint sequence
and bounded verifier output.

The verifier uses a separate harness with an empty tool catalog, a zero
tool-call limit, and one model step. It cannot edit the primary transcript,
approve effects, or make tool calls. Verification evaluates arrival criteria
against immutable evidence. This keeps verifier output reproducible and makes
disagreement inspectable rather than allowing an auxiliary model to mutate
live state invisibly.

The verdict is not appended to the primary conversation history. A retry Goal
turn may explicitly read the bounded `goal/verifier_verdict.md` artifact to
address rejected findings, while an ordinary session resume does not inject
the verdict automatically. World state is already part of the settled
checkpoint; the checkpoint sequence remains the authoritative source
reference.
