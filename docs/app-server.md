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
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientName":"example","clientVersion":"0","capabilities":{},"providers":{"model":"openai","tools":"builtin","extensions":"builtin","policy":"builtin"}}}
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
`approval/respond`. The request and resolution carry typed access and approval
scope, bounded workspace identity, an action class, and the optional `turnId`
and `callId` for the built-in Shell path. Clients can correlate
`requestId`/`turnId`/`callId` from `approval/request` through
`approval/respond`, `approval/resolved`, and the matching `turn/event`, without
inferring approval from `tool/finished` content. `full_machine` means
machine-wide path scope, not allow-all; security Deny, Plan locks, tool
availability, and high-risk confirmation remain independent gates.
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

Host construction is kept outside the service worker: callers use the Host's
bounded internal runtime composition and hand the resulting runtime to the App
Server or an edge adapter. The App Server does not discover extensions or infer
capabilities from transport method names. The initialize result includes a
structured, non-secret `capabilityManifest` with enabled/disabled capability,
provider, prompt/rule source, typed policy, source status, bounded fingerprints,
and context-limit metadata. It does not expose a selectable Profile identity or
accept a Profile-shaped startup input.

`initialize.params.providers` is an optional selector for the four local
provider categories (`model`, `tools`, `extensions`, and `policy`). The
standalone server applies these bounded IDs before constructing the first
Thread; an embedded runtime requires requested IDs to match the frozen
composition. Provider instances, credentials, commands, and paths never cross the
JSON-RPC boundary. The current registry exposes `openai` for models and
`builtin` for the other three categories.

## Public JSON-RPC interface

The following is the complete public JSON-RPC surface of protocol version 1.
The executable contract is defined by
`crates/mini-agent-app-server-protocol`; this section explains the direction,
lifecycle, and fields that an SDK or Web Studio client needs. JSON object
fields use `camelCase`; enum values use `snake_case` unless noted otherwise.

### Handshake

| Method | Direction | Parameters / result |
| --- | --- | --- |
| `initialize` | client request | Required `protocolVersion`, `clientName`, `clientVersion`; optional `capabilities` (`approvals`, `notifications`) and `providers` (`model`, `tools`, `extensions`, `policy`). Returns `protocolVersion`, server identity, `capabilities`, and the secret-free `capabilityManifest`. |
| `initialized` | client notification | No parameters and no response. Enables all methods after a successful `initialize`. |

`initialize` must be the first request. Clients should send `initialized`
after accepting its result. A request received before that notification is
rejected. The server currently accepts protocol version `1` only.

### Method index

Every row below is a request unless marked as a notification. Request results
for runtime actions are normally wrapped as `ActionResult`; the direct
structural exceptions are handshake responses, `thread/list`, and an existing
Thread returned by `thread/start`.

#### Thread and session lifecycle

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `thread/start` | Optional `threadId` | Starts or returns the selected Thread; result contains `threadId`. |
| `thread/list` | Optional `cursor`, `limit` | Returns `{data: [threadId], nextCursor}`. This is a bounded index query. |
| `thread/fork` | `sourceThreadId`, `newThreadId` | Creates a Thread from the source checkpoint; result contains the new `threadId`. |
| `thread/resume` | `threadId`, `checkpoint` | Installs the supplied bounded `ThreadReadResult` checkpoint and resumes the Thread identity. |
| `thread/read` | `threadId` | Returns status, messages, context revision, turn counters, last turn, and next event sequence. |
| `thread/close` | `threadId` | Closes the Thread; the action value is `{closed: true}`. |
| `thread/items/list` | `threadId`; optional `turnId`, `cursor`, `limit`, `sortDirection` | Returns cursor-bounded `data` entries, `nextCursor`, and `backwardsCursor`. |
| `session/info` | No parameters | Returns the current session ID, Thread ID, session path, and `resumed` flag. |

`thread/resume` is a controlled checkpoint install, not a second persistence
format. The Session store and App Server remain the authorities for the
active Thread; clients should use `thread/read` and `thread/items/list` for
history instead of reading session files directly.

#### Turn execution

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `turn/start` | `threadId`, `input: {mode, text}` | Starts one turn and returns `turnId` and status. Current public modes are `start` and `start_if_idle`; other modes are rejected on this method. |
| `turn/read` | `turnId` | Returns status, optional `stopReason`, optional `finalText`, step count, bounded messages, projected items, and optional error. |
| `turn/steer` | `threadId`, `turnId`, `text` | Sends cooperative steering input to the active turn. The supplied `turnId` must be active. |
| `turn/interrupt` | `threadId`, `turnId` | Requests cooperative cancellation and returns `{accepted: true}` when admitted. |

`turn/start` is asynchronous. Clients should render `turn/event` and Item
notifications while the turn is running, then use `turn/read` for the settled
result. Steering and interruption are requests to the runtime; they do not
force an immediate stop before the runtime reaches a cancellation boundary.

#### Thread settings, Plan, and Goal

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `thread/settings/update` | `threadId`, `collaborationMode: {mode}`, optional `builtinTools: [name]` | Updates the Thread's `default` or `plan` mode and optional bounded Builtin tool selection. Emits `thread/settings/updated`. |
| `thread/goal/set` | `threadId`; optional `objective`, `status`, `tokenBudget` | Sets or replaces a Goal subject to lifecycle checks; returns the public Goal projection and emits `thread/goal/updated`. A running Goal must be cleared before replacement. |
| `thread/goal/get` | `threadId` | Returns `{goal}` where `goal` may be `null`. |
| `thread/goal/clear` | `threadId` | Clears the Goal and returns `{cleared: true|false}`; emits `thread/goal/cleared` when applicable. |

Goal status values are `active`, `paused`, `blocked`, `usage_limited`,
`budget_limited`, and `complete`. Goal continuation, verification, pause,
resume, and checkpoint association belong to GoalRuntime; clients do not
submit verifier verdicts or advance milestones directly.

#### Runtime management

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `world/state` | No parameters | Returns the current workspace, structured status, status lines, and bounded model context. |
| `world/refresh` | No parameters | Refreshes the world and returns `{changed, state}`. |
| `world/set_execution` | `access`, `approval` | Sets execution scope and returns `{changed, state}`. `access` is `project` or `full_machine`; `approval` is `per_action`, `current_session`, or `current_project`. |
| `mcp/status` | No parameters | Returns enabled/inactive servers, tool count, and whether retry is available. |
| `mcp/retry` | No parameters | Retries MCP setup and returns enabled/inactive servers, diagnostics, and tool count. |

`world/set_execution` changes runtime configuration, not the security order.
`full_machine` expands the candidate filesystem range but does not mean
allow-all: Deny, Plan locks, tool availability, and high-risk confirmation
still apply. The approval lifetime is scoped to the current action, session,
or project according to the selected value.

#### Approval response

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `approval/respond` | `requestId`, `decision` (`approve` or `deny`), `access`, `approval`, optional `reason` | Resolves one pending approval. The server emits `approval/resolved`; it does not return a second approval authority to the client. |

The response must preserve the access and approval scope selected by the
client. An approval is matched against the request's project, workspace,
workspace revision, action class, and path scope. A changed workspace
revision, project switch, policy change, or revocation invalidates a prior
project/session reuse decision.

### Server notifications

Notifications have no JSON-RPC `id` and never receive a response. They are
emitted on one ordered runtime stream.

| Notification | Payload highlights | Use |
| --- | --- | --- |
| `turn/event` | `threadId`, optional `turnId`, Core `sequence`, bounded `items`, `event` | Ordered Core execution events, including turn settlement. |
| `item/started` | `threadId`, `turnId`, `item`, `startedAtMs` | One ThreadItem becomes visible. |
| `item/completed` | `threadId`, `turnId`, `item`, `completedAtMs` | Authoritative final projection for that item. |
| `approval/request` | Request identity, project/workspace/revision, action class, summary, path scope, access, allowed approval modes, risk | Requests a user decision for a sensitive action. |
| `approval/resolved` | Request identity, outcome, selected approval, and the original scope metadata | Reports `approved`, `denied`, `expired`, `revoked`, or `unavailable`. |
| `thread/settings/updated` | `threadId`, effective mode, Builtin tools, `stateRevision` | Projects a settings change. |
| `thread/goal/updated` | `threadId`, optional `turnId`, Goal projection | Projects Goal creation, update, or runtime progress. |
| `thread/goal/cleared` | `threadId` | Projects Goal removal. |

`sequence` is the Core Thread event sequence. `actionSequence` in an action
response is the App Server admission order; they are different counters and
must not be merged by clients. The stable ToolCall item identity is the model
`callId`, which lets a client merge model, start, completion, and replay
projections.

### Common response and error rules

The JSON-RPC envelope is `{"jsonrpc":"2.0","id":...,"result":...}` or
`{"jsonrpc":"2.0","id":...,"error":...}`. An admitted runtime action
uses:

```json
{
  "value": {},
  "actionId": 1,
  "actionSequence": 1,
  "stateRevision": 1
}
```

The metadata identifies the admitted action, its server ordering, and the
Runtime revision observed when the result was produced. Actor-rejected
actions carry the same metadata in JSON-RPC error `data`; requests rejected
before admission do not claim an action.

The standard error codes currently used are:

| Code | Meaning |
| ---: | --- |
| `-32700` | Parse error. |
| `-32600` | Invalid JSON-RPC request or protocol version. |
| `-32601` | Method not found, including removed legacy methods. |
| `-32602` | Invalid or incomplete parameters. |
| `-32000` | Runtime, capability, approval, or management failure. |

All messages, tool arguments, tool output, event lists, item projections,
cursor pages, and model context are bounded. Sensitive approval and item
fields are redacted according to the Host policy.

### Removed and deferred surface

The following are intentionally not public protocol methods:

- `workflow/state` and the former `workflow/goal/*` and `workflow/plan/set`
  methods; use Thread settings and Thread Goal methods.
- Profile or `turbomode` selection; startup provider selection is limited to
  the bounded `providers` selectors in `initialize`.
- Specialized Item variants and generic Artifact APIs; these remain deferred
  until an independent contract and evidence set is accepted.

The protocol list in this document must be updated together with the constants
and DTOs in `mini-agent-app-server-protocol`. It must not document private
Host constructors, `LocalAppServerClient` helper methods, or provider
credentials as if they were wire interfaces.
