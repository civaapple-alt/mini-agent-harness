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

The control plane must keep three independent dimensions separate:

```text
Host Profile (startup): interactive | ask | auto
Thread Mode (session): chat | plan | goal
Approval (runtime): policy + bounded decision scope
```

The runtime also binds every Thread/Session to one immutable workspace
manifest:

```text
Workspace Binding (Session): primaryRoot + associatedReadRoots + write policy
```

Plan and Goal remain the existing canonical Thread boundaries. This proposal
does not create a second Workflow service, a second turn loop, or a generic
policy/plugin framework.

## Current evidence and problem statement

The current implementation has the required pieces, but their authority and
semantics are split across the three repositories:

1. `ApprovalController` caches every approved action in `ApprovalStore`. As a
   result, a UI choice intended as “allow once” can behave like “always allow”.
2. `ApprovalRespondParams` carries `remember`, but the App Server response path
   currently consumes only the boolean approval result. The requested scope is
   therefore lost before Host execution resumes.
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

### Profile is not the same as Mode

The Host profile remains an initialization-time composition of provider,
prompt/rule sources, extension depth, tool scope, sandbox, and security preset.
It is not a live replacement for Plan or Goal.

The Studio main control should therefore present:

| Studio label | Canonical operation | Runtime meaning |
| --- | --- | --- |
| Interactive / Chat | `thread/settings/update` with `collaborationMode=default` | Ordinary user-driven Turns |
| Plan | `thread/settings/update` with `collaborationMode=plan` | Read-only project mutations; living plan remains writable |
| Goal | `thread/goal/set|get|clear` | Thread-owned autonomous lifecycle and verifier |

The current Host profiles `interactive`, `ask`, and `auto` remain available as
startup or advanced configuration. Selecting a different Host profile at
runtime must either restart/recreate the App Server Runtime or be explicitly
described as “effective on next Runtime”. It must not silently update only a
Python preference field.

### Approval policy and approval scope are different

`per_action` describes when an action must ask. It does not mean that every
action is automatically allowed. Tool exposure, Host security policy, Plan
lock, and human approval remain separate gates.

The public decision vocabulary is:

```text
policy: per_action | auto_approve | strict
scope:  once | session | workspace
```

Rules:

- `once` approves only the current `requestId`;
- `session` allows the same bounded action class within the current Thread or
  App Server session, according to the selected lifetime;
- `workspace` allows only the current workspace, tool, and normalized action
  class; it is not unrestricted machine-wide permission;
- explicit security Deny always wins over a UI approval;
- Plan Mode mutation locks cannot be bypassed by approval;
- `auto_approve` may skip interactive waiting for admissible actions but cannot
  override explicit Deny or Plan restrictions;
- `strict` denies approval-gated actions without opening a user prompt.

The approval key must include the bounded owner and action identity, for
example:

```text
(workspace root, thread/session scope, tool name, normalized action class)
```

Raw unbounded commands, secrets, or arbitrary prompt text must not become an
approval rule.

## Ownership model

| Layer | Owns | Must not own |
| --- | --- | --- |
| Core | Turn loop, limits, stop classification, events, history writeback | UI, approval dialogs, persistence, profile composition |
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

## Proposed protocol changes

### Thread settings

Extend the existing `thread/settings/update` contract with an optional typed
`approvalPolicy`; keep `collaborationMode` and bounded `builtinTools` in the
same Thread boundary. The result and `thread/settings/updated` notification
must return the effective values and existing `stateRevision`.

Do not reintroduce an aggregate `workflow/state` wire authority. An SDK helper
may combine settings and Goal for presentation, but the App Server remains
authoritative through the independent canonical methods.

### Approval lifecycle

Keep the existing methods and add bounded fields rather than inventing a second
approval transport:

```json
{
  "requestId": "approval-1",
  "threadId": "thread-1",
  "turnId": "turn-1",
  "callId": "call-1",
  "toolName": "shell",
  "action": "shell command `cargo test`",
  "scopes": ["once", "session", "workspace"]
}
```

```json
{
  "requestId": "approval-1",
  "approved": true,
  "scope": "once",
  "reason": ""
}
```

`approval/resolved` must echo the final scope and correlation fields. For
wire-compatibility, accept legacy `remember`: `false` maps to `once`, `true`
maps to `session`; new clients use `scope`. The App Server must no longer
silently remember every successful boolean approval.

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
│           └── plan/goal artifacts when enabled
├── web/
│   ├── state.json                   # projects, UI preferences, selections
│   └── session-index.json           # optional derived/cache index only
└── logs/
    └── <workspaceId>/<sessionId>/   # diagnostic logs, not conversation state
```

This is a semantic unification rather than a requirement to move every
diagnostic file in one change. Existing single-root session directories remain
readable during migration. The Web checkpoint directory becomes a
compatibility reader/import source and then stops receiving writes; it must
never win over a newer canonical checkpoint. A migration must be idempotent,
must not overwrite a canonical session, and must report an orphaned legacy
checkpoint instead of silently attaching it to the wrong project.

`state.json` can remain a versioned Web manifest during the first migration,
but it should live under `~/.mini-agent/web/` and contain only project
registry, UI preferences, and references such as `projectId`, `workspaceId`,
`sessionId`, and `threadId`. It must not contain a transcript or a duplicate
Thread checkpoint.

### Project, WorkspaceSpec, and Session binding

Web Studio's Project is a user-facing identity. It must resolve to an explicit
runtime `WorkspaceSpec`:

```text
Project
└── WorkspaceSpec
    ├── primaryRoot
    ├── associatedReadRoots[]
    └── bounded write/execution policy
```

`workspaceId` is a stable identifier derived from the normalized, canonical,
ordered root manifest and its schema version; it must not be based on the
display name. Each Session stores an immutable copy in `workspace.json`, so a
later edit to a Project's associated directories cannot silently change the
meaning of an old conversation. Rebinding an existing Session requires an
explicit fork/new Session, not an in-place mutation of its workspace.

The current project API already accepts a primary path and multiple
`source_folders`, but the Web runtime currently launches the SDK with the
process cwd and does not pass the complete manifest into Host tool creation.
The implementation must close that gap across the chain:

```text
Project source_folders
    → WorkspaceSpec in FastAPI
    → SDK/App Server trusted runtime configuration
    → Host ToolBuildRequest.extra_read_roots
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
| `associatedReadRoots[]` | allowed after canonical path check | denied | never changes cwd | no |

Associated roots are read roots, not an implicit “all project folders are
writable” grant. They must be passed to the existing bounded
`extra_read_roots`/`Workspace::with_read_roots` path rather than merely shown
in the UI. Nested/duplicate roots are normalized; symlink and escape cases are
rejected or reduced to the canonical root manifest before runtime creation.

Native shell execution starts in `primaryRoot`. Because a native child process
can issue arbitrary filesystem writes that file-tool checks cannot observe,
associated-root access from shell commands is not treated as a write grant. A
strict enforcement mode must use the configured process sandbox or require an
explicit approval describing the affected path; the UI must not claim that
`per_action` alone makes native shell path writes safe. Docker/sandbox mounts
must include only the declared roots and preserve the same primary-versus-read
only distinction where the sandbox supports it.

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
| Goal | `none`, `active`, `paused`, `completed`, `failed` | Thread Goal lifecycle |

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

The approval dock should expose:

```text
请求批准
允许一次
本会话允许
当前工作区范围允许
拒绝并说明原因
```

“完全范围” must be rendered with its actual bounded scope, such as “当前
工作区 + 当前工具类别”，not as unrestricted “allow everything”.

The plus menu and slash commands should call typed APIs:

| Command | Action |
| --- | --- |
| `/status` | Read world, active Thread settings, Goal, and MCP status |
| `/compact` | Call explicit Thread compaction |
| `/plan on\|off` | Update active Thread collaboration mode |
| `/goal <objective>` | Set a bounded Thread Goal |
| `/goal clear` | Clear the active Goal |
| `/mcp` / `/mcp retry` | Read or retry MCP state |
| `/review` | Launch an allowlisted review workflow |
| `/skill` / `/plugin` | List or select only discovered and approved entries |

Control commands must not become ordinary model prompts. Review, Skill, and
Plugin activation must remain allowlisted and bounded. Dynamic hot-loading of
arbitrary extension instructions is outside this proposal; if it needs a new
runtime activation contract, it receives a separate admission record.

## Implementation batches

### Batch 0 — Contract and trace fixtures

Document the state axes, approval scope vocabulary, and correlation rules. Add
offline protocol fixtures before changing execution. No provider call is
needed.

### Batch 1 — Canonical storage, WorkspaceSpec, and Session catalog

- define the versioned `WorkspaceSpec`, stable `workspaceId`, immutable
  per-Session `workspace.json`, and bounded root-manifest limits;
- pass the Project primary root and associated read roots from Web Studio all
  the way to Host tool construction;
- make the primary root the agent cwd/write root and associated roots explicit
  read roots; add path, symlink, sandbox, and native-shell boundary tests;
- add the bounded App Server/SDK/Web session list and inspect path backed by
  canonical `SessionStore` summaries and locks;
- move Web state under the Web-owned subdirectory, remove ongoing writes to
  duplicate Web checkpoints, and add an idempotent legacy checkpoint migration
  or compatibility read path;
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

- make `once` genuinely one-shot;
- make `session`/`workspace` explicit bounded cache entries;
- preserve Deny and Plan lock precedence;
- pass scope through App Server → SDK → FastAPI → Studio;
- remove Web's duplicate approval authority;
- route pending requests by Thread and request ID;
- add Core/Host/App Server boundary tests and one SDK/Web approval scenario.

This is the first implementation batch and is a security correctness gate.

### Batch 3 — Active Thread control

- pass active `threadId` through every Plan, Goal, Builtin, and status call;
- hydrate settings and Goal independently after Thread selection;
- emit and consume authoritative settings/Goal notifications;
- stop treating profile preference changes as live Runtime changes unless a
  restart/recreate operation is explicitly performed.

### Batch 4 — SDK and FastAPI convergence

- add typed approval request/decision models;
- keep bool/string approval callbacks only as compatibility adapters;
- ensure reconnect, timeout, cancellation, and late resolution behavior is
  deterministic;
- remove obsolete Web-side remembered approval state and broad broadcasts.

### Batch 5 — Studio control surface

- rename the main selector to Mode: Interactive, Plan, Goal;
- keep Security/Approval as a separate selector;
- implement the plus menu and approval dock;
- add active Thread status indicators for mode, Goal, approval policy, MCP,
  and Runtime revision.

### Batch 6 — Slash and review workflows

Implement `/status`, `/compact`, `/plan`, `/goal`, and `/mcp` first. Add `/review`
and Skill/Plugin selection only through existing allowlists and after the
corresponding capability evidence is accepted.

## Acceptance evidence

The following scenarios are required before the proposal can move to
`implemented/`:

1. `allow once` causes a second identical action to request approval again.
2. `session` approval affects only the intended Thread/session.
3. `workspace` approval does not affect another workspace or unrelated action
   class.
4. Security Deny cannot be overridden by any UI scope.
5. Plan Mode defers project mutation while allowing living-plan mutation.
6. Goal set, update, clear, resume, verifier, and continuation events remain
   ordered and Thread-scoped.
7. Two active Threads cannot display or resolve each other's approvals.
8. Profile changes are either effective after Runtime recreation or clearly
   reported as next-Runtime settings.
9. `/status`, `/plan`, `/goal`, `/mcp`, and `/compact` use control APIs rather
   than accidental model prompts.
10. `item/started`, approval events, `approval/resolved`, `item/completed`, and
    `turn/read` preserve the same bounded call identity and final outcome.
11. A Project with one primary root and multiple associated roots can read
    files from every declared associated root, while `apply_patch` and create
    remain limited to the primary root by default.
12. A root outside the Project manifest, a symlink escape, and an undeclared
    shell write are denied or require the documented sandbox/approval path; the
    UI never presents associated roots as unrestricted write scope.
13. Changing a Project's associated roots does not change an existing Session's
    `workspace.json`; a new Session or explicit fork receives the new manifest.
14. Legacy Web checkpoints can be discovered and migrated/reported
    idempotently, while canonical `SessionStore` history always wins and no
    second checkpoint is written afterward.
15. Project history lists bounded historical, running, paused, and locked
    Sessions with stable `sessionId`/`threadId`/`workspaceId` correlations.
16. Switching to a running Session attaches to its event stream without
    canceling another running Session; switching to a paused Session resumes
    only after the canonical lock is reacquired and never duplicates a turn.
17. A Session locked by another owner remains readable as history but cannot
    be resumed or approve a pending action; a WebSocket disconnect is shown as
    reconnecting/unknown until server reconciliation.

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
3. **Replace vs. add:** Replace Web checkpoint authority with a reference/cache
   index and an idempotent migration path. Replace mutable process-cwd
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
   Session catalog/status fields, approval scope metadata, typed control fields,
   and Thread-scoped notifications. Do not expose arbitrary prompt
   replacement, unlimited root lists/paths, unrestricted extension activation,
   or unbounded event payload. Associated roots are read-only by default.
6. **Boundary evidence:** Existing Session, Workspace, Protocol, Host, App
   Server, SDK, and Web tests cover portions of the path. New multi-root path
   admission, storage migration, Session list/lock, historical/running/paused
   switching, approval-scope, Thread-routing, Plan/Goal, compaction, and slash
   scenarios are mandatory because unit tests alone cannot prove the
   end-to-end control-plane trace.

## Change test

- **Hypothesis:** Explicit approval scopes and Thread ownership let a client
  reproduce Codex-like controls without weakening Host security, losing
  multi-root boundaries, or duplicating execution state.
- **Distinguishing trace:** `projectId → workspaceId → sessionId → threadId →
  turnId → callId → requestId`, followed by root admission, Session lock/status,
  approval scope, resolved outcome, ToolItem completion, and canonical
  readback. The same trace must prove that history selection and a paused
  Session resume do not create a duplicate checkpoint or turn.
- **Why it cannot live only in a host adapter:** root admission and Session
  identity must cross the App Server/SDK boundary, while canonical history and
  lock ownership must be observable by Web Studio. A UI adapter alone cannot
  preserve either invariant; a host-only root list would also leave the current
  Web process-cwd gap unresolved.
- **Permanent complexity:** one typed bounded WorkspaceSpec, one canonical
  SessionStore ownership path, one bounded Session catalog projection, and one
  scope-aware approval contract. Generic hooks, policy engines, storage
  frameworks, or extension frameworks are explicitly excluded.

## Non-goals

- Do not modify `D:/gh-ws/codex` or copy the official Codex repository.
- Do not implement unrestricted machine-wide “allow all” approval.
- Do not add a second Core execution loop, Goal verifier history, or Web-side
  persistence authority.
- Do not make the Web `state.json` or checkpoint directory a second Session
  database; migration is compatibility work, not a parallel long-term store.
- Do not make associated Project directories implicitly writable, and do not
  claim that native shell cwd alone enforces multi-root filesystem isolation.
- Do not support two concurrent writers for one Session or silently rebind an
  existing Session to a changed Project workspace manifest.
- Do not make arbitrary raw system-prompt replacement public.
- Do not make dynamic Skill/Plugin hot-loading part of the first approval batch.

The external alignment principle is to make autonomy and approval boundaries
explicit, name safe local actions, and require confirmation for destructive,
external, or scope-expanding actions, consistent with the [official OpenAI
guidance](https://developers.openai.com/api/docs/guides/latest-model).
