# Codex-Aligned Agent Control Plane for Approval, Plan, Goal, and Web Studio

Status: proposed — pending review and bounded implementation batches
Date: 2026-09-03
Scope: mini-codex Protocol/Host/App Server and mini-agent-web SDK/FastAPI/Web Studio

## Decision requested

Converge the Mini Agent execution path and Web Studio control surface around one
explicit control plane:

```text
Web Studio
    → FastAPI gateway
    → Python SDK
    → App Server JSON-RPC
    → Host/Capabilities admission and approval
    → mini-agent-core turn loop
```

The control plane must keep user controls and execution ownership explicit,
without introducing a standalone Profile layer:

```text
Thread Mode (session): chat | plan | goal
Access and Approval (runtime): access scope + approval mode
Execution ownership: Core Turn | Plan lifecycle | Goal lifecycle
```

The runtime also binds every Thread/Session to one immutable workspace
manifest:

```text
Workspace Binding (Session): primaryRoot + associatedRoots + per-root access policy
```

The intended user-facing flow is deliberately simple:

1. Create a Project in Web Studio and start a task.
2. When the task needs editing, choose the `Full access` preset once. It means
   machine-wide access for the current Runtime/Session, with an explicit
   high-risk confirmation. It still preserves non-overridable Host Deny,
   Plan locks, and any security-prescribed shell confirmation. A separate
   `Project access` preset is available when least privilege is desired.
3. If the task spans more directories, add them in Project editing as either
   editable workspace roots or reference-only roots. The agent receives the
   resulting immutable manifest.
4. If a directory is mentioned only in a message, treat it as a one-time
   reference request by default. A request to synchronize or update it must
   create explicit path-scoped write intent. If machine-wide `Full access` is
   already active, Host may admit it under that scope and its remaining
   guardrails; otherwise ask for explicit approval or offer to add it to the
   Project as an editable root. A model-authored path must never silently
   expand the Project.
5. Enter Plan with `/plan`; Plan is read-mostly but may run bounded exploration,
   create scratch scripts/outputs, clean them up, and retain an explicit
   `plan.md`. Enter Goal with `/goal`. Goal remains visible above the
   conversation for the selected Thread and exposes Start/Resume, Pause,
   Update, and Delete actions.
6. For Auto Copilot, choose `Goal + Full access (machine-wide) +
   current-project approval`. This is an explicit Project-owned machine-wide
   autonomous runtime, not a Profile or a global allow-all setting.

Plan and Goal remain the existing canonical Thread boundaries. This proposal
does not create a second Workflow service, a second turn loop, or a generic
policy/plugin framework.

### Direct cutover; no migration compatibility layer

This proposal defines one new control contract and one canonical storage
layout. It does not read, import, translate, write, or delete legacy Web
`~/.mini-agent/state.json`, legacy Web checkpoint files, `profile`,
`profile=auto`, `interactive`, `ask`, `auto`, legacy `remember`, or
boolean/string approval shapes. Those artifacts and inputs are outside the new
runtime's scope. The only narrow migration exception is an inbound old
`turbomode`/`Turbomode` token: it may map once to `Full access` /
`SecurityPreset::FullMachine`, then must be discarded. It is never persisted,
exposed as a Runtime/Profile identity, or accepted by the new public protocol.
The new implementation starts from its canonical Session Store and new
`~/.mini-agent/web/state.json`; no general migration parser, importer,
fallback, or compatibility layer is admitted.

## Current evidence and problem statement

The current implementation has the required pieces, but their authority and
semantics are split across the three repositories:

1. `ApprovalController` caches every approved action in `ApprovalStore`. As a
   result, a UI choice intended as “allow once” can behave like “always allow”.
2. `ApprovalRespondParams` carries `remember`, but the App Server response path
   currently consumes only the boolean approval result. The requested access
   scope and lifetime are therefore lost before Host execution resumes.
3. `mini-agent-web` also keeps `_remembered_approvals` and broadcasts pending
   approvals to every WebSocket client. This creates a second approval authority
   and permits cross-Thread presentation or response races.
4. `profile` is applied at SDK/App Server initialization. Web settings updates
   do not rebuild the running Host profile, so a visible profile change may not
   change runtime behavior.
5. The Web UI treats Plan and Goal as part of a profile-like selector, while the
   App Server correctly models Plan as Thread settings and Goal as a separate
   Thread Goal lifecycle.
6. Several Web calls omit the active Thread ID and fall back to `default`.
   Plan, Goal, Builtin selection, and approval presentation can therefore point
   at a different Thread than the one visible in Studio.
7. The current Web slash catalog covers only a subset of the desired control
   surface. `/goal`, `/compact`, `/mcp`, `/review`, and bounded Skill/Plugin
   discovery are not yet one consistent command contract.
8. Rust `SessionStore` already persists the canonical append-only log, settled
   checkpoint, summary, signals, prompt context, attachments, and lock under
   `~/.mini-agent/sessions/`, while Web Studio separately persists
   `state.json`, per-Thread checkpoints, and workspace logs. This creates two
   possible history/resume authorities.
9. Web Studio accepts a Project primary path and multiple `source_folders`,
   but the current SDK launch uses process cwd and does not pass the complete
   Project root manifest into Host tool construction. The UI therefore shows a
   wider Project than the agent's effective workspace.
10. Web metadata does not currently provide a canonical Project Session
    catalog with runtime status, pause reason, lock/read-only state, active
    turn, and resumability. The UI's global client/active state is insufficient
    for switching among historical, running, and paused Sessions.
11. Core's default `max_steps` is 8. Web Studio currently exposes a technical
    `Turn Settled`/`step_limit` result as if the Turn were interrupted, without
    explaining that this is an implementation loop guard rather than a useful
    user task state.

Relevant existing owners are:

- `crates/mini-agent-capabilities/src/workspace/approval.rs`: Host-side
  approval enforcement and cached approvals;
- `crates/mini-agent-app-server/src/lib.rs`: approval broker and lifecycle;
- `crates/mini-agent-app-server-protocol/src/lib.rs`: approval, Thread settings,
  and Goal wire types;
- `server/session_manager.py`: SDK lifecycle, Web transport, and current
  duplicate approval state;
- `sdk/python/src/mini_agent/client.py`: Stdio JSON-RPC and notification adapter;
- `frontend/src/App.jsx` and `frontend/src/utils/slashCommands.js`: Studio
  control state and local command dispatch.

## Target semantics

### No standalone Profile layer

The product and protocol should not model `Profile` as a separate control-plane
layer. Runtime startup receives the actual bounded inputs it needs: provider,
prompt/rule sources, extensions, tools, sandbox/security, access scope, and the
immutable WorkspaceSpec. If an implementation needs to pass these values as an
internal struct, it remains a private runtime configuration object rather than
a user-visible Profile, a selectable workflow, or a persisted Session axis.

Studio should show the current mode and Goal state, but mode activation should
follow the user's explicit slash commands rather than requiring a second mode
selector:

| Studio label | Canonical operation | Runtime meaning |
| --- | --- | --- |
| Interactive / Chat | Normal composer or `/plan off` | Ordinary user-driven Turns |
| Plan | `/plan` or `/plan on\|off` → `thread/settings/update` | Read-mostly exploration runtime; scratch scripts/outputs and `plan.md` allowed, formal project mutation deferred |
| Goal | `/goal ...` → `thread/goal/set|get|clear` | Thread-owned autonomous lifecycle and verifier |

The current code does not justify three distinct user-facing presets: the Host
`interactive`, `ask`, and `auto` built-ins share the same named baseline unless
overridden by workspace configuration. In the stdio App Server path, selecting
`auto` also does not remove the default `HarnessConfig` eight-step limit. The
legacy embedded/local `automatic` path changes loop budgeting and context
compaction; that is an execution implementation detail, not a user persona.

Therefore Web Studio and the REPL should remove the profile selector and the
`profile=auto` runtime option. The new public contract rejects profile-shaped
inputs instead of translating them. No runtime or Session retains a Profile
identity. The only machine-wide access name is user-facing `Full access`,
internally backed by `SecurityPreset::FullMachine`. The old
`turbomode`/`Turbomode` token is permitted only at a migration input boundary
as a one-way alias to that access preset; it must not survive as a Runtime or
Profile identity. Expose the actual user decisions independently:

- access: `project` or `machine`;
- approval mode: `per_action`, `current_session`, or `current_project`;
- mode: Chat/Plan through `/plan`;
- the selected Thread's Plan runtime state and Goal lifecycle through `/goal`
  and its persistent header.

Changing startup inputs requires Runtime recreation or an explicit next-Runtime
operation. It must not silently update only a Python preference field.

### Plan is read-mostly, not read-only

Plan owns a bounded exploration runtime for the selected Thread. It may read
the declared Project roots, run analysis commands or scripts, write temporary
scripts and generated outputs into a Session-owned `planScratchRoot`, and write
the explicit `plan.md` artifact. The scratch area is not a Project root and is
not a durable implementation workspace.

Formal Project mutation means changing source files, Project configuration, or
business/generated artifacts that the user expects to keep. Plan defers those
mutations behind its existing Plan gate even when the selected access scope is
machine-wide. Approval and Full access authorize admission scope; they do not
silently turn Plan into implementation mode.

The Plan runtime must maintain a bounded cleanup manifest. On normal Plan
settlement, cancellation, or explicit exit, it removes scratch scripts and
outputs while retaining `plan.md` and a bounded summary of the exploration.
Cleanup failure is surfaced as `cleanup_pending` and never silently presented
as a clean Plan completion. If the user explicitly asks to keep an exploratory
output, that becomes a separately reviewed Project mutation.

For Plan exploration, the shell/tool working directory should default to the
Session-owned scratch root, with declared Project roots mounted or exposed as
read inputs. Chat and Goal may keep the Project primary root as their normal
working directory according to their own Runtime rules.

### Approval mode and access scope are different

`per_action` describes when an action must ask. It does not mean that every
action is automatically allowed. Tool exposure, Host security policy, Plan
lock, and human approval remain separate gates.

The public decision vocabulary is:

```text
approval: per_action | current_session | current_project
access:   project | machine
```

Rules:

- `per_action` means that each approval-gated action can ask; it does not mean
  that every action is automatically allowed;
- `current_session` remembers a bounded approval only for the current Thread/
  Session;
- `current_project` remembers a bounded approval for all Sessions belonging to
  the current Web Studio Project;
- `project` limits file/process admission to the immutable Project
  WorkspaceSpec;
- `machine` means machine-wide access scope for the current Runtime/Session;
- explicit security Deny always wins over a UI approval;
- Plan Mode's formal Project-mutation gate cannot be bypassed by approval;
  bounded writes under the Session-owned Plan scratch root and the explicit
  `plan.md` artifact remain allowed;
- current-project sharing never expands a Session's immutable WorkspaceSpec or
  turns a reference root into an editable root;
- a current-project approval is keyed to the Project, WorkspaceSpec revision,
  access scope, action class, and bounded path scope. It is not a global
  approval cache.

The approval key must include the bounded owner and action identity, for
example:

```text
(project/session owner, WorkspaceSpec revision, access scope, tool name,
 normalized action class, bounded path scope)
```

Raw unbounded commands, secrets, or arbitrary prompt text must not become an
approval rule.

### Product presets: Project access and machine-wide Full access

The common user path should not require selecting a new approval mode or scope for
every tool call. The Studio-facing choices are deliberately few. The normal
default remains `per_action`. The user can separately choose Project-shared
approval when they want all Sessions in the current Project to reuse a bounded
decision:

```text
Project access:
  access scope: current Project WorkspaceSpec
  coverage: primaryRoot + associatedRoots marked editable/reference

Full access:
  access scope: machine-wide for the current Runtime/Session
  confirmation: explicit high-risk confirmation before activation

Current-project approval:
  approval owner: the current Web Studio Project
  coverage: all Sessions in that Project, subject to the bounded approval key
```

`Full access` is the user-facing name for machine-wide access; it must not be
renamed or documented as Project-scoped access. The current implementation
baseline is `SecurityPreset::FullMachine`: file actions can address paths
outside the Project workspace, while hard safety Deny remains in force and
shell actions may still ask for confirmation. If a future product option also
removes those shell prompts, it must be a separately named, explicitly
confirmed high-risk preset rather than being hidden behind `per_action` or
`profile=auto`.

`Project access` remains bounded by the current Project's immutable
WorkspaceSpec, explicit security Deny, Plan locks, and configured tool scope.
The approval choice is independent of the access preset. The UI detail text
must say either “current Project workspace” or “entire machine”, identify the
approval owner (current Session or current Project), and identify any remaining
Deny/confirmation guardrails.

### Auto Copilot is an explicit composition, not a Profile

The user-facing Auto Copilot scenario is the following composition:

```text
mode:     Goal
access:   Full access (machine-wide)
approval: current_project
```

`Goal` supplies cross-Turn execution, verification, and continuation. `Full
access` supplies the machine-wide admission scope. `current_project` shares
bounded approval decisions across Sessions owned by the same Web Studio
Project. Together they create a Project-owned machine-wide autonomous runtime;
they do not create a Profile or a global allow-all switch.

Enabling this combination requires an explicit high-risk confirmation and must
show the effective meaning in the UI. Hard security Deny, unavailable tools,
Plan locks, pause/cancel, and any non-overridable confirmation remain in force.
If the user wants project-only autonomy, the safer composition is `Goal` plus
`Project access` plus `current_project` approval.

Project editing is the durable way to expand the workspace. It should offer
two explicit directory intents: `Add as reference` and `Add as editable
workspace`. A path mentioned in a normal message is temporary context, not a
workspace mutation. `reference <path>` may grant bounded one-time reading;
`sync/update <path>` must create explicit path-scoped write intent. If
machine-wide `Full access` is already active, Host may admit that intent under
the machine scope and its remaining guardrails; otherwise it must request
explicit approval or offer to add the path as an editable Project root. The
model cannot persistently expand the Project by emitting a path in its own
message.

### Thread-owned runtimes and internal safety guard

`HarnessConfig::default()` currently sets `max_steps = 8`. This is an
implementation loop guard, not a useful user task setting. Remove
`max_steps`/`step_limit` from the normal Web Studio, SDK, and REPL control
contract; do not offer “increase the step budget” as a routine recovery path.

Core still needs a non-user-configurable safety guard against a runaway
provider/tool loop. That guard belongs to Core's limits and must be exposed
through the new typed `runtime_guard` diagnostic, not a legacy compatibility
classification. It must not be rendered as “8 steps”, “Turn interrupted”, or
“task failed” without explaining the safety-guard cause.

Plan and Goal own their own runtime lifecycle:

- Chat runs one user-requested Core Turn and ends on provider completion, an
  explicit cancellation, or an exceptional runtime guard.
- Plan owns the planning Turn and Plan lock/state for the selected Thread; it
  does not depend on a global user-tunable step budget.
- Goal owns continuation, verification, pause/resume, and completion for the
  selected Thread. It may use an internal cycle guard, but no `max_steps`
  setting is exposed as Goal progress semantics.
- App Server routes and serializes these Thread-owned runtimes; it does not
  add a global Profile or step-budget layer.

“Settled” remains an internal persistence/lifecycle term and does not mean
“completed”. App Server and SDK should preserve the final semantic status and
the raw diagnostic fields for debugging. Web Studio should show Completed,
Cancelled, Waiting for approval, or “Runtime protection triggered; inspect or
retry” only when those outcomes actually occur. Raw implementation counters are
not part of the public status contract; internal diagnostics may record them
for debugging.

The `turn_finished` notification or its SDK projection must carry the final
semantic status (and raw diagnostics when available), so Web Studio never
infers “interrupted” from a generic stream close. Continuation is an explicit
next Turn or Goal action, not an unbounded automatic loop.

### Goal is the objective-driven autonomous runtime

Goal should be understood as a long-lived, objective-driven Agent/Copilot
runtime, not as a `profile=auto` switch and not as a larger single Turn. Its
unit of progress is a verified cycle across multiple Turns in the same
Thread:

```text
/goal <objective>
    → run one Goal Turn
    → persist the settled checkpoint and evidence
    → run the independent Goal verifier
    ├─ approved / objective complete → complete Goal
    ├─ rejected or insufficient evidence → schedule the next Turn
    │                                      in the same Thread
    ├─ user pause → paused
    └─ blocked / usage or safety limit → stop with an actionable status
```

The distinction between the layers is intentional:

- Core owns one bounded Turn and its tool/event/history semantics;
- App Server owns Goal scheduling, serialization, verifier dispatch, stale
  checkpoint rejection, and Thread-scoped notifications;
- Host owns Goal state and bounded evidence/plan persistence;
- SDK and Web Studio project the same Goal state and expose Start/Resume,
  Pause, Update, and Clear.

Each continuation creates a new `turnId` but keeps the same `threadId`,
`sessionId`, workspace binding, approval scope, and Goal identity. A rejected
verification is not a user-visible Turn failure: it is evidence for the next
attempt, and the Goal prompt must direct the agent to address the verifier's
findings. Completion requires verifier evidence, not merely a model claim.

Goal may use milestones as internal progress/evidence checkpoints, but
milestones are not a second user workflow. `max_steps`, per-Turn step budgets,
and loop counts must not be presented as the Goal's task model. Keep only a
non-user-configurable runaway/safety guard and, where needed, a bounded
resource/usage stop. These stops must produce `blocked`, `usage_limited`, or a
clear runtime-protection diagnostic and must be resumable or inspectable; they
must not be rendered as an unexplained “Turn interrupted”.

### Goal runtime limits for long-lived autonomy

The default Goal limits should favor meaningful autonomous progress instead of
short single-Turn runs. The proposed initial baseline is:

| Variable | Current default | Proposed Goal default | Boundary |
| --- | ---: | ---: | --- |
| `MINI_AGENT_GOAL_MAX_LOOPS` | 20 | 100 | maximum Goal continuation cycles |
| `MINI_AGENT_GOAL_STEP_BUDGET` | 50 | 200 | Core model-step cap per Goal milestone Turn |
| `MINI_AGENT_GOAL_TIMEOUT_SECS` | 600 | 1800 | cooperative wall-clock limit per milestone Turn |

These values are Host/App Server runtime safeguards, not Web Studio task
settings and not progress semantics shown to the user. They are intentionally
larger for Goal/Auto Copilot and may be lowered by operators for deterministic
fixtures. `MINI_AGENT_MAX_STEPS` is not part of the Web Goal control surface;
if a standalone CLI path retains it, it remains an implementation-only guard.
The optional Goal `tokenBudget` remains an explicit caller-selected resource
limit; it has no small implicit default, and an explicitly reached budget
settles as `budget_limited`.

When a Goal limit is reached, the active Turn must settle its checkpoint and
bounded evidence before the Goal Runtime reports `usage_limited` or a clear
runtime-protection state. Resume continues from that settled state; it does not
replay the user message or silently create a second Turn. The UI must never
surface the raw limit as “Turn Settled (Status: step_limit; Steps: 8)”.

### Limits by layer: enlarge autonomy, not every boundary

The current implementation has several limits with different purposes; they
must not all be treated as the Goal's step budget:

| Category | Current evidence | Proposal treatment |
| --- | --- | --- |
| Goal continuation | `20 / 50 / 600s` | Raise defaults to `100 / 200 / 1800s`, owned by Host/App Server; Web does not configure progress semantics |
| SDK waiting | Python `wait_for_turn` defaults to `60s` | Goal must not be truncated by this independent client timeout; use App Server settlement as authority, while an explicit caller deadline may produce visible cancellation/timeout semantics |
| Web human-approval wait | `session_manager` currently uses `600s` | Add a Host/App Server-owned approval-wait guard, proposed default `1800s` for Auto Copilot; expiry produces `expired` and never leaves a dangling approval |
| Core Turn safety guard | default `max_steps=8`, plus byte and call-count limits for input/context/model response/Tool output | Remove `max_steps` from the user control surface; retain other limits as non-user or Host-selected safety guards and do not auto-remove them for Goal |
| Approval cache | `ApprovalStore` currently holds at most `1024` entries and clears all when full | Bound entries by Session/Project owner and approval key; overflow affects only that scope and requires approval again, never clearing another Project |
| Pending/approval/event queues | Core accepts at most `16` pending inputs; App Server has `32` command and `256` event buffers; Web pending approvals have no explicit cap | Add bounded pending-request count and lifetime; at capacity return `unavailable`/a protection diagnostic rather than silently dropping requests. Keep cursor/replay semantics for events instead of solving disconnects with an unbounded buffer |
| History/read projection | App Server worker item lists are capped at `128`; verifier history is bounded to `24` messages | Keep cursor-based, bounded reads and explicit truncation metadata; do not replay an incomplete Turn as settled history or expand the model context to satisfy a UI page |
| Capability payloads | Source reads are capped at `8MiB`; read defaults/max are `200/2000` lines with `15KiB` pages; writes `1MiB`; patches `512KiB`, `16` files, `32k` lines; commands `16KiB` and `8MiB` capture; MCP has `120s`, `32` tools, `16KiB` schemas, and `64KiB` results | Keep these operation and payload limits independent from Goal continuation. Increase them only as a separately measured capability experiment with explicit truncation/failure outcomes |
| Host context/path metadata | World context is capped at `8KiB` with `1KiB` per path; Project instructions are capped at `16KiB` | Keep Project roots and runtime instructions bounded before they enter model-visible context; put larger material in canonical artifacts |
| Per-tool/MCP side effect | Shell/MCP calls are currently about `120s`, with command and result byte caps | Keep independent from Goal continuation; do not enlarge unconditionally for Auto Copilot without a separate Host safety experiment, and never let approval mode override it |
| Goal text/evidence | objective `8KiB`, plan/verifier `32KiB`, verifier history `24` messages | Keep bounded; put longer material in canonical artifacts instead of expanding model-visible context without a bound |

The common rule is that Core, Host, or App Server enforces each limit at its
actual ownership boundary, while SDK/FastAPI/Web only project the state. A
limit must settle recoverable state before reporting `usage_limited`, `expired`,
`unavailable`, or a runtime-protection diagnostic. Increasing Goal loops, step
budget, or timeout must not implicitly increase machine access, approval scope,
concurrent Session count, message size, or tool-result size.

Timeout and queue settlement are explicit: a Goal command returns an accepted/
running projection and App Server settlement remains authoritative; an SDK or
Web wait deadline may stop waiting, but it must not classify the Runtime as
paused, settled, or failed unless an explicit cancel/pause or an authoritative
runtime outcome exists. Reconnection resumes from the server cursor and
settled checkpoint. An approval wait that expires resolves as `expired`, and a
capacity rejection resolves as `unavailable`; neither leaves a pending request
that can later resume a Tool call.

Plan and Goal are deliberately different: Plan is a user-invoked,
read-mostly exploration runtime that produces a plan artifact; Goal is the
user-invoked autonomous runtime that can carry out approved Project changes,
verify them, and continue across Turns until completion, pause, or a terminal
guard condition. Neither requires a Profile selector.

## Ownership model

| Layer | Owns | Must not own |
| --- | --- | --- |
| Core | Turn loop, internal safety limits, stop classification, events, history writeback | UI, approval dialogs, persistence, runtime startup composition |
| Protocol | Typed Thread settings, Goal, approval request/response/resolution | Transport-specific UI state |
| Capabilities/Host | Tool admission, security Deny/Ask/Allow, Plan lock, scope-aware approval enforcement | WebSocket, React, JSON-RPC parsing |
| App Server | Thread state, Runtime Actor ordering, Approval Broker, public notifications | A second execution loop or arbitrary prompt replacement |
| Python SDK | JSON-RPC transport, typed parsing, approval callback adaptation | Policy authority or durable approval decisions |
| FastAPI | HTTP/WebSocket mapping and connection routing | Independent approval cache or runtime semantics |
| Web Studio | Render state and submit explicit user decisions | Guessing approval state or applying hidden policy |

The authoritative correlation chain is:

```text
projectId → workspaceId → sessionId → threadId → turnId → callId/itemId → requestId → resolved decision
```

All approval notifications and responses must be validated against this chain.

### First vertical slice: the Auto Copilot approval loop

The first implementation slice should validate one complete scenario: two
Sessions belong to the same Web Studio Project; the user selects
`Goal + Full access + current_project` in one Session; after one approval, the
other Session may reuse it for the same bounded action, but cannot cross an
access or security boundary. Control and tool approval must travel through one
chain:

```text
Studio selection
    → typed FastAPI request
    → typed JSON-RPC from the Python SDK
    → App Server validates Project/Workspace/Session/Thread identity and routes
    → Host/Capabilities applies access + approval + Plan/security admission
    → Core executes the admitted Tool Turn and emits bounded events
```

The approval round trip keeps the same correlation fields:

```text
Host admission request
    → App Server approval broker
    → SDK notification
    → FastAPI route
    → Studio decision
    → typed SDK/App Server response
    → Host resumes the same call
```

The minimum verifiable identity and decision fields are:

```text
projectId, workspaceId, workspaceRevision, sessionId, threadId,
turnId, callId, requestId, access, approval, actionClass, pathScope
```

These fields have separate responsibilities: `Full access` supplies the
current Runtime/Session's machine-wide access scope; `current_project` supplies
only a bounded approval-reuse lifetime; and `Goal` supplies cross-Turn
execute–verify–continue behavior. None replaces another, and Web cannot fill in
missing fields from local state. Core does not need Project or UI approval
vocabulary; it receives only Host-admitted tool results and its bounded events.
App Server cannot execute tools directly or turn a boolean approval into a
process-global allow cache.

The initial approval lifetimes are:

| Mode | Owner | Session switch/new Session | Invalidated by |
| --- | --- | --- | --- |
| `per_action` | current request | not shared | request settlement, denial, or cancellation |
| `current_session` | current Session | not shared with another Session | Session close, control-plane restart, or explicit revoke |
| `current_project` | current Project control plane | shared within the same Project | explicit revoke, WorkspaceSpec revision change, Project close, or App Server restart |

By default, `current_project` approval does not survive an App Server restart.
If cross-restart retention is needed later, it requires an explicit
App Server/Host-owned Project Approval Store and a defined re-confirmation rule;
it must not be written to Web `state.json`, and the presence of a Web manifest
must never restore machine-wide authorization.

The first offline Harness Fixture must cover at least:

1. `project-1/session-a` makes its first machine-wide shell/file request and
   receives one high-risk confirmation;
2. after the user approves `current_project`, `project-1/session-b` may reuse it
   only with the same `workspaceRevision`, `access=machine`, `actionClass`, and
   `pathScope`;
3. `project-2`, a `Project access` Session, a different action class, or a new
   root revision must request approval again;
4. after revocation, later requests from both Sessions cannot silently pass,
   and hard Deny still has precedence;
5. an App Server restart cannot restore the old Project machine-wide approval
   from Web state.

## Proposed protocol changes

### Thread settings

Extend the existing `thread/settings/update` contract with an optional typed
`approvalMode`; keep `collaborationMode` and bounded `builtinTools` in the
same Thread boundary. The result and `thread/settings/updated` notification
must return the effective values and existing `stateRevision`.

Do not reintroduce an aggregate `workflow/state` wire authority. An SDK helper
may combine settings and Goal for presentation, but the App Server remains
authoritative through the independent canonical methods.

### One typed control envelope across the chain

Every Web Studio control request and every approval/event projection should
carry the same bounded identity envelope:

```json
{
  "projectId": "project-1",
  "workspaceId": "workspace-1",
  "workspaceRevision": 3,
  "sessionId": "session-a",
  "threadId": "thread-1",
  "requestId": "request-1",
  "payload": {"kind": "goal_start"}
}
```

`projectId` and the immutable WorkspaceSpec binding refer to the selected Web
Project, but App Server must resolve them against its trusted Project registry;
a client-supplied path is not authoritative. `sessionId` and `threadId` come
from the selected Session. A client control request may carry an opaque RPC
correlation ID; App Server/Host create the authoritative `turnId`, `callId`,
and approval `requestId` at the execution boundary. FastAPI and the Python SDK
pass these fields as typed values and do not infer, rewrite, or drop them. App
Server rejects a missing, stale, or cross-Project/Session/Thread binding before
dispatch. In particular, no Web route may fall back to a global `default`
Thread or manufacture an execution identity.

The envelope is an association and routing contract, not model-visible prompt
content. Host attaches the effective access, SecurityPolicy result, approval
owner, and normalized `pathScope`; Core receives only the Host-admitted Tool
request/result and its own bounded Turn identity. Core must not be made aware
of Web Project state, and Web must not synthesize a successful Core/Host
outcome from a disconnected transport.

### Runtime startup inputs without Profile

App Server initialization and Runtime construction should receive direct typed
inputs: provider/model selection, bounded tool and extension selection,
prompt/rule sources, sandbox/security, access scope, and WorkspaceSpec. There
is no `profile` field in the new public startup contract. Once a Runtime/Session
is created, these startup inputs are immutable for that identity; changing them
requires an explicit Runtime recreation or a new Session. Profile-shaped startup
input is rejected; it is not parsed or translated.

### Approval protocol

Keep the method names `approval/request`, `approval/respond`, and
`approval/resolved`, but replace their parameters with the following typed,
bounded models. Do not invent a second approval transport or retain legacy
fields in the new public contract:

```json
{
  "requestId": "approval-1",
  "projectId": "project-1",
  "workspaceId": "workspace-1",
  "workspaceRevision": 3,
  "sessionId": "session-a",
  "threadId": "thread-1",
  "turnId": "turn-1",
  "callId": "call-1",
  "toolName": "shell",
  "actionClass": "shell_execute",
  "actionSummary": "shell command: cargo test",
  "pathScope": {"kind": "machine"},
  "access": "machine",
  "allowedApprovalModes": ["per_action", "current_session", "current_project"],
  "highRisk": true
}
```

`pathScope` must be a normalized, bounded path descriptor: Project scope may
refer only to roots in the current WorkspaceSpec, while machine scope uses an
explicit `machine` marker. An unbounded raw command, secret, or arbitrary
prompt text must never be stored as an approval rule. `actionSummary` is for UI
display and is not the sole admission identity.

```json
{
  "requestId": "approval-1",
  "decision": "approve",
  "access": "machine",
  "approval": "current_project",
  "reason": ""
}
```

`decision` accepts only `approve` or `deny`. `approval/resolved` must echo the
final access, approval mode, and correlation fields, and use a typed `outcome`
of `approved`, `denied`, `expired`, `revoked`, or `unavailable`. The new wire
contract rejects an `approved` boolean, legacy `remember`, or a boolean-only
approval response; internal implementation work must delete those paths rather
than continue to accept or translate them. The App Server must no longer
silently write a successful boolean approval into a global cache.

Host creates the request; App Server only validates correlation, queues, and
routes it; the SDK only transports and parses types; FastAPI only maps
connections; and Studio only submits the user decision. A response must match
an unsettled `requestId` and all of its correlation fields. Late, duplicate,
cross-Thread/Session/Project, or scope-expanding responses are rejected and
must not resume the tool call.

Host/Capabilities must use one admission order:

```text
validate identity and normalize pathScope
    → check that the tool is exposed and available
    → apply hard Security Deny
    → verify that access covers the target path/resource
    → apply Plan lock and Sandbox boundaries
    → if SecurityPolicy=Allow, execute without creating an approval entry
    → if SecurityPolicy=Ask, look up an exact approval-key match
    → on a miss, request a typed decision from the user
    → record a bounded approval for the selected lifetime only after approve, then execute
```

`Full access` therefore sets the current Runtime/Session's access scope to
machine-wide; it cannot bypass hard Deny, the Plan lock, Sandbox, or the tool
allowlist, and it does not guarantee that every tool skips confirmation.
`current_project` applies only to actions Host has classified as askable and
can reuse only an exact approval-key match. Together they provide the execution
scope and approval lifetime required by Auto Copilot, but they are still not an
unconditional allow-all.

`current_project` decisions are owned by the App Server/Host approval authority
and keyed by `projectId`; Web `state.json` may display or reference the setting
but cannot store or enforce it. A Project approval is shared across its
Sessions only when the bounded WorkspaceSpec revision, access scope, action
class, and path scope match. It is revocable and never becomes a process-global
allow rule.

### Final user-facing decision matrix

The Studio should make the following product contract visible without exposing
the internal Host objects:

| User intent | Mode | Access | Approval lifetime | Meaning |
| --- | --- | --- | --- | --- |
| Ordinary task | Chat | Project access | `per_action` (default) | Work inside the declared Project roots; ask again for each askable action |
| Explore and write a plan | Plan | Project access, or explicitly Full access when exploration needs machine-wide reads | `per_action` or an explicitly chosen session/project lifetime | Read-mostly exploration with bounded scratch; it does not authorize final Project edits |
| Scoped autonomous work | Goal | Project access | `current_project` | Continue across Turns and share exact, bounded approvals within the Project |
| Auto Copilot | Goal | Full access (machine-wide) | `current_project` | Continue across Turns with machine-wide admission and Project-owned approval reuse |

The three axes remain independent:

1. `Full access` changes the resource/path scope to machine-wide; by itself it
   does not approve any askable action or choose an approval lifetime.
2. `current_project` changes who may reuse a matching approval; by itself it
   does not upgrade `Project access` to machine-wide access.
3. `Goal` changes the execution lifecycle to execute → verify → continue; it
   does not grant access or approval.

Only the fourth row is the named Auto Copilot experience. The UI may offer it
as a convenience after separately showing the three underlying choices, but it
must persist and transmit those typed choices rather than a new `copilot`,
`profile`, or `allow_all` flag.

### SecurityPolicy remains an internal admission gate

The public `access` and `approval` axes do not select or override the Host's
security policy. The effective admission remains:

| Host result | Effect of the public choices |
| --- | --- |
| `Deny` | Reject the action. Neither `Full access` nor any approval lifetime can override it. |
| `Ask` | `Full access` may cover the target resource, but the action still needs a typed decision or an exact, valid approval entry. |
| `Allow` | Execute without creating an approval entry; this is a Host policy result, not a grant that Web or `current_project` can manufacture. |

`Full access` therefore means “the machine-wide scope may be eligible for
admission”; it does not mean “set SecurityPolicy to Allow for every tool”.
`current_project` only resolves an exact askable request for its bounded
lifetime; it must never turn `Deny` into `Ask`/`Allow` or widen a path scope.
The UI should expose the resulting state as blocked, waiting for approval,
approved by a Project grant, or executing, rather than showing a single
“allowed” boolean.

### Project-owned approval versus Session-effective settings

`current_project` is a Project-owned approval lifetime, so a matching grant
can be reused by Sessions in that Web Studio Project. The effective `access`
and `approval` values still belong to each Runtime/Session and must be visible
in its status. Joining a Project does not silently turn a Session into
`current_project`, and it never silently grants `Full access`.

The safe user flow is:

1. The user explicitly enables `current_project` for the Project approval
   surface and approves the first bounded action.
2. A new or resumed Session explicitly declares its effective access and
   approval mode; App Server may reuse the Project grant only when every key
   field matches.
3. Machine-wide `Full access` remains an explicit high-risk Runtime/Session
   choice. If a future “apply to all Sessions” control is offered, it must be
   a separate, visible Project action with high-risk confirmation, not an
   implicit Project default or a Profile.

This gives the Project the requested shared approval scope without making
Project membership an ambient machine-wide authorization.

### Project approval lifecycle

`current_project` means that an approval decision is shared by a Project; it
does not grant the Project an access scope by itself. Every new or resumed
Session must still carry a matching access setting. For example, a Project
approval can authorize a machine-wide action only when the current
Runtime/Session explicitly has `Full access` and the approval entry also has
`access=machine`. A `Project access` Session cannot be upgraded to machine-wide
access by sharing an approval.

- Project settings must expose the active shared approvals and a revoke action.
  Revocation blocks later admission immediately; an in-flight action may only
  finish at its cooperative boundary, and its side effect is not presented as
  rolled back;
- changing associated Project roots creates a new WorkspaceSpec revision. An
  approval from the old revision does not expand to new roots and requires a
  new explicit Project approval;
- a Session fork, new Project, or copied Project configuration does not inherit
  approval entries. Approval ownership always ends at the explicit `projectId`;
- Web may submit inspect/revoke requests but cannot mutate shared approvals
  through a local state file.

### Explicit compaction

Expose the already bounded Core/App Server compaction path through an explicit
Thread control method such as `thread/compact`. It must persist the resulting
checkpoint and emit the normal bounded `ContextCompaction`/turn projection. It
must not create a second history store or model-visible unbounded input path.

## Canonical storage, workspace scope, and Session lifecycle

### Decision: unify authority, not every file

The storage model should be unified around the Rust `SessionStore`, but Web
Studio's presentation state should remain a separate, smaller concern. The
important rule is one authority for durable conversation state, not one giant
JSON file containing runtime, UI, and project data.

The Rust Session Store already owns the append-only session log, settled
checkpoint, summary, signals, prompt context, attachments, and session lock.
Web Studio currently adds `~/.mini-agent/state.json` and
`~/.mini-agent/checkpoints/<threadId>.json`. The latter must not remain a
second resume authority. Web metadata may cache references and display fields,
but transcript, items, checkpoint, resume, and lock state must come from the
canonical Session Store through App Server/SDK adapters.

The target layout is:

```text
~/.mini-agent/
├── sessions/
│   └── <workspaceId>/
│       └── <sessionId>/
│           ├── workspace.json       # immutable WorkspaceSpec snapshot
│           ├── session.jsonl        # durable append-only authority
│           ├── summary.json         # bounded list/read projection
│           ├── signals.json         # bounded lifecycle signals
│           ├── prompt_context.json
│           ├── session.lock         # single-writer ownership
│           ├── attachments/
│           └── plan/
│               ├── plan.md             # retained explicit Plan artifact
│               ├── scratch/            # disposable exploration scripts/outputs
│               └── cleanup.json        # bounded cleanup manifest
├── web/
│   ├── state.json                   # projects, UI preferences, selections
│   └── session-index.json           # optional derived/cache index only
└── logs/
    └── <workspaceId>/<sessionId>/   # diagnostic logs, not conversation state
```

This is a direct cutover for the new implementation. Canonical Session Store
records under the target `sessions/` layout are the only durable conversation
and resume authority. The old single-root Web `state.json` and old Web
checkpoint directory are not read, imported, translated, written, or deleted.
They are outside the new runtime's scope and do not participate in Project or
Session discovery.

The new `~/.mini-agent/web/state.json` is a versioned Web manifest containing
only project registry, UI preferences, and references such as `projectId`,
`workspaceId`, `sessionId`, and `threadId`. It is not the old root-level
`state.json`, and it must not contain a transcript or a duplicate Thread
checkpoint.

Plan scratch belongs under the canonical Session directory, not under the
Project root and not under Web-only state. Its path is generated and bounded by
the Session/Plan Runtime. `plan.md` is the retained Plan artifact; scratch
contents are disposable and are removed according to `cleanup.json`.

There is no `~/.mini-agent/profile/` directory, per-Session Profile file, or
project-local `.agents/profile.json` input in the new contract. Profile files
are not read or translated, and no Profile identity is copied into canonical
Session history or Web state.

### Project, WorkspaceSpec, and Session binding

Web Studio's Project is a user-facing identity. It must resolve to an explicit
runtime `WorkspaceSpec`:

```text
Project
└── WorkspaceSpec
    ├── primaryRoot
    ├── associatedRoots[] (reference | editable)
    └── bounded write/execution policy
```

`workspaceId` is a stable identifier derived from the normalized, canonical,
ordered root manifest and its schema version; it must not be based on the
display name. Each Session stores an immutable copy in `workspace.json`, so a
later edit to a Project's associated directories cannot silently change the
meaning of an old conversation. Rebinding an existing Session requires an
explicit fork/new Session, not an in-place mutation of its workspace.

The control-plane shape should be small and explicit:

```json
{
  "schemaVersion": 1,
  "projectId": "project-1",
  "workspaceId": "workspace-1",
  "workspaceRevision": 3,
  "primaryRoot": {"path": "D:/work/app", "role": "primary"},
  "associatedRoots": [
    {"rootId": "root-ref-1", "path": "D:/work/docs", "role": "reference"},
    {"rootId": "root-edit-1", "path": "D:/work/shared", "role": "editable"}
  ]
}
```

The Host canonicalizes and validates every path before assigning the stable
identity; the example is a control-plane record, not model-authored context.
`planScratchRoot` is not a Project root and is never supplied by the model: it
is generated under the canonical Session directory for the selected Plan
Runtime. A Project workspace edit creates a new `workspaceRevision`; new
Sessions bind to that revision, while existing Sessions retain their snapshot.
The corresponding operations should be explicit (`project/workspace/update`,
`session/create` with a revision, and `session/attach` with an exact binding),
so no API can silently mutate a running Session's cwd or root list.

The current project API already accepts a primary path and multiple
`source_folders`, but the Web runtime currently launches the SDK with the
process cwd and does not pass the complete manifest into Host tool creation.
The implementation must close that gap across the chain:

```text
Project source_folders
    → WorkspaceSpec in FastAPI
    → SDK/App Server trusted runtime configuration
    → Host ToolBuildRequest.extra_read_roots + bounded write roots
    → Capabilities Workspace path admission
```

The root manifest is control-plane configuration, not model-authored input. It
must be validated, bounded, canonicalized, and included in the runtime/session
identity before tools are built.

### Workspace scope rules

The initial policy is deliberately asymmetric:

| Root kind | Read tools | `apply_patch`/create | Shell cwd | Default write authority |
| --- | --- | --- | --- | --- |
| `primaryRoot` | allowed | allowed subject to Plan/security/approval | `primaryRoot` | yes |
| `associatedRoots[]: reference` | allowed after canonical path check | denied | never changes cwd | no |
| `associatedRoots[]: editable` | allowed after canonical path check | allowed with explicit Project or machine access, Plan/security/approval | never changes cwd | yes, only for this root |
| Session-owned `planScratchRoot` | bounded Plan exploration | allowed for temporary scripts/outputs and cleanup only | `planScratchRoot` in Plan | temporary only |

Associated roots are not an implicit “all project folders are writable” grant.
Project editing must make the intent visible by adding a directory as either
`reference` or `editable`. Reference roots are passed to the existing bounded
`extra_read_roots`/`Workspace::with_read_roots` path; editable roots additionally
need an explicit bounded write-root admission path. Both kinds must be passed
to Runtime creation rather than merely shown in the UI. Nested/duplicate roots
are normalized; symlink and escape cases are rejected or reduced to the
canonical root manifest before Runtime creation.

Normal Chat and Goal shell execution starts in `primaryRoot`; Plan exploration
starts in `planScratchRoot`. Because a native child process can issue arbitrary
filesystem writes that file-tool checks cannot observe, associated-root access
from shell commands is not treated as a write grant. A Plan shell action that
writes outside its scratch root is a formal Project mutation, not exploration.
A strict enforcement mode must use the configured process sandbox or require
an explicit approval describing the affected path; the UI must not claim that
`per_action` alone makes native shell path writes safe. Docker/sandbox mounts
must include the declared Project roots plus the bounded scratch root, and
preserve the reference-versus-editable distinction where the sandbox supports
it.

The protocol needs small explicit limits for the root manifest (number of
associated roots, path length, and aggregate serialized bytes). The exact
constants belong to the protocol implementation batch; the list must never be
unbounded model-visible context.

### Session catalog and lifecycle

The product must distinguish live runtime state from Goal state:

| Dimension | Values | Authority |
| --- | --- | --- |
| Runtime | `running`, `idle`, `closed` | App Server Actor/turn state |
| Session UI projection | `running`, `paused`, `historical`, `locked` | Web projection of runtime + lock + summary |
| Connection projection | `connected`, `reconnecting`, `unknown` | App Server/transport reconciliation |
| Plan | `none`, `exploring`, `settling`, `cleanup_pending` | Selected Thread's Plan runtime |
| Goal | `none`, `active`, `paused`, `completed`, `failed` | Thread Goal lifecycle |

Goal lifecycle and stop reason are separate protocol fields. The bounded
`stopReason` vocabulary is `none`, `user_paused`, `goal_completed`,
`cancelled`, `awaiting_approval`, `blocked`, `usage_limited`,
`budget_limited`, `runtime_guard`, or `failed`. Wire values use these
snake_case names; Studio may render friendly localized labels, but must not
collapse a wait, a protection stop, or a client disconnect into `paused` or
`completed`.

“Paused Session” means a resumable Session whose active turn has been
interrupted or deliberately suspended. It must not be confused with
`Goal.status=paused`. An idle Session with no explicit pause reason may be
shown as historical/idle; its durable history remains resumable if the lock is
available. A closed Session is retained for history and can only be resumed by
an explicit attach/resume operation.

The single-writer rule is per Session: `session.lock` and the App Server owner
prevent two runtimes from mutating one session at once. Multiple Sessions may
run concurrently. A Web tab switch changes the selected Session and event
subscription; it must not cancel another Session's active turn. Approval
requests, active turn IDs, Goal controls, and pending UI state are keyed by
`sessionId`/`threadId` and never stored as one global Web value.

The authoritative read path should be bounded and explicit:

```text
App Server:  session/list(workspaceId, cursor, limit)
            session/inspect(sessionId)
            thread/read + thread/items/list for settled content
SDK:        list_sessions / inspect_session / resume_thread adapters
FastAPI:    project session list/read/resume/interrupt routes
Studio:     project-scoped history with Running / Paused / History groups
```

The exact method names can follow the existing naming convention, but the
contract must provide:

- a bounded, cursor-based Session list with `sessionId`, `threadId`,
  `projectId`, `workspaceId`, title/summary, `updatedAt`, runtime status,
  Goal status, active turn ID, checkpoint sequence, lock owner/read-only
  indication, and `resumable`;
- history reads from the canonical Session Store, never from a Web checkpoint
  copy;
- live Session reads from notifications while a turn is running, with only
  settled checkpoints used as resume authority;
- explicit resume/attach behavior that fails as read-only when another owner
  holds the lock, instead of forcefully merging or stealing the Session;
- interrupt/pause and resume transitions that remain correlated to the same
  Thread/Session and are visible after a reload.

For different WorkspaceSpecs, the Web backend must keep separate runtime
handles (or separate App Server Runtime instances) because a single process cwd
cannot represent multiple primary roots. Within one WorkspaceSpec, multiple
Threads/Sessions may be multiplexed only if App Server routing and SessionStore
locks remain Thread/Session-scoped. The current singleton Web client and
global `_active_turns`/approval presentation are therefore implementation
constraints to remove, not product semantics.

### History and switching behavior

The expected user flow is:

1. Project selection loads the Project's immutable WorkspaceSpec and a bounded
   Session catalog.
2. Selecting a historical Session reads its canonical summary and settled
   items, then attaches the corresponding Thread without creating a Web copy.
3. Selecting a running Session subscribes to its Thread events and shows the
   current active turn; it does not replay a partial turn as a settled history
   item.
4. Selecting a paused Session shows the last settled checkpoint, pause reason,
   and a Resume action. Resume reacquires the lock and continues from the
   canonical checkpoint; it does not duplicate the last user message.
5. Switching away leaves an independent running Session running, but approval
   decisions can only be submitted with its exact correlation fields and
   authorized owner. Returning reconciles the UI from App Server status/events
   and canonical summary rather than stale React state.

If a Session is locked by another process, Studio may browse its durable
history but must label it read-only and disable resume/approval controls. If a
runtime disconnects, status becomes `unknown/reconnecting` until App Server
reconciliation completes; the UI must not guess `paused` merely because its
WebSocket disappeared.

## Studio and command contract

The Web UI should keep one state object per active Thread and a map of pending
approvals by `requestId`. It must route notifications by `threadId`; a pending
approval from another Thread must not appear in the active Thread's composer.

The approval dock should expose two independent groups:

```text
Approval mode:
  This action only (per_action)
  Current Session
  Current Project (shared by all Sessions in this Project)

Access scope:
  Project access (current Project workspace)
  Full access (machine-wide, high risk)

Deny with reason
```

The default approval mode is `per_action`. `Full access` must be rendered as
machine-wide access with its high-risk confirmation and remaining
Deny/confirmation guardrails. `Project access` must identify the current
Project workspace and its reference/editable roots. The two access scopes and
the three approval modes must not be collapsed into a vague “allow everything”
label. The Auto Copilot combination is visibly `Goal + Full access + Current
Project approval`.
Project settings must also expose inspection and revocation of the current
Project's shared approvals; revocation must not auto-approve later requests.

The plus menu and slash commands should call typed APIs:

| Command | Action |
| --- | --- |
| `/status` | Read world, active Thread settings, Goal, and MCP status |
| `/compact` | Call explicit Thread compaction |
| `/plan on\|off` | Start/stop the active Thread's read-mostly exploration runtime |
| `/goal <objective>` | Create and start a bounded Thread Goal |
| `/goal start\|resume` | Start or resume the active Thread Goal |
| `/goal pause` | Pause the active Thread Goal without deleting its objective |
| `/goal update <objective>` | Update the active Goal objective |
| `/goal clear` | Clear the active Goal |
| `/mcp` / `/mcp retry` | Read or retry MCP state |
| `/review` | Launch an allowlisted review workflow |
| `/skill` / `/plugin` | List or select only discovered and approved entries |

Control commands must not become ordinary model prompts. Review, Skill, and
Plugin activation must remain allowlisted and bounded. Dynamic hot-loading of
arbitrary extension instructions is outside this proposal; if it needs a new
runtime activation contract, it receives a separate admission record.

### Persistent Goal header

When a Thread Goal is set through `/goal <objective>`, Studio must render a
persistent Goal header above the conversation for that selected Thread. The
header is a projection of the canonical Goal state, not a second UI-owned
workflow record, and remains visible across reloads and Session switching.

The header must expose:

- the bounded objective, Goal lifecycle (`active`, `paused`, `completed`, or
  `failed`), and optional stop reason (`blocked`, `usage_limited`,
  `budget_limited`, `runtime_guard`, `cancelled`, or `awaiting_approval`);
- Start/Resume when the Goal is not running;
- Pause when it is active;
- Update/Edit, which sends an explicit Thread Goal update;
- Delete/Clear, which clears the active Goal after the normal confirmation;
- the latest verifier/continuation summary when available, without embedding
  an unbounded transcript in the header.

`/goal <objective>` creates and starts the Goal. `/goal pause` preserves the
objective for later `/goal start` or `/goal resume`; `/goal clear` deletes the
active Goal configuration while the Session history remains available. When
the user switches Threads, the header changes to the selected Thread's Goal
and must not display another Thread's objective or controls.

## Implementation batches

### Batch 0 — Contract and trace fixtures

Document the user control axes, access/approval vocabulary, Thread-owned
Plan/Goal runtime lifecycle, internal safety-guard semantics, and correlation
rules. Define the direct startup inputs that replace Profile and the
non-configurable Core guard that replaces routine `max_steps`. Add offline
protocol fixtures before changing execution, prioritizing the complete
`Full access + current_project + Goal` approval loop and the two-Session Project
sharing boundary. No provider call is needed.

### Batch 1 — Canonical storage, WorkspaceSpec, and Session catalog

- define the versioned `WorkspaceSpec`, stable `workspaceId`, immutable
  per-Session `workspace.json`, and bounded root-manifest limits;
- pass the Project primary root and associated roots, including each root's
  `reference`/`editable` intent, from Web Studio all the way to Host tool
  construction;
- make the primary root the agent cwd/write root and enforce explicit
  reference/editable root admission; add path, symlink, sandbox, and
  native-shell boundary tests;
- implement distinct Project access and machine-wide `Full access` presets;
  preserve `SecurityPreset::FullMachine` guardrails and distinguish configured
  roots from one-off paths mentioned in a user message;
- add the Session-owned Plan scratch root, retained `plan.md`, bounded cleanup
  manifest, and cleanup-pending recovery path;
- add the bounded App Server/SDK/Web session list and inspect path backed by
  canonical `SessionStore` summaries and locks;
- move new Web state under the Web-owned subdirectory, remove ongoing writes to
  duplicate Web checkpoints, and ensure old Web state/checkpoint paths are
  never read, imported, or written;
- expose per-Session runtime/Goal status, active turn, lock, and resumable
  fields; remove the assumption that one global Web client or one global
  active state represents all Projects;
- implement project-scoped history selection and attach/read-only behavior
  before enabling concurrent running/paused Session switching.

This batch is the storage and identity prerequisite for approval scope. It is
also the point at which the implementation must decide whether one App Server
process multiplexes immutable WorkspaceSpecs or the Web keeps one runtime
handle per WorkspaceSpec; a single mutable cwd is not an acceptable third
state.

### Batch 2 — Approval correctness and routing

- make `per_action` genuinely one-shot at the request boundary;
- make `current_session` and `current_project` approval ownership explicit
  bounded decision entries;
- expose Project-scoped approval inspection/revocation and invalidate grants
  when the WorkspaceSpec revision changes;
- carry `project` versus `machine` access scope separately from approval
  mode;
- preserve Deny and Plan lock precedence;
- pass access scope and approval mode through App Server → SDK → FastAPI →
  Studio;
- remove Web's duplicate approval authority;
- route pending requests by Thread and request ID;
- add Core/Host/App Server boundary tests and one SDK/Web approval scenario.

This is the first implementation batch and is a security correctness gate.

### Batch 3 — Active Thread control

- pass active `threadId` through every Plan, Goal, Builtin, and status call;
- hydrate settings and Goal independently after Thread selection;
- emit and consume authoritative settings/Goal notifications;
- bind Plan runtime state and Goal lifecycle to the selected Thread; remove
  profile updates from the live control path entirely;
- make Plan exploration, settlement, cleanup, and `plan.md` retention ordered
  Thread events rather than Web-local state;

### Batch 4 — SDK and FastAPI convergence

- add typed approval request/decision models;
- use typed approval callbacks and decisions only; remove bool/string approval
  callback paths;
- ensure reconnect, timeout, cancellation, and late resolution behavior is
  deterministic;
- remove obsolete Web-side remembered approval state and broad broadcasts.

### Batch 5 — Studio control surface

- show the current Mode and Goal state, with `/plan` and `/goal` as the explicit
  activation controls;
- show Plan exploration/cleanup state and the retained `plan.md` without
  presenting Plan as read-only;
- keep Security/Approval as a separate setting and approval dock, with
  `Project access` and machine-wide `Full access` clearly distinguished;
- remove `interactive`/`ask`/`auto` and `profile=auto` from the normal and
  runtime control paths; expose only effective bounded runtime inputs in
  advanced status;
- render an exceptional runtime-guard outcome as a protection diagnostic, not
  as “8 steps”, “step limit reached”, or “interrupted”;
- implement the plus menu and approval dock;
- add active Thread status indicators for mode, Goal, approval mode, MCP,
  and Runtime revision.

### Batch 6 — Slash and review workflows

Implement `/status`, `/compact`, `/plan`, `/goal`, and `/mcp` first. Add `/review`
and Skill/Plugin selection only through existing allowlists and after the
corresponding capability evidence is accepted.

## Acceptance evidence

The following scenarios are required before the proposal can move to
`implemented/`:

1. `per_action` causes a second identical action to request approval again.
2. `current_session` approval affects only the intended Thread/Session, while
   `current_project` approval is shared by all Sessions in the same Project
   only when access scope, WorkspaceSpec revision, action class, and path scope
   match; it never affects another Project, and revocation blocks later
   admission.
3. `project` access/approval does not affect another Project or unrelated
   action class.
4. Security Deny cannot be overridden by any UI scope.
5. Plan Mode permits declared-root reads, bounded exploration commands, temporary
   scripts/outputs under `planScratchRoot`, and the retained `plan.md`; formal
   Project source/configuration mutation remains behind the Plan gate.
6. Goal set, update, clear, resume, verifier, and continuation events remain
   ordered and Thread-scoped. A settled Goal Turn is followed by verifier
   evidence; an incomplete/rejected verdict schedules a new Turn with the
   same `threadId`, while an approved verdict completes the Goal.
7. Two active Threads cannot display or resolve each other's approvals.
8. Web Studio and the REPL have no Profile control or retained Profile identity.
   Profile-shaped input, including `profile=auto`, is rejected. The sole
   migration-only exception is `turbomode`/`Turbomode`, which maps once to
   `Full access` / `SecurityPreset::FullMachine`, is discarded, and cannot
   change mode or loop semantics.
9. `/status`, `/plan`, `/goal`, `/mcp`, and `/compact` use control APIs rather
   than accidental model prompts.
10. `item/started`, approval events, `approval/resolved`, `item/completed`, and
    `turn/read` preserve the same bounded call identity and final outcome.
11. A Project with one primary root and multiple associated roots can read
    files from every declared root; `reference` roots are read-only and
    `editable` roots can be written only through explicit Project or machine
    access, subject to Plan/security/approval gates; Plan scratch writes remain
    isolated in `planScratchRoot`.
12. The user-facing `Project access` preset is limited to the declared Project
    workspace, while `Full access` explicitly covers machine-wide scope for
    the current Runtime/Session. Both preserve hard security Deny; shell
    confirmation and other remaining guardrails are visible in the details.
    The Auto Copilot composition is explicitly `Goal + Full access +
    current_project` approval and requires a high-risk confirmation.
13. Changing a Project's associated roots does not change an existing Session's
   `workspace.json`; a new Session or explicit fork receives the new manifest.
14. A path mentioned only in a message can be used as bounded reference
    context, while a request to sync/update it creates explicit path-scoped
    intent; active machine-wide `Full access` may admit it under its guardrails,
    otherwise it requires explicit approval or an explicit Project-root
    addition.
15. Legacy Web `state.json` and checkpoint paths are never read, imported,
    translated, written, or deleted. Project history and resume use only the
    canonical `SessionStore` and the new derived Web manifest/index.
16. Project history lists bounded historical, running, paused, and locked
    Sessions with stable `sessionId`/`threadId`/`workspaceId` correlations.
17. Switching to a running Session attaches to its event stream without
    canceling another running Session; switching to a paused Session resumes
    only after the canonical lock is reacquired and never duplicates a turn.
18. A Session locked by another owner remains readable as history but cannot
    be resumed or approve a pending action; a WebSocket disconnect is shown as
    reconnecting/unknown until server reconciliation.
19. `/plan` starts/stops the selected Thread's Plan exploration runtime, while
    `/goal <objective>` creates and starts a Goal whose persistent header
    supports Start/Resume, Pause, Update, and Delete without exposing another
    Thread's Goal.
20. Plan scratch scripts and outputs are removed on normal settlement,
    cancellation, or explicit exit; cleanup failure is visible as
    `cleanup_pending`, while `plan.md` and its bounded summary remain.
21. No user-facing `max_steps` or routine `step_limit` control exists. Plan and
    Goal runtime state owns continuation and completion; an exceptional Core
    safety-guard outcome is preserved as an internal diagnostic and rendered
    as protection-triggered/inspect-or-retry, while only an actual cancellation
    is rendered as “interrupted/cancelled”.
22. A fresh Goal uses the proposed `100 / 200 / 1800` runtime defaults unless
    an operator deliberately lowers them for a fixture; an explicitly supplied
    `tokenBudget` remains an independent resource limit and reports
    `budget_limited` when reached.
23. Editing Project roots, forking a Session, or copying a Project creates no
    inherited approval entry; a new WorkspaceSpec revision requires explicit
    Project approval, and Project revocation invalidates later matching
    requests.
24. An offline Harness Fixture traces one Auto Copilot approval round trip across
    `projectId → workspaceId → workspaceRevision → sessionId → threadId →
    turnId → callId → requestId`; a second Session in the same Project reuses
    approval only when every bounded field matches, while another Project or a
    mismatched scope requests again.
25. `current_project` approval is shared across Session switching and new
    Sessions within the same App Server control plane; revocation, a
    WorkspaceSpec revision change, and App Server restart prevent later reuse.
    Web `state.json` never restores the approval.
26. The approval fixture accepts `decision=approve|deny` and typed `outcome`,
    echoes every correlation field, and rejects an `approved` boolean,
    `remember`, or late, duplicate, cross-Project, or scope-expanding responses.
27. The Host admission trace proves that `Full access` only expands access
    scope and `current_project` only reuses exact approvals for askable actions;
    hard Deny, Plan lock, Sandbox, and tool availability cannot be overridden by
    either UI selection.
28. A Goal milestone's `1800s` default is not misreported as interrupted because
    of the SDK's `60s` wait or the Web approval wait; approval expiry produces
    `expired`, Core/Host/transport capacity limits produce a clear protection
    state, and ApprovalStore overflow never clears another Project's grants.
29. A Goal control call remains `running` or settles according to App Server
    authority across SDK/Web wait deadlines and reconnects; only explicit
    cancel/pause or an authoritative outcome changes the Runtime classification,
    and expired/capacity-rejected approvals cannot later resume a Tool call.
30. `project/workspace/update` creates a new `workspaceRevision`; `session/create`
    and `session/attach` bind an exact revision, existing `workspace.json` files
    remain unchanged, and `planScratchRoot` is Session-owned rather than a
    Project root.
31. The server resolves Project and WorkspaceSpec identity from its trusted
    registry, rejects stale/cross-scope envelopes, and creates authoritative
    execution IDs; Web cannot fall back to `default` or manufacture a `callId`.
32. A Project-owned `current_project` grant is reusable by matching Sessions,
    but Project membership alone never grants `Full access` or changes a
    Session's effective approval mode; any apply-to-all control is explicit and
    high-risk.

Provider-backed verification remains opt-in and must not use paid calls by
default. The normal evidence path uses mock providers, protocol fixtures, and
Harness scenarios.

## Change admission

1. **Layer:** Capabilities/Host owns WorkspaceSpec validation and tool scope;
   SessionStore remains the durable Capabilities/Host boundary; App Server
   exposes bounded Session management and live status; Python SDK/FastAPI/Web
   adapt Project/session listing and switching. Core changes only if an
   existing bounded checkpoint seam needs an adapter; Core does not own UI,
   project registry, filesystem persistence policy, or approval dialogs.
2. **Existing owner:** Reuse `SessionStore`, `session_directory`, settled
   checkpoint rules, `Workspace::with_read_roots`, `ToolBuildRequest`,
   `ApprovalController`, `ApprovalStore`, `ApprovalBroker`, Thread Settings,
   Goal Runtime, `ThreadListener`, SDK notification handling, and existing
   WebSocket routes. No second transcript/checkpoint store, router, workflow
   service, turn loop, or policy framework is admitted.
3. **Replace vs. add:** Remove the old Web checkpoint authority and use a
   reference/cache index derived from canonical SessionStore records; no
   migration/import path is added. Replace mutable process-cwd
   assumptions with an immutable WorkspaceSpec per runtime/Session. Extend
   existing Session, Thread, and tool-build seams with bounded root/status
   metadata, then add scope-aware approval; do not introduce a generic storage
   framework.
4. **Net line delta:** The 2026-09-03 baseline is runtime
   `19,554/20,000` and release Rust `29,328/30,000`; remaining margins are 446
   and 672 lines. The proposal is not one unbounded implementation batch. Every
   Rust batch must identify a measured offset or remain net-zero, run
   `python scripts/line_budget.py`, and record actual before/after counts.
5. **Visible surface:** Add only bounded WorkspaceSpec root metadata,
   Session catalog/status fields, access scope and approval mode metadata,
   typed control fields, and Thread-scoped notifications. Do not expose
   arbitrary prompt replacement, unlimited root lists/paths, unrestricted
   extension activation, or unbounded event payload. Reference roots are
   read-only; editable roots require an explicit Project or machine access
   decision and remain approval/Plan constrained. Plan scratch roots,
   `plan.md`, and cleanup state are bounded Thread-owned artifacts. Do not add
   a standalone Profile layer or make Profile names a user-facing control.
6. **Boundary evidence:** Existing Session, Workspace, Protocol, Host, App
   Server, SDK, and Web tests cover portions of the path. New multi-root path
   admission, canonical storage, Session list/lock, historical/running/paused
   switching, approval-scope, Thread-routing, Plan/Goal, compaction, and slash
   scenarios are mandatory because unit tests alone cannot prove the
   end-to-end control-plane trace.

## Change test

- **Hypothesis:** Independent Project/machine access, bounded approval mode,
  Project-shared approval ownership, and Thread-owned Plan/Goal runtimes let a client reproduce
  Codex-like controls without weakening Host security, losing multi-root
  boundaries, or duplicating execution state. Removing Profile and routine
  step-budget controls keeps ownership understandable.
- **Distinguishing trace:** `projectId → workspaceId → sessionId → threadId →
  turnId → callId → requestId`, followed by root admission, Session lock/status,
  access scope/approval owner, resolved outcome, ToolItem completion, Plan scratch
  cleanup state, retained `plan.md`, stop reason, and canonical readback. The
  same trace must prove that history selection and a paused Session resume do
  not create a duplicate checkpoint or turn. It must also distinguish a
  Project-configured editable root from a one-off message path used only for
  reference or an explicitly approved sync/update.
- **Why it cannot live only in a host adapter:** root admission and Session
  identity must cross the App Server/SDK boundary, while canonical history and
  lock ownership must be observable by Web Studio. A UI adapter alone cannot
  preserve either invariant; a host-only root list would also leave the current
  Web process-cwd gap unresolved.
- **Permanent complexity:** one typed bounded WorkspaceSpec, one canonical
  SessionStore ownership path, one bounded Session catalog projection, one
  scope-aware approval contract, one bounded Plan scratch/cleanup path, and one
  internal Core runaway guard. Generic
  hooks, Profile/policy engines, storage frameworks, or extension frameworks
  are explicitly excluded.

## Non-goals

- Do not modify `D:/gh-ws/codex` or copy the official Codex repository.
- Do not equate machine-wide `Full access` with unrestricted “allow all”:
  hard security Deny and documented shell/confirmation guardrails remain.
- Do not add a second Core execution loop, Goal verifier history, or Web-side
  persistence authority.
- Do not read, import, translate, write, or delete the old Web `state.json` or
  checkpoint directory. They are outside the new runtime; the new Web
  manifest/index is derived presentation state, not a second Session database.
- Do not make associated Project directories implicitly writable, and do not
  claim that native shell cwd alone enforces multi-root filesystem isolation.
- Do not support two concurrent writers for one Session or silently rebind an
  existing Session to a changed Project workspace manifest.
- Do not make Plan strictly read-only, and do not let its scratch root become
  an undeclared durable Project write area.
- Do not make arbitrary raw system-prompt replacement public.
- Do not make dynamic Skill/Plugin hot-loading part of the first approval batch.
- Do not add a standalone `Profile` layer or retain `interactive`/`ask`/`auto`,
  `profile=auto`, or `turbomode`/`Turbomode` as Runtime/Session identity. The
  sole migration-only `turbomode` alias maps to `FullMachine` and is discarded;
  no other migration parser or compatibility alias exists.
- Do not expose `max_steps` or routine `step_limit` as a task control; the Core
  safety guard remains internal and continuation belongs to Plan/Goal runtime
  state.

The external alignment principle is to make autonomy and approval boundaries
explicit, name safe local actions, and require confirmation for destructive,
external, or scope-expanding actions, consistent with the [official OpenAI
guidance](https://developers.openai.com/api/docs/guides/latest-model).
