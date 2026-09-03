# App Server

`mini-agent-app-server` exposes the host backed `Thread` service to a
subprocess client. The current transport is newline delimited JSON over
stdin/stdout. Each input line is one JSON-RPC request; turn progress is emitted
on the same output stream as `turn/event` notifications.

The default binary owns one configured thread per process. Embedded callers can
construct a service with several preconfigured thread identities and address
them through the same methods. The service also exposes bounded thread
list/read/items/list/close, fork and resume, turn result reads, cooperative steering and
interruption, and approval request/response routing. External adapters should
use the same App Server boundary.

Run it after configuring the provider environment:

```sh
cargo run --release -p mini-agent-app-server --bin mini-agent-app-server
```

The first request must negotiate protocol version 1:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientName":"example","clientVersion":"0","capabilities":{},"profile":"interactive","providers":{"model":"openai","tools":"builtin","extensions":"builtin","policy":"builtin"}}}
```

Then start the configured thread and submit a turn:

```json
{"jsonrpc":"2.0","id":2,"method":"thread/start","params":{}}
{"jsonrpc":"2.0","id":3,"method":"turn/start","params":{"threadId":"default","input":{"mode":"start","text":"inspect the workspace"}}}
```

The service returns a correlated response for each request. Requests admitted
to the runtime actor use an action result envelope around the method payload:

```json
{"value":{"status":"started","turn_id":"turn-1"},"actionId":1,"actionSequence":1,"stateRevision":1}
```

`actionId` identifies the admitted action, `actionSequence` is the server-side
admission order, and `stateRevision` is the Runtime version captured when the
result was produced. Actor-rejected actions put the same metadata in the
JSON-RPC error's `data`; requests rejected before admission do not claim an
action. The protocol negotiation and thread index responses remain structural
responses rather than action results.

The service emits `turn/event`, Item, Goal, and settings notifications from one
ordered runtime stream. Core `turn/event` notifications contain the event type,
thread/turn identity, and `sequence` number; that sequence belongs to the Core
Thread event stream and is intentionally distinct from `actionSequence`. The
single runtime stream prevents a ready Goal/settings notification from racing
past an earlier Core notification at the transport boundary. `turn/steer`
validates the supplied active `turnId`;
`turn/interrupt` requests cooperative cancellation; `turn/read` returns the
settled result and messages. When the host runtime is wired with
an `ApprovalBroker`, sensitive tool calls emit an `approval/request`
notification, then emit `approval/resolved` after the client replies with
`approval/respond`. The resolution carries the request ID, action, and final
approved boolean, as well as the optional `turnId` and `callId` for the built-in
Shell path. Clients can correlate `requestId`/`turnId`/`callId` from
`approval/request` through `approval/respond`, `approval/resolved`, and the
matching `turn/event`, without inferring approval from `tool/finished` content.
The App Server worker runs on a dedicated runtime thread, so a synchronous host
approval callback does not block the connection's async transport. The worker
still serializes one Thread at a time while that approval is pending.

`item/started` and `item/completed` carry one bounded `ThreadItem` with
`threadId`, `turnId`, and its lifecycle timestamp. The completed notification
is the authoritative final projection for that item; the same tool `callId` is
used across model, tool-start, tool-completion, and replay projections. These
notifications use the same ordered runtime stream as `turn/event`.

Thread settings and Goal control use the canonical Thread boundary:

- `thread/settings/update` changes the typed `collaborationMode` and optional
  bounded `builtinTools` selection. A changed setting emits one
  `thread/settings/updated` notification with the effective values and the same
  `stateRevision` as the action response.
- `thread/goal/set`, `thread/goal/get`, and `thread/goal/clear` are the only
  Goal lifecycle methods. A Goal turn is persisted as a settled checkpoint
  before its isolated tool-free verifier runs; continuation and retry are then
  scheduled by the Runtime Actor through the existing Thread worker.
- There is no aggregate `workflow/state` method. Clients read Thread settings
  and Thread Goal independently; the former manual `workflow/goal/*` methods
  are removed, so clients cannot submit an arbitrary verifier verdict or
  advance a milestone behind GoalRuntime's lifecycle.

On resume, an unsettled Goal schedules a new ordinary turn, while a settled
checkpoint is re-verified without replaying that turn. Clearing an idle Goal
invalidates any pending verifier association; a late verifier result cannot
advance a cleared or replacement Goal.

`turn/event` and `turn/read` include bounded `items` projections. A ToolCall
uses the model `callId` as its stable item identity; its in-progress and
completed events can therefore be merged by clients. Arguments are recursively
bounded and sensitive keys are redacted, while completed items preserve the
same argument projection and add bounded output. The verifier keeps only the
newest bounded settled-message window. `thread/items/list` returns cursor-bounded
`ThreadItemEntry` values, optionally filtered by `turnId`, from the Session JSONL
projection (or the current in-memory checkpoint when Session is disabled).
Specialized Item variants and generic Artifact APIs remain deferred.

The Rust `LocalAppServerClient` uses the same DTOs and dispatch without stdio,
which lets an embedded frontend migrate to the service boundary before it
spawns a child process.

Host construction is kept outside the service worker: callers use
`HostRuntimeFactory` with an explicit `RuntimeProfile`, then hand the resulting
runtime to the App Server or an edge adapter. The App Server does not discover
extensions or infer capabilities from transport method names.

`initialize.params.profile` is optional for the stdio server. The standalone
server reads the first initialize request before constructing its first Thread,
resolves the requested allowlisted profile plus the bounded workspace profile,
and freezes that selection for the service lifetime. Unavailable profile names
are rejected before Thread construction. For an already constructed embedded
runtime, the request must match the active profile. The initialize result includes the
selected `profile` and structured `capabilityManifest` with enabled,
disabled, extension depth, selected extension names, prompt, and rule source
metadata, precedence, typed rule policy, per-source rule status, resolver
state, bounded source fingerprints, conflicts, and context limits. The
manifest reports the selected bounded extension names and load depth.
For the regular `general` agent, this manifest describes the selected base
prompt fingerprint, independent prompt/rule source admission, and bounded
context limits. A typed base-prompt/output/context preset is a follow-up wire
addition; the current protocol never accepts arbitrary prompt or rule bodies.
The standalone server resolves its workspace profile before creating the
thread factory and reuses that frozen selection for new threads, keeping the
advertised manifest consistent for the lifetime of the process.
Set `MINI_AGENT_PROFILE` before starting the standalone binary to select one
of the three builtin profiles (`interactive`, `ask`, or `auto`); unknown names fail closed before a provider or
thread is created.

`initialize.params.providers` is an optional selector for the four local
provider categories (`model`, `tools`, `extensions`, and `policy`). The
standalone server applies these bounded IDs before constructing the first
Thread; an embedded runtime requires requested IDs to match the frozen
profile. Provider instances, credentials, commands, and paths never cross the
JSON-RPC boundary. The current registry exposes `openai` for models and
`builtin` for the other three categories.
