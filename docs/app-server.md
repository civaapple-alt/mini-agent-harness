# App Server

`mini-agent-app-server` exposes the Host-backed `Thread` service to a
subprocess client. It is the public control-plane boundary around Core's thin,
bounded Agent Loop: Core owns model/tool contracts, the loop, limits, stop
classification, and execution events; Host and Capabilities own admission,
approval, and sandboxed side effects; App Server owns the durable Session
boundary. The current transport is newline-delimited JSON over stdin/stdout.
Each input line is one JSON-RPC request; turn progress is emitted on the same
output stream as `turn/event` notifications.

The default binary owns one configured thread per process. Embedded callers can
construct a service with several preconfigured thread identities and address
them through the same methods. The service also exposes bounded thread
list/read/items/list/close, fork and resume, turn result reads, cooperative steering and
interruption, and approval request/response routing. External adapters should
use the same App Server boundary.

Local processes that must outlive one Turn use the Shell tool's explicit background
mode. `background-task/list`, `background-task/read`, `background-task/logs`,
`background-task/stop`, and `background-task/restart` expose the same bounded
control surface to clients. The App Server runtime owns the authoritative
`BackgroundShellTask` records and cleans their process groups when the owning
Thread runtime closes. `runtime/status` remains scoped to the current Turn.
Child Sessions can read their parent's task projection but cannot control it.
Remote waits such as GitHub Actions are not background Shell tasks. `scheduled_task`
creates a bounded delay marker only: it does not hold or end the current Turn, wake or
resume a Thread, or query the remote task. Use it only when a later Turn will be started
explicitly by the user or Host and needs a record of when to check. For a local process,
use the Shell task's `status` and `logs` actions instead.

## Session log maintenance

The executable has two local maintenance commands for the Gateway. They bypass normal
runtime startup and call the Capabilities Session parser directly:

```sh
mini-agent-app-server doctor --json
mini-agent-app-server doctor repair --session-id <id> --json
```

Run them with the Project workspace as the current directory. The scan reads at most
8,192 directory entries, examines at most 4,096 Session directories, and returns at most
256 findings. It reports inspection,
history integrity, and recovery availability as separate states. The report contains
no prompts, tool arguments, or tool results.

Repair takes the Session lock without reclaiming stale locks, re-reads the log, and
accepts only an incomplete final record after a valid settled checkpoint. It writes
and syncs the original log under `~/.mini-agent/recovery-backups/` before truncating
the tail. It does not repair sequence gaps, `recovery_gap`, invalid complete records,
or missing checkpoints. A locked Session remains unverified.

These commands are local maintenance entry points, not JSON-RPC methods. The Gateway
resolves a registered Project ID to its primary workspace and invokes the executable
there. It does not accept a browser-supplied path or parse Session JSONL itself.

Ordered `turn/event` notifications preserve `thread_id`, `turn_id`, sequence,
and bounded ThreadItem identity. Each model response also carries an optional
`item_id` shared by its `model_started`, reasoning/text delta, and
`model_responded` events; the projected reasoning item uses
`<item_id>:reasoning`. In particular, a live context-compaction start/finish
pair shares one unique `item_id`; local redacted trace records retain these
identities without retaining model or tool payloads.

The main execution path is `Core → Host → App Server`: Core owns the turn loop
and records the tool outcome, Host owns admission, approval, concrete execution,
and typed outcome propagation, and App Server serializes the settled event and
item projections. `ToolExecutionStatus` is carried as structured state; clients
must not infer status from the human-readable tool `content`.

The App Server can start before a model is configured. Provider credentials and
defaults live in the Host model catalog, which is shared by CLI and Web Studio.
Configure that catalog in Web Studio before starting a provider-backed Turn.

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

The service emits `turn/event`, Item, Goal, settings, runtime-status, and
workflow-lifecycle notifications from one ordered runtime stream. Core
`turn/event` notifications contain the event type,
thread/turn identity, and `sequence` number; that sequence belongs to the Core
Thread event stream and is intentionally distinct from `actionSequence`. The
single runtime stream prevents a ready Goal/settings notification from racing
past an earlier Core notification at the transport boundary. `turn/steer`
validates the supplied active `turnId`;
`turn/interrupt` requests cooperative cancellation and acknowledges admission,
not settlement; `turn/read` returns the settled result and messages. A
successful interrupt publishes `runtime/status` with `phase=stopping` and
keeps the same `turnId` until the worker publishes the terminal result. When
the host runtime is wired with
an `ApprovalBroker`, sensitive tool calls emit an `approval/request`
notification, then emit `approval/resolved` after the client replies with
`approval/respond`. The request carries typed access, policy, a structured
action key, action summary, bounded target paths, and allowed grant scopes; the
resolution carries the same action summary, its outcome, selected grant scope,
and optional reason. For example, `apply_patch` can report counts for added,
modified, and deleted files without exposing the complete patch in the
approval card. `action` remains the authorization identity and must not be
replaced by the human-facing summary. Both carry bounded workspace identity
and the optional `turnId` and `callId` for the built-in Shell path.
Clients can correlate
`requestId`/`turnId`/`callId` from `approval/request` through
`approval/respond`, `approval/resolved`, and the matching `turn/event`, without
inferring approval from `tool/finished` content. `full_machine` means
machine-wide path scope, not allow-all; security Deny, Plan-mode source-file
mutation locks, tool availability, and high-risk confirmation remain independent
gates. Shell is still admitted according to the selected execution policy and
does not receive a separate Plan-mode read-only restriction.
The App Server worker runs on a dedicated runtime thread, so a synchronous host
approval callback does not block the connection's async transport. The JSON-RPC
transport multiplexes request handling and resolves `approval/respond` through
a control-plane fast path even when a `turn/steer` request is waiting for the
worker. The worker still serializes one Thread at a time while that approval is
pending; other runtime actions remain ordered behind it.

Embedded callers should call the local client's or connection's explicit
`shutdown()` after an idle runtime when they need to release a SessionStore lock
for an in-process restart. Shutdown is not a public JSON-RPC method: an active
turn returns `Busy`, while an idle worker acknowledges shutdown and drops its
runtime-owned SessionStore before the next process can resume it.

`item/started` and `item/completed` carry one bounded `ThreadItem` with
`threadId`, `turnId`, and its lifecycle timestamp. The completed notification
is the authoritative final projection for that item; the same tool `callId` is
used across model, tool-start, tool-completion, and replay projections. These
notifications use the same ordered runtime stream as `turn/event`.

For a `ToolCall` item, `status` is the lifecycle projection
(`inProgress`, `completed`, or `failed`) and optional `outcome` preserves the
Core `ToolExecutionStatus` (`completed`, `failed`, `needs_approval`, `deferred`,
or `retryable`). Keep these fields separate. A client may use `status` for
rendering lifecycle and `outcome` for the tool's policy/execution result; it
must not reconstruct the latter from `output` text. Older persisted items may
omit `outcome` and remain valid.

Realtime context compaction also carries an independent item identity through
the `EventEnvelope` and its projected `ContextCompaction` item. The start and
finish events reuse that ID, so clients can merge adjacent lifecycle updates;
older envelopes without the optional identity retain the deterministic fallback.
The local redacted trace keeps the same bounded `item_id` for correlation without
retaining model or tool payloads.

Thread settings and Goal control use the canonical Thread boundary:

- `thread/settings/update` changes the typed `collaborationMode`, optional
  bounded `builtinTools` selection, and optional `continuationMode` (`manual`
  or `continuous`). A changed setting emits one
  `thread/settings/updated` notification with the effective values and the same
  `stateRevision` as the action response. When a SessionStore is enabled, an
  explicit continuation choice is atomically persisted in the session-owned
  `thread_settings.json` sidecar, tagged with the current `thread_id`; startup
  restores it before the first client-visible settings read.
- `thread/goal/set`, `thread/goal/get`, and `thread/goal/clear` are the only
  Goal lifecycle methods. A Goal turn is persisted as a settled checkpoint
  before its isolated tool-free verifier runs; continuation and retry are then
  scheduled by the Runtime Actor through the existing Thread worker. Goal action
  results and `thread/goal/updated|cleared` notifications carry the same
  `stateRevision` as the Runtime Actor state; Goal updates emitted during a
  turn use the revision that the turn is about to commit.
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
Each entry may include the structured `turnSource` associated with its Turn.
`child_wakeup` identifies an automatic parent continuation triggered by child
reports or terminal updates; clients can label it without rendering its
synthetic continuation input as a user message. The source is stored in the
existing `turn_started.presentation.turnSource` Session record and restored
when the App Server rebuilds the item projection. It does not change the model
input or create a synthetic user message.
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
fields use `camelCase`; enum values below are shown using their exact wire
spelling.

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
| `session/fork` | `sourceThreadId`, `newThreadId`; optional `contextPolicy` (`exact` or explicit `compact`, default `exact`), `operationId`, `operationAttempt`, `operationPrompt`, `operationGroupId`, `executionMode`, `groupSequence` | Persists a new Session from the latest settled checkpoint, returning child/parent IDs, bounded context sizes, and the compaction method. Fork metadata is a bounded operation projection only; it does not make Core a scheduler. |
| `session/control` | `threadId`, `action` (`read`, `freeze`, `freeze_settled`, `resume`, `resume_settled`); state changes require a bounded `requestId` | Reads or transitions the durable Session-control state (`running`, `freezing`, `frozen`, `resuming`). Freeze intent is persisted before the Gateway interrupts the parent and active children. Only the matching request may settle a freeze or resume. |
| `child/task` | `threadId`, `parentThreadId`, `operationId`, `attempt`, `action`; action-specific bounded report, prompt, report/request identity, and for pause/active cancellation the `turnId` | Persists a validated child report or operation control. Queued updates and cancellation, follow-up, pause/resume, active cancellation, retry, and start-failure actions validate parent lineage and operation attempt. `queue_follow_up` is idempotent by `requestId`, allows one pending instruction, and allocates the next attempt on success. |
| `session/notebook/read` | `threadId`, optional `scope` (`self` or `parent`) | Reads the current Session notebook or a Host-validated parent snapshot. Parent scope is read-only and cannot select an arbitrary Session or path. |
| `session/notebook/write` | `threadId`, `key`, `content`, optional `append`, `importance` (`critical`, `high`, `normal`, `temporary`), `keywords`, and bounded `evidence` | Upserts the current Session's bounded Notebook entry and returns the new snapshot. Evidence is bounded caller-supplied provenance metadata; subject normalization and truncation are applied, but Git/file-system verification is not claimed. |
| `session/notebook/forget` | `threadId`, `key` | Removes one current-Session entry and advances the Notebook revision without rewriting checkpoint history. |

#### Background Shell tasks

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `background-task/list` | `threadId` | Returns an action result containing bounded local Shell task snapshots. |
| `background-task/read` | `threadId`, `taskId` | Returns one task snapshot. |
| `background-task/logs` | `threadId`, `taskId` | Returns the bounded log tail; logs are not inserted into the default information stream. |
| `background-task/stop` | `threadId`, `taskId` | Stops the complete local process group. Child Sessions receive a read-only error. |
| `background-task/restart` | `threadId`, `taskId` | Recreates the same task from its pinned command and working directory. |
| `background-task/updated` | notification | Carries one bounded task snapshot after an explicit control action. |

#### Scheduled delay markers

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `scheduled-task/list` | `threadId` | Returns bounded delay markers owned by the Thread runtime; reading refreshes due markers to `ready`. |
| `scheduled-task/read` | `threadId`, `taskId` | Returns one marker with `scheduled`, `ready`, or `cancelled` state. |
| `scheduled-task/cancel` | `threadId`, `taskId` | Cancels the local marker. It does not cancel a remote Action, build, or deployment. Child Sessions receive a read-only error. |
| `scheduled-task/updated` | notification | Carries one marker after cancellation. |

The model-facing `scheduled_task` tool supports only `create`, `list`, `read`, and
`cancel`; delay is bounded to 1 second through 24 hours. A marker becomes `ready` when
the runtime next reads or updates it; there is no timer worker. It does not execute a
command, end the current Turn, start a later Turn, or provide a remote-provider adapter.
A future Turn must be started explicitly and call the relevant remote status tool.

Successful `session/notebook/write` and `session/notebook/forget` operations also
emit `session/notebook/updated` with `threadId`, `revision`, and bounded
`changedKeys`. The notification is an invalidation signal; clients re-read the
Notebook projection and must not expect content in the event.

`thread/resume` is a controlled checkpoint install, not a second persistence
format. The Session store and App Server remain the authorities for the
active Thread; clients should use `thread/read` and `thread/items/list` for
history instead of reading session files directly.

The `thread_settings.json` sidecar is a bounded control-plane projection, not
conversation history. It contains only a schema version, the owning Thread ID,
and the explicit continuation mode. Web/Gateway integrations may read this
projection for a read-only session listing, but mutation remains an App Server
`thread/settings/update` operation.

Plan mode state is persisted separately in the Session-owned `plan_mode.json`.
It contains the active state, bounded Plan artifact paths, and
`review_pending`. A completed Plan Turn sets that flag; starting another Plan
Turn or disabling Plan clears it. Web Studio can therefore restore the
“continue planning / start implementation” confirmation after reload or Session
restore. This is a workflow/UI projection, not a second authority; mode changes
still go through `thread/settings/update`.

#### Workspace roots and Session capabilities

`world/state` reports the primary Project root and associated roots with an
explicit role and access (`primary/read_write`, `associated/read_only`, or
`associated/read_write`). The App Server never treats a Session directory as a
normal `workspace_root`. The Host instead injects a stable logical
`session_capabilities` context containing the Plan, Goal, Notebook, and
current-turn attachment capabilities without an absolute Session path.

`plan.md` and `goal/...` are Host-resolved logical aliases. Notebook state is
available only through the bounded Notebook methods/tools. A Gateway-owned
attachment root may be read by `read_file` or `read_image`, but it cannot be
modified. `session.jsonl`, `summary.json`, approval evidence, and other
SessionStore sidecars are not opened through ordinary file tools; history must
use the bounded Session APIs.

The Host maintains separate read/write/Session-root sets and replaces the
`world_state` context slot when the Project root set changes. It does not keep
appending snapshots on every Turn. Session IDs, attachment IDs, mtimes, and
sidecar sizes are excluded from the stable prompt prefix; current-turn
attachment references and explicit external paths remain dynamic input.

#### Turn execution

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `turn/start` | `threadId`, `input: {mode, text, selectedSkills?, workflow?}`, optional `operationId`, `operationAttempt`, `operationAttemptKind`, `turnSource` | Starts one turn and returns `turnId` and status. Current public modes are `start` and `start_if_idle`; other modes are rejected on this method. `selectedSkills` names up to eight effective skills for this turn. `workflow` may be `{"kind":"skill_group","id":"pstack","mode":"auto"}` for a turn-local group activation. `turnSource` is bounded metadata; the currently defined value `child_wakeup` marks an automatic parent continuation and does not change the input text. `operationAttemptKind` is child lifecycle metadata (`initial`, `retry`, `follow_up`); it does not change Core execution. |
| `turn/read` | `turnId` | Returns status, optional `stopReason`, optional `finalText`, step count, bounded messages, projected items, optional error, and bounded execution recovery metadata. An unsettled Turn with an execution checkpoint returns `in_progress`. |
| `turn/resume` | `threadId`, `turnId`, `checkpointSeq`, stable `requestId` | Explicitly resumes the same logical Turn from the matching persisted execution checkpoint. The request fails if the Turn or checkpoint sequence is stale. Repeating an accepted request ID is idempotent. |
| `turn/events` | `threadId`; optional `afterSequence`, `limit` (`1..128`) | Returns a bounded replay page of ordered `turn/event` notifications with `nextCursor`, `oldestSequence`, and `hasGap`. |
| `turn/steer` | `threadId`, `turnId`, `text`, optional bounded `requestId` | Sends cooperative steering input to the active turn. The supplied `turnId` must be active. Child control supplies a stable request ID so a replayed accepted steer is idempotent. |
| `turn/interrupt` | `threadId`, `turnId` | Requests cooperative cancellation and returns `{accepted: true}` when admitted; settlement remains pending until `turn_finished`. |

`turn/start` is asynchronous. Clients should render `turn/event` and Item
notifications while the turn is running, then use `turn/read` for the settled
result. Steering and interruption are requests to the runtime; they do not
force an immediate stop before the runtime reaches a cancellation boundary.

The Session checkpoint and execution checkpoint serve different purposes. The
Session checkpoint stores model context after a Turn settles. New Turns and
forked Child Sessions use that context. The execution journal stores the input
and model context for one logical Turn, its next model step, and durable tool
batch outcomes. Core writes a checkpoint before each model request and after a
whole tool batch. A failed checkpoint write stops execution before the next
model request or side effect.

App Server startup never resumes an execution checkpoint automatically. A
persisted active Turn becomes `waiting_for_continue` after restart. A tool call
that started without a recorded outcome becomes `needs_reconciliation`; clients
must verify that side effect before they continue. A completed tool batch with
recorded outcomes can continue without rerunning those calls. `turn/read`
returns bounded status, phase, heartbeat and progress timestamps, checkpoint
sequence, and recovery reason. `turn/resume` requires the current Turn ID and
checkpoint sequence, then continues that same Turn without creating a new Turn
or child operation attempt. While an execution checkpoint is waiting or needs
reconciliation, `turn/start` returns `not_submitted` and preserves that
checkpoint. The caller must resume or reconcile it before starting another Turn.
The App Server records an executor heartbeat every 10 seconds. The Responses
provider treats 120 seconds without provider data as a stalled request and does
not retry that silent stream. Recognized transient transport, incomplete-stream,
HTTP 408/429, and 5xx failures use exponential delays of 1, 2, 4, and 8 seconds,
bounded by five attempts and a 120-second retry window. Partial events from a
failed attempt are discarded.

When a Turn is waiting for tool approval, the control plane queues
`turn/interrupt` before it releases the approval wait. The App Server worker
prioritizes the queued stop command, so the Turn observes cancellation before it
can start another model or tool step. The approval resolves as denied. Clients
must still wait for `turn_finished`; an accepted interrupt is not a settled
result.

`turn/events` is a reconnect aid, not a second history store. The App Server
keeps a bounded in-memory window of Core events per process. `afterSequence` is
exclusive; when the requested cursor is older than the retained window,
`hasGap` is true and the client must reconcile with `thread/read` and
`thread/items/list` before accepting the replay as complete.

When `selectedSkills` is present, the worker resolves names against the
effective catalog before model execution. It emits `skills_loaded` with
`phase: "started"` and then `phase: "loaded"` after `turn_started` and before
`run_started` when every selected body loads. Records contain `name`,
`qualifiedName`, `source`, and optional `group`. A missing `phase` in a legacy
event is interpreted as `loaded`. Namespaced
`pstack:how` is canonical; `pstack-plugin:how` is a compatibility alias and
`how` is accepted when it is unambiguous. If validation or body loading fails,
the worker emits `skills_load_failed` with the selected names and bounded
`reason_code`, then finishes the turn as failed without calling the model.

When `workflow` activates a Skill Group, the worker emits
`skill_group_activated` and adds a bounded metadata-first instruction for that
turn. It does not read every body or invoke a routing model. The model can read
matching `SKILL.md` files with `read_file`; Host recognizes the first read of
each trusted Skill path and emits `skills_loaded` with `phase: "started"` before
the tool read and `phase: "loaded"` after a successful read. Explicit Skill
activation uses `activation: "explicit"`; ordinary metadata-first Turns use
`activation: "on_demand"`. The event carries the same Thread/Turn identity,
Core sequence, and bounded `itemId` as other `turn/event` notifications.

启用 Skill 的根目录由 Host 作为受信任的只读根加入工具 Workspace。全局 Skill 在
metadata 中使用受控的 `.mini-agent/skills/...` 或 `.agents/skills/...` 逻辑位置，
Host 会把它解析到已授权的实际根目录；模型可以在需要时用现有 `read_file` 查看
该 Skill 目录内的关联文档、脚本源码或其他文本
资源；App Server 不递归预加载这些文件，也不把资源路径加入 capability manifest。
当前 Turn 的 Skill 目录读取结果合计不超过 64 KiB。这个授权不改变写入、Shell
执行或审批边界。

The capability manifest returned by `initialize` contains
`builtinSkillGroups` and `availableSkills`. Each available-skill entry contains
only `name`, `qualifiedName`, compatibility `aliases`, `description`, `source`,
`group`, and `enabled`. The manifest does not expose skill paths or bodies.

#### Child operations and Session notebook

`delegate_task`, `task_list`, `task_read`, `task_report`, and `task_control` are
Host-owned capabilities. `delegate_task` returns a bounded queue request; the
surrounding Host/App Server control seam creates an exact child Session and
starts a separate child runtime. Child Sessions receive `task_report`, but not
delegation tools, so delegation remains one level deep. `task_list` and
`task_read` read the canonical child operation and Session projection. They do
not add a scheduler or a second history authority to Core. WebStudio
defaults to two active children per parent, with a Host setting bounded to
`1..=8`; overflow is durable `queued` state and starts automatically when a slot
frees. Queued operation prompts retain ordinary line breaks and tabs, remain
bounded to 32 KiB, and reject other control characters. `task_read` should be
used to check a queued task once and to follow active children; queued tasks do
not need repeated polling.
`delegate_task` accepts `child_key`, a bounded key unique within the creating
parent Session. Capabilities derives the canonical `child_thread_id` from the
parent Session ID and key and returns that ID with the queue request. The same
key in another parent Session maps to a different child Thread; display titles
may repeat and never define identity. `task_read`, `task_list`, and
`task_control` use the returned canonical ID. The bounded tool result carries
identity and scheduling metadata but omits the prompt. Gateway first persists
the `tool_started` arguments in a bounded receipt, then materializes a task only
after the matching successful `delegate_task` result arrives. This keeps
maximum-size prompts out of the model-visible tool result and binds recovery to
the parent Turn and tool call.
The setting controls only the active-child capacity. Every `delegate_task` call
must provide `execution_mode` as `parallel` or `sequential`. Sequential work
also requires `group_id` and a zero-based `sequence`. Missing sequence positions
wait; a failed or cancelled predecessor pauses later work in the group. The Host
validates and persists that scheduling intent without changing Core's loop.

Children send progress with `task_report`. App Server appends each bounded
report to the child Session with its operation ID, attempt, report ID, timestamp,
and Session cursor. Repeated report IDs within an attempt are idempotent. A report
is attributed to the attempt active when the tool ran, so a late report can still
be stored after that attempt settles or a retry starts. The parent reads reports
incrementally with `task_read`; a bounded receipt in the parent Session marks
each returned report as `main_received`, while unread reports remain `reported`.
This receipt is idempotent by child Thread, operation, attempt, and cursor. Full
child tool activity and transcript stay in the child Session. The JSON-RPC
`child/task` action supports report persistence,
queued-task updates/cancellation, and `queue_follow_up`. Follow-up requests validate
the parent lineage, stable operation ID, expected completed attempt, bounded
32 KiB prompt, and request ID before allocating the next attempt. Repeating a
request ID returns that allocation rather than creating another Turn. WebStudio
routes `task_control.assign` to a steer for a live child or to a same-Session
follow-up after completion. Failed, cancelled, and step-limited attempts retain
the existing retry action and prompt. Before a child steer is submitted, the App
Server persists its request ID as a reservation; acceptance or definitive
non-acceptance is then appended to the same Session log. Replays retain the
original route and do not inject a second instruction. If a restart leaves only
an unresolved reservation, `turn/steer` returns `pending`; the Gateway must not
resend that ID automatically. The Gateway reports the unresolved outcome in the
parent wake-up, and the parent rereads the authoritative child Turn and operation
state before deciding what to do. A reservation may have been written immediately
before or after delivery, so pending is an at-most-once outcome, not a delivery
guarantee. If the child settles between Gateway inspection and reservation, the
App Server returns `not_submitted`; the Gateway rereads the child and may queue
the completed follow-up with the same request ID.

`task_list` returns parent-owned child operation summaries in pages of at most
32 entries, ordered by `child_thread_id`; use `next_cursor` as
`after_child_thread_id` to continue. It does not return reports or transcripts.
`task_read` returns bounded status, attempt, result, error, and incremental
reports (`after_cursor`, at most 32 reports / 10 KiB per page).
Each operation summary keeps `status` separate from `turn_outcome` and
`latest_session_turn`. `turn_outcome` describes the Turn bound to the current
operation attempt. `latest_session_turn` describes the newest Turn in the child
Session, which may belong to a later follow-up. Use `operation.status` to decide
whether the child task attempt succeeded.
Both tools reconcile a nonterminal operation with its matching settled Turn.
This includes operations left in `pausing` or `cancelling`: the settled Turn is
authoritative, so an interrupted Turn after a pause request projects as `paused`,
an interrupted Turn after active cancellation projects as `cancelled`, and any
completed or failed Turn keeps its actual terminal status. The projection drops
the stale control request so Gateway recovery does not interrupt a settled Turn
again.
If a completed operation has no stored result, `task_read` recovers the final
assistant item from that Turn, bounded to 512 characters. `reports` contains
only explicit `task_report` updates; an empty report page does not mean the
final result is missing. Persisted operation results allow ordinary line
breaks and tabs and are bounded to 16 KiB.
Child operation Turns use the same continuous loop profile as main-thread
Continuous mode: `max_steps=0` and context compaction. They continue within the
same Turn until a final response, failure, or explicit control action. An active
Goal's explicit milestone step budget still takes precedence. This does not
create a hidden follow-up Turn. The Gateway's SDK wait window is 60 seconds and
is polled again after timeout; there is no five-minute total child Turn deadline.
A Turn stopped by an explicit Goal step budget records its stop reason, step
count, and bounded diagnostic. A steer accepted during a child-task Turn adds
input to that same Turn, so it does not mark the operation failed or release
its concurrency slot. Regular Turns keep their existing stop-at-checkpoint steer
behavior.
Every per-child `task_control` intent identifies `child_thread_id`,
`operation_id`, and the expected positive `attempt`. The Gateway compares that
identity against the persisted projection before it acts; stale attempts return
a visible stale outcome. `cancel_group` instead targets a sequential group in
the current parent Turn.

At most one follow-up can be pending for an operation. Duplicate request IDs
return the same result; a second pending request is rejected without replacing
the first. A pending follow-up starts on the same child Session after the
current attempt succeeds. If that attempt fails, the follow-up remains durably
blocked until a retry succeeds; cancelling the operation cancels the pending
follow-up. Retry increments `attempt` on the same operation and Session.

Pause and stop first persist their request in the child Session, then ask the
Gateway to interrupt the active Turn cooperatively. The operation remains
`pausing` or `cancelling` and occupies a concurrency slot until the Turn settles.
Only a settled pause projects `paused` and frees the slot. Resume returns the
same operation and attempt to `queued`; it does not count as a retry. The
Gateway uses one control executor for parent `task_control` intents and
WebStudio panel actions. Each request is bound to a stable request ID, operation,
and attempt. A tool control is dispatched only after App Server reports its
validated `tool_finished` result, not from the unvalidated start event.

Task control execution remains Gateway-mediated. The Gateway sends each control
action through the child runtime, then coalesces its bounded outcome into a
parent wake-up. An active parent is not interrupted; the Gateway starts one
continuation Turn after it settles. An idle parent starts one continuation Turn
when the update arrives. The outcome reports what the Gateway
control call did. App Server operation state remains authoritative, so the
parent rereads `task_read` or the child projection before deciding what to do
next. Pending wake-ups live in Gateway memory and are not replayed after a
Gateway restart. WebStudio retains at most 64 distinct pending child states per parent
and submits at most 16 child updates per continuation; overflow is summarized
with a count and up to eight sample IDs. A per-Session start lock serializes
automatic wake-up admission with user Turn starts.

#### Session-wide freeze and explicit resume

`session/control` is the durable lifecycle authority for a parent Session. A
main-thread Stop first writes `freezing`, then Gateway requests cooperative
interruption of the parent Turn and pauses active child Turns. Queued child
operations remain queued. The state becomes `frozen` only after the parent and
active child Turns settle. Queue draining, child retries, and child wake-up
continuations are gated while the Session is freezing or frozen, including
after Gateway or App Server restart. An incomplete freeze is reconciled from
the persisted request and Turn state; the Gateway does not infer settlement
from an unreadable runtime snapshot.

Only an explicit user Continue changes the Session to `resuming`. It resumes
children whose persisted control source is `parent_freeze`, drains the preserved
queue, and starts the parent with `turnSource: "session_resume"` to inspect the
existing work and continue from the settled point. A child paused or cancelled
individually by the user or main agent is not resumed by parent Continue. Child
operation control records retain `control_source` (`main_agent`, `user_panel`,
or `parent_freeze`) through Turn settlement. A report written while a parent is
stopping remains durable; its receipt changes from `reported` to
`main_received` only when `task_read` returns that report. Reports cannot wake a
frozen parent into a new Turn.

The Session store appends operation lifecycle records (`queued`, `running`,
`awaiting_approval`, `paused`, `completed`, `failed`, or `cancelled`) to the existing
bounded JSONL persistence. A child operation keeps the same `operationId` across
attempts and increments `operationAttempt`; `attemptKind` distinguishes `initial`,
`retry`, and `follow_up`. A retry or follow-up is a new child Turn, not a replay of
the old Core loop. The App Server `runtime/status` and
WebStudio child projection may expose the latest operation identity without
copying child history into the parent.

Gateway recovery scans persisted operation projections after Runtime attach or
restart. Queued operations without a Turn are drained again under the existing
Child lock; operations with a Turn are reattached for observation instead of
starting a duplicate Turn. A `delegate_task` intent is first recorded as a
bounded Gateway receipt keyed by its parent Turn and tool call, so a crash
between event observation and Child materialization can be retried idempotently.
The Gateway may publish a parent-scoped `child_operation_updated` projection with
only operation/Child IDs, status, execution mode, group/sequence, and a bounded
error code; it never copies the Child transcript into the parent stream.

The Session-owned `notebook.json` is a separate bounded persistence surface.
`notebook_read` and `notebook_write` are Host/Capabilities tools; the matching
session methods expose the same authority to WebStudio. Entries have an explicit
importance level, are deterministically summarized, and can be forgotten. After
resume, App Server injects only a bounded notebook summary, while full entries
remain available through explicit reads. Child Sessions do not copy parent entries;
they may read a validated parent snapshot but cannot write or forget it.

#### Thread settings, Plan, and Goal

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `thread/settings/update` | `threadId`; optional `collaborationMode: {mode}`, `builtinTools: [name]`, `continuationMode`, `modelSelection: {providerId, modelId}`, and `reasoningSelection` | Updates Thread settings. `reasoningSelection` is `{kind: "api_default"}` or `{kind: "level", value: "<model-supported-level>"}`. An explicit `null` for `modelSelection` or `reasoningSelection` clears that Thread override and returns to defaults. The older `reasoningEffort` string is accepted for compatibility. Model changes apply to the next Turn. Emits `thread/settings/updated`; Goal Runtime owns its own loop while a Goal is active, so continuation updates are rejected until that Goal is paused or settled. |
| `thread/model-settings/get` | `threadId` | Returns the persisted `modelSelection` and typed `reasoningSelection` for the Thread. |
| `model/catalog/manage` | `operation`; operation-specific provider, model, default, or Project fields | Reads or updates the Host-owned machine model catalog. `set_defaults` accepts `defaultModel`, `defaultReasoningSelection` (`{kind: "api_default"}` or a supported `{kind: "level", value}`), and a separate `verifierDefaultModel`. `test_connection` sends one bounded request without tools and returns a bounded status/message. Provider API keys are accepted only on provider updates. Responses include `apiKeyConfigured` and never include key values. |
| `thread/goal/set` | `threadId`; optional `objective`, `status`, `tokenBudget` | Sets or replaces a Goal subject to lifecycle checks; returns the public Goal projection and emits `thread/goal/updated`. A running Goal must be cleared before replacement. |
| `thread/goal/get` | `threadId` | Returns `{goal}` where `goal` may be `null`. |
| `thread/goal/clear` | `threadId` | Clears the Goal and returns `{cleared: true|false}`; emits `thread/goal/cleared` when applicable. |

Goal status values are `active`, `paused`, `blocked`, `usageLimited`,
`budgetLimited`, and `complete`. Goal continuation, verification, pause,
resume, and checkpoint association belong to GoalRuntime; clients do not
submit verifier verdicts or advance milestones directly.

#### Runtime observation

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `runtime/status` | `threadId` | Returns a non-blocking bounded snapshot: `phase`, `threadId`, optional `turnId`/`operationId`/`checkpointSeq`, `stateRevision`, `timestampMs`, and optional `error`. |

`phase` distinguishes `starting_turn`, `model`, `tool`, `waiting_approval`,
`stopping`, `compaction`, `persisting`, `goal_verification`,
`goal_continuation_queued`, `completed`, and `failed` (as well as `idle`,
`resuming`). `stopping` is monotonic for that Turn: late approval or tool
notifications cannot regress it to a runnable phase. The snapshot is
control-plane telemetry; it does not replace `turn/read` or the canonical
Goal/Thread projections.

#### Control ordering and attachment

The App Server worker admits commands in one sequence. While a Turn is active,
`turn/start` can only submit typed steer/follow-up input for that same Thread;
`turn/interrupt`, `thread/fork`, `session/fork`, Thread mutations, and runtime
mutations are checked against the active identity. `turn/interrupt` sets the
cooperative stop request and returns before tool/model cleanup finishes. A
`thread/fork` or compact `session/fork` is rejected as busy while the source
Turn is active; an exact `session/fork` is admitted by reading the last
complete persisted checkpoint. A fork that arrives after an interrupt is
accepted remains rejected while the source Turn is stopping, and must be
retried after `turn_finished`. Neither fork path copies an in-flight Turn or
approval wait. After stopping is accepted, new
steer/follow-up input and runtime mutations are rejected until the same Turn
reaches `turn_finished`; read-only observation remains available.

`thread/fork` remains an in-process logical Thread fork. `session/fork` creates
an independent Session from the latest settled checkpoint. The exact variant
reads the last complete persisted checkpoint without borrowing mutable Core
state, so a Host can create a child Session while the source Turn is running;
the in-flight prompt, tool calls, and approval wait are intentionally not
copied. The compact variant still requires an idle source and may prepare
context through the source runtime. Gateway `attach` reuses an existing
local client when possible, reports the local active Turn in its response, and
returns a locked external Session as read-only. It never steals a Session lock
or starts a competing writer. Consumers should keep the active identity until
`turn_finished` and use the canonical Session status after reconnect.

#### Runtime management

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `world/state` | No parameters | Returns the current workspace, structured status, status lines, and bounded model context. |
| `world/refresh` | No parameters | Refreshes the world and returns `{changed, state}`. |
| `world/set_execution` | `access`, `policy` | Sets execution scope and returns `{changed, state}`. `access` is `project` or `full_machine`; `policy` is `interactive`, `automatic`, or `trusted`. `trusted` bypasses approval for ordinary validated workspace and Shell actions and public `web_fetch` requests after URL validation. Destructive, system-level, MCP, and workspace-external actions such as `read_image` still require approval. |
| `mcp/status` | No parameters | Returns enabled/inactive servers, tool count, and whether retry is available. |
| `mcp/retry` | No parameters | Retries MCP setup and returns enabled/inactive servers, diagnostics, and tool count. |

`world/set_execution` changes runtime configuration, not the security order.
`full_machine` expands the candidate filesystem range but does not mean
allow-all: Deny, Plan-mode source-file mutation locks, tool availability, and
high-risk confirmation still apply. Shell remains governed by the selected
policy. `policy` selects the global execution posture; it does not choose
the lifetime of an individual grant.

#### Approval response

| Method | Parameters | Result / effect |
| --- | --- | --- |
| `approval/respond` | `requestId`, `decision` (`approve` or `deny`), optional `grantScope` (`once`, `session`, or `project`), optional `reason` | Resolves one pending approval. The server emits `approval/resolved`; it does not return a second approval authority to the client. |

The response may select only one of the request's `allowedGrantScopes`; a
denial cannot carry a grant scope. Host/Capabilities match grants against the
complete structured action key: action class, normalized action, target paths,
access scope, workspace, and revision. Web clients own only pending/UI state;
they do not create or cache grants. A changed workspace revision, project
switch, policy change, or revocation invalidates prior reuse.

### Server notifications

Notifications have no JSON-RPC `id` and never receive a response. They are
emitted on one ordered runtime stream.

| Notification | Payload highlights | Use |
| --- | --- | --- |
| `turn/event` | `threadId`, optional `turnId`, optional `turnSource`, Core `sequence`, bounded `items`, `event` | Ordered Core execution events, including turn settlement. `turnSource` is repeated on replay notifications for the source Turn. |
| `turn/event` with `skills_loaded` | `phase`, `activation`, `skills: [{name, qualifiedName?, source, group?}]` | Reports a Skill body read starting or completing for the current turn. Missing `phase` means `loaded`. |
| `turn/event` with `skills_load_failed` | `skills: [name]`, `reason_code` | Reports a bounded activation failure before model execution. |
| `turn/event` with `skill_group_activated` | `group`, `source` | Reports a turn-local Skill Group workflow activation. |
| `item/started` | `threadId`, `turnId`, `item`, `startedAtMs` | One ThreadItem becomes visible. |
| `item/completed` | `threadId`, `turnId`, `item`, `completedAtMs` | Authoritative final projection for that item. |
| `approval/request` | Request identity, project/workspace/revision, action class, summary, structured action key, access, policy, allowed grant scopes | Requests a user decision for a sensitive action. |
| `approval/resolved` | Request identity, `outcome`, selected `grantScope`, and optional `reason` | Reports the settled approval result without creating a second authority. |
| `thread/settings/updated` | `threadId`, effective mode, Builtin tools, continuation mode, optional `modelSelection`, optional `reasoningSelection`, compatibility `reasoningEffort`, `stateRevision` | Projects a settings change. |
| `thread/goal/updated` | `threadId`, optional `turnId`, Goal projection, `stateRevision` | Projects Goal creation, update, or runtime progress. |
| `thread/goal/cleared` | `threadId`, `stateRevision` | Projects Goal removal. |
| `runtime/status/updated` | Runtime status snapshot | Reports phase transitions without waiting for a turn to settle, including the monotonic `stopping` phase. |
| `checkpoint/committed` | `threadId`, `turnId`, `checkpointSeq`, `stateRevision`, `operationId` | Confirms a settled turn checkpoint was persisted. |
| `goal/verification_started\|completed\|failed` | Goal/turn/checkpoint identity, milestone fields, optional `error` | Exposes the verifier boundary and result. |
| `goal/continuation_queued\|started` | Goal/turn identity, checkpoint and milestone fields | Exposes scheduling and actual start of the next Goal turn. |
| `plan/updated` | `planActive`, checkpoint and `stateRevision` | Reports Plan projection changes. |
| `plan/cleanup_started\|completed\|failed` | Operation/checkpoint identity, optional `error` | Reports cleanup of Plan scratch state, including failures. |

`sequence` is the Core Thread event sequence. `actionSequence` in an action
response is the App Server admission order; they are different counters and
must not be merged by clients. The stable ToolCall item identity is the model
`callId`, which lets a client merge model, start, completion, and replay
projections.

For an exact `session/fork`, the checkpoint supplies the child model's initial
context. `thread/items/list` still returns items owned by the child Session.
When a fork child has no local item records, the App Server returns an empty
page instead of projecting the inherited parent checkpoint as child activity.
Ordinary non-fork Threads retain the checkpoint fallback when Session item
storage is unavailable.

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

`session/fork` child identity conflicts use `-32001`, not the generic `-32000`
runtime error. The error `data` keeps the action metadata and adds a tagged
`kind` of `parentLineage` or `contextPolicy`, plus the bounded child and policy
fields relevant to that conflict.

The standard error codes currently used are:

| Code | Meaning |
| ---: | --- |
| `-32700` | Parse error. |
| `-32600` | Invalid JSON-RPC request or protocol version. |
| `-32601` | Method not found, including removed legacy methods. |
| `-32602` | Invalid or incomplete parameters. |
| `-32000` | Runtime, capability, approval, or management failure. |
| `-32001` | `session/fork` cannot reuse the child identity because its parent lineage or context policy conflicts. |

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
