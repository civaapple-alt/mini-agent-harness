# Web Studio 集成运行指南

Status: current cross-repository integration guide

Scope: `mini-agent-core` → Host → App Server → Python SDK → FastAPI Gateway →
Web Studio. This document describes the boundary between the two repositories
at the current protocol version 1; it does not replace the App Server wire
contract or the Web repository's local implementation README files.

## Runtime topology and ownership

```text
Web Studio browser
    │ REST / WebSocket
    ▼
FastAPI Gateway (mini-agent-web/server)
    │ one MiniAgentClient per live Thread
    ▼
Python SDK (mini-agent-web/sdk/python)
    │ stdio JSON-RPC 2.0, protocol version 1
    ▼
mini-agent-app-server
    │ Thread / Turn / Goal / Item / policy control
    ▼
Host → Capabilities → Core
```

请求控制面从 Web Studio 向内进入 App Server；一次模型回合的执行与结果则沿
`Core → Host → Capabilities → App Server → Python SDK → Gateway → Web Studio`
向外返回。Core 产生的 `ToolExecutionOutcome` 在 Host/Capabilities 边界保留
`ToolExecutionStatus`，App Server 只负责有序投影；Gateway、SDK 和前端消费
`outcome`，不复制准入、审批或重试分类权威。

| Concern | Authority | Gateway responsibility |
| --- | --- | --- |
| Model steps, tool loop, limits, stop classification, Core events | Core | Relay the bounded projection. |
| Workspace tools, process, sandbox, MCP, approval admission | Host/Capabilities | Supply Project roots and receive approval requests. |
| Thread, Turn, Goal, ThreadItem, ordering, CAS/revision, wire protocol | App Server | Use the SDK; do not create another execution loop. |
| Session history and settled checkpoints | App Server/SessionStore | Read the canonical projection; do not write a Web copy. |
| Project list, names, source folders, UI preferences | Web Gateway | Persist only the derived Project/UI manifest. |
| Action grants | Host/Capabilities runtime memory | Broadcast pending requests and submit typed responses; never create or restore grants from UI state. |

The Web Gateway's `~/.mini-agent/web/state.json` is Project/UI metadata. The
canonical conversation data remains under
`~/.mini-agent/sessions/<encoded-primary-workspace>/<session-id>/`, including
the settled checkpoint, `session.jsonl`, Goal state, Plan state, and item
projection. A gateway restart may lose live process handles and in-memory
pending approval state, but it must not create or merge a second conversation history.

## Configuration ownership

For Web Studio, copy `mini-agent-web/.env.example` to the Web repository's
`.env`. The Python SDK discovers that file and passes the effective provider,
Goal, verifier, and App Server path settings to each child App Server process.
`%USERPROFILE%\.mini-agent\.env` is the shared CLI/Host user configuration and
fallback; it is not the Web Project registry or Session history directory.

The effective precedence is:

```text
explicit SDK env / process env
    > Web workspace .env or project workspace .env
    > ~/.mini-agent/.env
    > built-in defaults
```

Project bindings are injected by the Gateway for each child process and should
not be copied manually into the Web `.env`:

| Variable | Injected value |
| --- | --- |
| `MINI_AGENT_PROJECT_ID` | Web Project identity used by scoped approval matching. |
| `MINI_AGENT_EXTRA_READ_ROOTS` | `os.pathsep`-separated associated roots available as read-only references. |
| `MINI_AGENT_EXTRA_WRITE_ROOTS` | `os.pathsep`-separated associated roots admitted for edits. |
| `MINI_AGENT_SESSION_MODE` | `new` for a new Session, `resume` for a canonical resumable Session. |
| `MINI_AGENT_SESSION_ID` | Existing Session identity for `resume`. |
| `MINI_AGENT_THREAD_ID` | Thread identity assigned to the child process. |

The Gateway sets the child process working directory to the Project's primary
directory. Provider configuration therefore belongs to the Web `.env` or the
shared user `.env`; the Project's associated directories are passed separately
as explicit Host roots.

## Project and workspace binding

A Web Project has one primary directory and zero or more associated source
folders. The primary folder is the App Server working directory. For every
associated folder, set `editable: false` when it is reference-only; editable
folders are passed as extra write roots as well as read roots.

The relevant Gateway operations are:

| HTTP operation | Purpose |
| --- | --- |
| `GET /api/projects` | List the Project registry and current Project. |
| `POST /api/projects/new` | Create a Project with `name`, optional `path`, and `source_folders`. |
| `PATCH /api/projects/{project_id}` | Change Project metadata, access, policy, or source folders. |
| `POST /api/projects/switch` | Select the active Project by ID or path. |
| `DELETE /api/projects/{project_id}` | Remove a Project from the Gateway registry; it does not delete the directory. |
| `POST /api/projects/{project_id}/pin` | Change sidebar pin state. |

Each Project mutation that changes the active workspace causes the Gateway to
rebind the Host/App Server process. This clears pending approvals and the
process-local Host/Capabilities grant store. A workspace revision therefore
changes when the primary or associated root set changes, and old grants must
not be reused for the new binding.

`project` and `full_machine` are access scopes. `full_machine` expands the
candidate path range but is not allow-all: Deny, Plan-mode source-file mutation
locks, unavailable tools, and high-risk confirmation remain effective.
`interactive`, `automatic`, and `trusted` are execution policies; Automatic directly
admits only bounded read-only Shell inspection, while Trusted also admits ordinary
validated workspace actions and Shell commands. Trusted still requires approval for
destructive/system commands, MCP, and workspace-external actions such as `read_image`.
Plan mode does not add a
separate Shell restriction. `once`, `session`, and `project` are action-grant
scopes selected in an approval response and validated by Host/Capabilities.

## Project-qualified requests and stale-response protection

Every Gateway request that can address a Thread, Session, workflow, runtime, or
workspace includes the selected `project_id`. REST history, runtime, Goal,
settings, and SidePanel file requests carry it in the query or payload; a
WebSocket is opened with `?project_id=...`, and each `turn`, `steer`,
`interrupt`, approval response, and `ping` repeats the same project identity.
The Gateway filters project-scoped broadcasts before delivery, so two Projects
may safely contain a Thread with the same display ID such as `default`.

Web Studio assigns a request epoch to the selected Project/Session. Switching,
creating, forking, or closing a Session cancels old history, workflow, runtime,
and file requests and atomically clears the old message, Turn, Plan, Goal,
Runtime, and approval projections before loading canonical state for the new
selection. A late response from an older epoch is discarded rather than written
into the current Session view.

## Session history and switching

The Gateway combines three sources into the Web sidebar:

1. live Thread IDs from the current App Server clients;
2. Web UI metadata such as title, summary, and pin state; and
3. the read-only SessionStore catalog for historical, paused, and locked
   Sessions across registered Projects.

Use these endpoints:

| HTTP operation | Purpose |
| --- | --- |
| `GET /api/threads` | Enriched list of live and canonical historical Threads. |
| `GET /api/threads/project/{project_id}/sessions` | Bounded canonical Session list for one Project. |
| `GET /api/threads/{thread_id}` | Read canonical history without writing a Web checkpoint. |
| `GET /api/threads/{thread_id}/items` | Read the bounded ThreadItem projection. |
| `POST /api/threads` | Start a new Thread or attach a selected Thread. |
| `POST /api/threads/{thread_id}/attach` | Attach a historical or paused Session. |
| `POST /api/threads/fork` | Fork a Thread through the canonical App Server boundary. |
| `GET /api/threads/{thread_id}/children` | List child Sessions derived from a parent Thread. |
| `POST /api/threads/{thread_id}/children` | Start one bounded Turn in an independent child Session/runtime. |
| `POST /api/threads/{thread_id}/children/{child_thread_id}/cancel` | Request cooperative cancellation of an active child Turn. |
| `POST /api/threads/{thread_id}/children/{child_thread_id}/retry` | Start a bounded new attempt for a settled failed or cancelled child. |
| `GET /api/threads/{thread_id}/notebook` | Read the bounded Session-owned notebook projection. |
| `POST /api/threads/{thread_id}/notebook` | Write one bounded current-Session Notebook entry. |
| `GET /api/threads/{thread_id}/notebook/search?q=...` | Search bounded Notebook keys, keywords, content, and cached evidence metadata. |
| `DELETE /api/threads/{thread_id}/notebook` | Forget one current-Session Notebook entry. |
| `POST /api/threads/{thread_id}/close` | Close an active Thread and release resources. |
| `PATCH /api/threads/{thread_id}/summary` | Update Web display metadata only. |
| `PATCH /api/threads/{thread_id}/rename` | Update the Web display title only. |

Session catalog entries expose bounded `session_status`, `runtime_status`,
`turn_active`, `process_online`, `resumable`, `goal_status`,
`plan_review_pending`, `cleanup_pending`, `active_turn_id`, `checkpoint_seq`,
and the last-turn diagnostic fields. The important transitions are:

```text
new Thread
    → process_online=true, turn_active=true while an active Turn owns session.lock
    → process_online=true, turn_active=false while the process is idle/standby
    → paused when Goal is user-paused
    → process_online=false after the process releases the lock
    → resumable when a complete settled checkpoint exists
```

`turn_active` means that the latest Turn is unsettled and still owned by a live
process; `process_online` only means that the SessionStore process lock is live.
An idle online process is therefore “online/standby”, not a running Turn. After
a crash, an unsettled record can be `process_online=false` and
`turn_active=false` while retaining `last_turn_status=in_progress` for
diagnostics; a complete checkpoint makes it recoverable. A completed Plan Turn
sets the persisted `plan_review_pending` confirmation. It survives reload and
restore in Session-owned `plan_mode.json`, and selecting implementation clears
the pending state and returns the Thread to default mode.

Child task execution uses the same split. The Gateway first asks the parent
App Server for an exact Session fork; that control path reads the last complete
persisted checkpoint even when the parent Turn is active. It then starts the
child Turn through a separate App Server client/process. The parent’s in-flight
input, mutable Core context, tool calls, and approvals are not copied. Child
history and status remain addressable by the child Thread and are observed
through the existing event and canonical Session projections. This is
structural concurrency through independent runtimes, not a Core scheduler.
The global or parent project setting limits active children to `1..=8` (default
`2`). The Main Thread chooses `parallel` or `sequential` for each delegation
and can attach an operation group and sequence; queued operations remain
durable and are drained by the Gateway when a slot becomes available. The
Gateway does not infer scheduling mode from project settings.

The child projection is recoverable because `session.jsonl` is the authority for
the latest `operation` record. `queued`, `running`, `awaiting_approval`,
`completed`, `failed`, and `cancelled` remain distinguishable after a Gateway
restart. `cancel` sends a cooperative `turn/interrupt`; it does not delete the
child Session. `retry` starts a fresh child Turn with the same operation ID and
an incremented attempt. If no process is online, the projection reports that
recovery or re-attach is required instead of fabricating a completed result.

The Session notebook is read through the dedicated projection endpoint. The
Gateway does not cache it as a second authority: the App Server/SessionStore
owns the bounded entries, and resume injects only a summary into the runtime.
WebStudio's Memory tab edits the current Session and displays the parent snapshot
as read-only. A child cannot use the parent scope to write or forget.

Notebook writes may include bounded `keywords` and `evidence`. Commit evidence
is cached at write time (`commit`, `subject`, `authorAt`, `committedAt`, and
`recordedAtMs`); the search/read path does not invoke Git. Project or global
settings expose only `notebook.max_entries` and `notebook.max_entry_chars`.

If a live process owns the Session lock, `attach` returns a conflict or an
`attached: false` lock description. The Gateway must not delete the lock or
start a second writer. Once the lock is released, `attach` starts an App Server
with `MINI_AGENT_SESSION_MODE=resume` and the canonical Session ID. Reading
history does not itself attach or mutate a Session.

## Turn, Plan, Goal, and approval flow

For normal execution, Web Studio uses `/ws/agent`:

```json
{"action":"turn","project_id":"project-1","threadId":"thread-1","mode":"start","prompt":"inspect the workspace"}
{"action":"steer","project_id":"project-1","threadId":"thread-1","turnId":"turn-1","text":"focus on the failing test"}
{"action":"interrupt","project_id":"project-1","threadId":"thread-1","turnId":"turn-1"}
```

The Gateway's REST equivalents are `POST /api/agent/turn`,
`/api/agent/stream`, `/api/agent/steer`, and `/api/agent/interrupt`. The SDK
maps these operations to `turn/start`, `turn/read`, `turn/steer`,
`turn/interrupt`, and the ordered notification stream described in
[`app-server.md`](app-server.md).

### pstack plugin and Skill entry points

Every new WebStudio Project enables the bundled `pstack` group by default.
The control-panel Skill tab changes `builtin_skill_groups` for the selected
Project; changing it while a Turn or approval is active returns `409` and
does not mutate the runtime. A successful change restarts only that Project's
App Server and refreshes `GET /api/skills?project_id=...`.

The two composer entry points have different scopes:

| Input | Wire field | Scope |
| --- | --- | --- |
| `+ pstack 重构模块` or the plus-menu item | `workflow` | Activates the pstack Skill Group for this Turn. It contributes metadata only; the model selects and reads relevant Skills on demand. |
| `$pstack:architect 重构模块` | `selectedSkills: ["pstack:architect"]` | Explicitly loads that Skill body for this Turn before model execution. |
| `$pstack-plugin:how` / `$how` | canonicalized to `pstack:how` when valid | Codex-compatible alias and unqualified convenience form. |

The cleaned prompt, `selectedSkills`, and `workflow` are preserved in queued
messages. The Gateway and browser only send names; Host resolves the effective
catalog, trusted Skill paths, enabled group, eight-Skill limit, and 32 KiB
body limit.

The ordered event stream reports `skill_group_activated` for `+` and
`skills_loaded` for explicit or successfully observed on-demand Skill reads.
`skills_loaded` has `phase: "started"` before a recognized `SKILL.md` read and
`phase: "loaded"` after success; old events without `phase` are treated as
loaded. The event is deduplicated per Turn and contains only Skill names,
qualified names, source, and group. `skills_load_failed` contains only the
bounded error code. These events are replayable through
`GET /api/threads/{thread_id}/events`
while the App Server runtime retains its bounded replay window; no Skill body or
filesystem path is sent to WebStudio.

When a parent model emits the Host `delegate_task` tool, the Gateway observes
the real `tool_started` event and invokes the same child control seam internally.
The event is therefore an observation trigger, not a Gateway-owned task state
machine; the child Session operation records and runtime projection remain
authoritative.

The runtime catalog also includes direct user Skills from
`%USERPROFILE%/.mini-agent/skills` and `%USERPROFILE%/.agents/skills`, plus the
synchronized builtin pstack group. Normal Turns receive metadata for all
enabled entries and may read a matching `SKILL.md` on demand. References,
scripts, assets, and other files below that enabled Skill root remain ordinary
read-only `read_file` resources and do not create separate skill events.

Plan and Goal are Thread-owned App Server workflows:

| HTTP operation | App Server operation | Meaning |
| --- | --- | --- |
| `POST /api/threads/{thread_id}/settings` | `thread/settings/update` | Set `default` or `plan` and optional Builtin tools. |
| `GET/POST/DELETE /api/threads/{thread_id}/goal` | `thread/goal/get|set|clear` | Read, set, or clear the canonical Goal. |
| `POST /api/threads/{thread_id}/goal/pause` | `thread/goal/set` with `paused` | Pause continuation through GoalRuntime. |
| `POST /api/threads/{thread_id}/goal/resume` | `thread/goal/set` with `active` | Resume continuation through GoalRuntime. |

`/api/workflows/state` is a Gateway-only read-only aggregate convenience
projection. It is not an App Server authority and must not be used to submit
verdicts, advance milestones, or create a competing workflow state machine.
Settings and Goal action responses, together with `thread/settings/updated` and
`thread/goal/updated|cleared`, expose the canonical App Server `stateRevision`.
Clients should consume these projections monotonically per Thread and re-read
`/api/workflows/state` after a WebSocket reconnect before applying new events.

The live Compaction start/finish pair has one independent bounded `item_id`,
which is also retained by the local redacted trace. Web Studio may merge adjacent
Compaction entries into an expandable “上下文压缩 ×N” card while preserving each
entry's `turn_id` and `item_id` in the details.

Approval flow:

```text
Host/App Server → SDK approval callback
                 → Gateway broadcasts approval_request over WebSocket
                 → browser POST /api/approval/respond or sends approval_response
                 → SDK approval/respond
                 → App Server/Host continues or records denial
```

The Gateway also exposes `GET /api/approval/pending`,
`GET /api/world/approval`, and `POST /api/world/approval/revoke`. The Gateway
retains only pending/UI state; Host/Capabilities owns the process-local grant
store and matches a complete action key (class, normalized action, target
paths, access, workspace, and revision). Project changes, runtime restart,
policy changes, or explicit revocation clear it. `state.json` must never be
treated as an approval store.

The App Server also writes an independent, bounded `approval-evidence.jsonl`
sidecar beside a durable Thread Session when the Thread is bound to a Session
file. It records `approval_requested` and `approval_resolved` entries with
project/Thread/Turn/call identity, policy, access, a tool/command-prefix summary,
the `session_item_id` join key, the canonical action-key hash, and, for a
resolution, the final outcome/grant scope. It does not copy the full command,
raw prompts, complete tool arguments, tool output, or approval grants as reusable
authority. To inspect the bounded/redacted command, readers join `session_item_id`
to the `kind=item` record with the same `item_id` in the co-located `session.jsonl`.
Each Thread owns its file; project reports merge sidecars at read time. The
evidence is for reviewing risk rules and repeated approvals. It never changes
Host/Capabilities authorization or automatically widens an allow rule.

## Startup and verification

From `mini-agent-web`:

```powershell
Copy-Item .env.example .env
uv sync
uv run mini-agent-server
```

Open `http://127.0.0.1:8000`. If the App Server binary is not on `PATH`, set
`MINI_AGENT_APP_SERVER_PATH` in the Web `.env`. The Gateway serves the built
frontend when `frontend/dist` exists; during frontend development run
`npm install` and `npm run dev` in `frontend`.

The minimum local verification is:

```powershell
uv run ruff check .
uv run ruff format --check .
uv run pytest -q
npm --prefix frontend test
npm --prefix frontend run build
```

For an end-to-end check, verify the following sequence in one report:

1. create a Project with one primary and one associated editable/reference
   folder;
2. start a Thread and confirm `MINI_AGENT_*` root bindings in the App Server
   approval request/world state;
3. submit a turn and observe `turn/event`, Item lifecycle, and settlement;
4. trigger approval, resolve it with `once`, `session`, or `project`, reuse it
   only under the matching complete action key, then revoke it;
5. pause or stop the Session, list it from the canonical catalog, and attach it
   again without creating a second writer;
6. switch Project or roots and confirm old approvals are not reused.
7. confirm REST and WebSocket requests are project-qualified, stale responses
   cannot overwrite a switched Session, and the sidebar distinguishes
   `turn_active` from `process_online`;
8. restore a completed Plan turn and verify its review confirmation, then
   expand grouped Compaction details and check their `turn_id`/`item_id` values.

When a browser shows a blank page, first inspect the browser console and the
Gateway log, then check `/health`, `/docs`, and the WebSocket connection. A
missing frontend import/build error is a Web layer failure; an
`approval_request` with `pending` status means the App Server is alive and is
waiting for a typed approval response. Do not infer a model or Session failure
from the UI alone; inspect the Gateway/App Server logs and canonical Session
projection.

## Maintenance rule

Update this document when the cross-repository route, environment ownership,
Session attachment behavior, or WebSocket message contract changes. Update
[`app-server.md`](app-server.md) when the JSON-RPC protocol changes, and update
`mini-agent-web/server/README.md` or the SDK guide for repository-local API
details. New duplicated state is not an acceptable fix for a missing
projection or notification.
