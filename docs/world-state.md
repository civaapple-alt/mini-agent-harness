# World state and durable session state

World state describes the current execution environment. Session state records
what a Thread has already done. They have different owners and different
lifetimes.

## World state

Host builds a bounded `WorldState` from the configured workspace roots and the
local host. It records the platform, workspace markers, host shell, selected
access scope, approval policy, sandbox kind, and a fixed command catalog. It
does not include environment values, command output, provider credentials, or
an unbounded directory scan.

### Local environment detection

Host checks a fixed command catalog against `PATH`. On macOS it also checks
`brew`, `swift`, `swiftc`, `xcrun`, and `xcodebuild`. A command found on `PATH`
is normally reported as available when it is an executable file. `xcodebuild`
must additionally complete `xcodebuild -version` within two seconds, so the
Command Line Tools placeholder does not imply a full Xcode installation.

When `python3` is present, Host runs `python3 -m pip --version` with a two-second
deadline and no network access. The `python3 -m pip` capability is unavailable
when the module is missing, the command fails, or the deadline expires. Host
does not install packages.

Blender is detected either as a `blender` executable on `PATH` or at the fixed
macOS CLI locations `/Applications/Blender.app/Contents/MacOS/Blender` and
`~/Applications/Blender.app/Contents/MacOS/Blender`. Host does not search other
application directories. The bounded `available_applications` status and model
context include the resolved CLI path, including when Blender is available
only through an application bundle. A `.blend` file directly inside the
primary or associated workspace root adds the `blender` project marker;
nested directories are not scanned. The marker check inspects at most 128 root
entries.

These checks describe local capabilities only. They do not install software,
grant tool permissions, or expand the existing execution sandbox. `world/state`
and `world/refresh` expose the same bounded projection; no separate RPC method
is added.

The App Server exposes the current projection through `world/state`. A client
can request `world/refresh` after the workspace or available commands change.
`world/set_execution` updates the access scope and approval policy through the
runtime control plane. It does not bypass the admission order: deny rules,
Plan-mode source mutation locks, tool availability, sandbox checks, and
high-risk approval still apply.

The Host contributes world state to bounded model context as a replaceable
context slot. A root-set change replaces that slot after runtime rebinding.
Ordinary Turns do not append another copy. Gateway-managed Session attachments
are a separate, read-only root. The model receives logical Session capabilities
instead of the raw Session directory or its sidecar files.

## Durable session state

App Server and the Session store own durable conversation state. A Session
contains an append-only `session.jsonl` log, complete settled checkpoints, and
bounded sidecars such as Thread settings, Goal state, approval evidence,
attachments, and Notebook data. The canonical relationship is:

```text
Session
  └─ Thread
       └─ Turn
            └─ ordered item
```

The settled Session checkpoint stores conversation context for a later Turn or
fork. The execution checkpoint stores the current logical Turn and can only be
continued by an explicit `turn/resume` against its exact Turn ID and checkpoint
sequence. Restart never resumes automatically. A started tool call without a
durable outcome blocks both resume and new Turns until an operator records a
bounded `turn/reconcile` decision; Mini Agent never retries an uncertain side
effect on its own. Already recorded tool outcomes are reused as-is.

Two bounded Session sidecars keep metadata out of model input. The Context
Manifest records source identity, fingerprint, scope, permission basis, and
injection reason without storing the injected body. The event replay ring keeps
at most 512 lifecycle summaries across App Server restarts; it omits text
deltas, prompts, tool arguments, and tool output. Canonical Thread and Item
projections remain the source for repairing replay gaps.

The Session store also persists control-plane state that is not conversation
history:

- Child operation records retain their queue, running, approval, completion,
  failure, cancellation, retry attempt, and scheduling metadata.
- `notebook.json` stores bounded Session facts. Resume injects only a bounded
  summary. Full entries are available through explicit Notebook reads.
- Each settled Turn retains a bounded presentation projection of its requested
  workflow and skill-group/skill-load milestones. It is an App Server display
  observation, anchored to completed assistant segments; it neither changes
  Core conversation history nor replays an interrupted Turn.
- Background Shell tasks and scheduled delay markers are bounded,
  runtime-scoped managers. They are not conversation history and do not create
  a second Core loop. Closing that runtime cleans up its local task state.

An exact `session/fork` creates an independent child Session from the newest
complete checkpoint. The child receives its own runtime, event stream, and
approval flow. Gateway recovery reconciles persisted operation records with
live runtimes. It rebinds known work instead of fabricating child history or
starting a duplicate operation.

## Goal verifier

Goal verification is a separate, tool-free model run against the latest
settled checkpoint. The verifier cannot modify primary conversation history,
approve effects, or call tools. Its bounded verdict is stored in the Goal area
and associated with the source Thread, Turn, and checkpoint sequence. App
Server emits Goal lifecycle notifications so clients can show progress without
inventing their own verifier state.

For Session methods, operation fields, and replay rules, see [App
Server](app-server.md). For local retention and removal guidance, see [Data and
privacy](privacy.md).
