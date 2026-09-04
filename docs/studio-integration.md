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
    │ Thread / Turn / Goal / Item / approval control
    ▼
Host → Capabilities → Core
```

| Concern | Authority | Gateway responsibility |
| --- | --- | --- |
| Model steps, tool loop, limits, stop classification, Core events | Core | Relay the bounded projection. |
| Workspace tools, process, sandbox, MCP, approval admission | Host/Capabilities | Supply Project roots and receive approval requests. |
| Thread, Turn, Goal, ThreadItem, ordering, CAS/revision, wire protocol | App Server | Use the SDK; do not create another execution loop. |
| Session history and settled checkpoints | App Server/SessionStore | Read the canonical projection; do not write a Web copy. |
| Project list, names, source folders, UI preferences | Web Gateway | Persist only the derived Project/UI manifest. |
| Current-project approval grants | Web/App Server runtime memory | Broadcast requests and submit typed responses; never restore grants from UI state. |

The Web Gateway's `~/.mini-agent/web/state.json` is Project/UI metadata. The
canonical conversation data remains under
`~/.mini-agent/sessions/<encoded-primary-workspace>/<session-id>/`, including
the settled checkpoint, `session.jsonl`, Goal state, Plan state, and item
projection. A gateway restart may lose live process handles and in-memory
approval grants, but it must not create or merge a second conversation history.

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
| `PATCH /api/projects/{project_id}` | Change Project metadata, access, approval, or source folders. |
| `POST /api/projects/switch` | Select the active Project by ID or path. |
| `DELETE /api/projects/{project_id}` | Remove a Project from the Gateway registry; it does not delete the directory. |
| `POST /api/projects/{project_id}/pin` | Change sidebar pin state. |

Each Project mutation that changes the active workspace causes the Gateway to
rebind the Host/App Server process. This clears pending approvals and the
process-local current-project grant cache. A workspace revision therefore
changes when the primary or associated root set changes, and old approvals
must not be reused for the new binding.

`project` and `full_machine` are access scopes. `full_machine` expands the
candidate path range but is not allow-all: Deny, Plan locks, unavailable tools,
and high-risk confirmation remain effective. `per_action`, `current_session`,
and `current_project` are approval reuse lifetimes, not access scopes.

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
| `POST /api/threads/{thread_id}/close` | Close an active Thread and release resources. |
| `PATCH /api/threads/{thread_id}/summary` | Update Web display metadata only. |
| `PATCH /api/threads/{thread_id}/rename` | Update the Web display title only. |

Session catalog entries expose bounded `session_status`, `runtime_status`,
`resumable`, `goal_status`, `cleanup_pending`, `active_turn_id`, and
`checkpoint_seq` fields. The important transitions are:

```text
new Thread
    → locked/running while its App Server owns session.lock
    → paused when Goal is user-paused
    → historical after the process releases the lock
    → resumable when a complete settled checkpoint exists
```

If a live process owns the Session lock, `attach` returns a conflict or an
`attached: false` lock description. The Gateway must not delete the lock or
start a second writer. Once the lock is released, `attach` starts an App Server
with `MINI_AGENT_SESSION_MODE=resume` and the canonical Session ID. Reading
history does not itself attach or mutate a Session.

## Turn, Plan, Goal, and approval flow

For normal execution, Web Studio uses `/ws/agent`:

```json
{"action":"turn","threadId":"thread-1","mode":"start","prompt":"inspect the workspace"}
{"action":"steer","threadId":"thread-1","turnId":"turn-1","text":"focus on the failing test"}
{"action":"interrupt","threadId":"thread-1","turnId":"turn-1"}
```

The Gateway's REST equivalents are `POST /api/agent/turn`,
`/api/agent/stream`, `/api/agent/steer`, and `/api/agent/interrupt`. The SDK
maps these operations to `turn/start`, `turn/read`, `turn/steer`,
`turn/interrupt`, and the ordered notification stream described in
[`app-server.md`](app-server.md).

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

Approval flow:

```text
Host/App Server → SDK approval callback
                 → Gateway broadcasts approval_request over WebSocket
                 → browser POST /api/approval/respond or sends approval_response
                 → SDK approval/respond
                 → App Server/Host continues or records denial
```

The Gateway also exposes `GET /api/approval/pending`,
`GET /api/world/approval`, and `POST /api/world/approval/revoke`. A
`current_project` grant is retained only in process memory and is keyed by
Project, access, workspace identity/revision, path scope, and action summary.
Project changes, runtime restart, policy changes, or explicit revocation clear
it. `state.json` must never be treated as an approval store.

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
4. trigger approval, resolve it once, reuse it only under
   `current_project`, then revoke it;
5. pause or stop the Session, list it from the canonical catalog, and attach it
   again without creating a second writer;
6. switch Project or roots and confirm old approvals are not reused.

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
