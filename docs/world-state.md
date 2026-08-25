# World state and durable conversation direction

Skills, plugins, marketplaces, and MCP preserve experience that people publish
and share. World state answers a different question: what is true in this
specific execution environment right now?

## Current experiment

At startup the CLI builds one bounded `WorldState` snapshot without executing
discovered commands. It inspects only the current workspace and `PATH` and
records:

- operating system, architecture, workspace, and the actual host shell;
- `default` or `auto` mode, per-action or automatic approval, and the lack of a
  command sandbox;
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
`/world` shows the same state to the user. `/world refresh` appends a new full
snapshot only when detection changed.

Mode changes are also append-only. `/auto` and `/auto off` keep `instructions`
byte-stable, update execution limits, and append an authoritative full world
snapshot. `/new` restores the current snapshot after clearing conversation
history. Compaction retains the newest context item next to its summary.

Full snapshots are deliberate at this scale. They avoid requiring old deltas
to reconstruct current authority, and remain far below the item limit. If the
state grows, the next experiment should introduce typed section snapshots and
diff rendering like Codex rather than mutable system-prompt rewriting.

## Durable item boundary

World state makes the need for durable ordered items visible. The opt-in JSONL
store now lives in the CLI host rather than `mini-agent-core` and preserves
these identities and relationships:

```text
session
  thread
    turn
      ordered item
```

A session owns provider profiles and creation metadata. A thread owns one
conversation lineage. A turn owns one user-initiated run and its settled
status. Items form the replayable append-only record: user input, context
snapshot, reasoning, assistant output, tool intent, tool settlement,
compaction, mentor insight, and verification.

Every durable item has a stable ID, thread and turn IDs, an increasing
sequence, kind, bounded payload, timestamp, and settled state. Tool intent and
tool settlement remain distinct in settled message items. A full bounded
checkpoint is appended only after a turn settles and is the sole resume
authority. Torn final writes fall back to the previous checkpoint.

This is conversation persistence, not operation recovery. A process crash can
still occur between tool intent and effect settlement; the interrupted turn is
not replayed. A future Pi-style operation register must make that uncertain
state explicit before safe/unsafe replay policies are introduced. Content
hashes, compaction lineage, and branch indexes are also not implemented yet.

## Mentor and verifier boundary

A mentor is a separately configured model profile, not a hidden second voice
inside the primary turn. It reads a settled persisted item range and writes a
derived `mentor_insight` or `verification` item linked to:

- the source session, thread, turn, and item range;
- the mentor provider/model/profile revision;
- the source-range hash and world-state hash;
- a bounded verdict, evidence references, and optional recommendations.

The mentor cannot edit the primary transcript, approve effects, or make tool
calls through the primary harness. Insight may advise a later turn;
verification evaluates arrival criteria against immutable evidence. This
keeps mentor output reproducible and makes disagreement inspectable rather
than allowing an auxiliary model to mutate live state invisibly.

The settled append-only store and cross-process replay now provide the input
boundary for mentor work. Mentor execution should follow only after a source
range can be selected and hashed without provider calls.
