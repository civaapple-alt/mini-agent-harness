# External Harness and ACP Boundary

Status: implemented

## Decision (2026-08-28)

The external harness boundary is implemented in four conceptual layers:

- `mini-agent-host` resolves profiles and composes runtime/workflow state
  through `RuntimeBuilder`; concrete provider, tool, MCP, skills, policy,
  workspace, session, and Result Store implementations live in
  `mini-agent-capabilities`.
- `mini-agent-cli` consumes that host as a frontend and keeps REPL, headless,
  and output concerns at the edge.
- `mini-agent-app-server-protocol` defines versioned JSON-RPC DTOs for
  initialization, thread start, turn start/steer/interrupt, and ordered event
  notifications.
- `mini-agent-app-server::serve_stdio` exposes the service as newline-delimited
  JSON over stdin/stdout, and the `mini-agent-app-server` binary composes a
  default host runtime for subprocess clients.
- The app-server now retains settled turn records, exposes checkpoint/read and
  thread/list/close methods, and can route turns across preconfigured Thread
  identities.
- `ApprovalBroker` bridges synchronous host approval callbacks to
  `approval/request` notifications and `approval/respond` replies on the
  service connection.
- `mini-agent-acp` provides an experimental edge mapping for ACP-style
  `session/new`, `session/prompt`, `session/cancel`, and `session/update`.

The app-server supports preconfigured and factory-created Thread identities,
settled checkpoint/result inspection, cooperative steering/interruption, and
approval request/response over its JSON-RPC boundary. The ACP adapter is an
explicit experimental mapping; it does not claim full ACP conformance or
implement ACP-only filesystem, terminal, batching, or authentication surfaces.

## Verification evidence

Initial verification on 2026-08-28 in the workspace passed:

- `cargo test --workspace --quiet` — all workspace tests passed, including the
  core, host, protocol, app-server, ACP, and CLI integration tests.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `cargo +1.88.0 check --workspace --locked` — passed on Windows.
- `cargo build --workspace --release` — passed on Windows.
- `cargo package --workspace --locked --no-verify --allow-dirty` — passed on
  Windows.
- `python -m unittest scripts/test_package_release.py` — passed (4 tests).
- `git diff --check` — passed; only Git's LF/CRLF normalization warnings were
  reported on Windows.

Using temporary Zig compiler/linker/archive wrappers on Windows,
`cargo check --workspace --target x86_64-unknown-linux-gnu` also passed. This
is a Linux-target compile check, not a native Linux runtime or CI result.

The source-line report separates the provider implementation group from the
runtime-layer gate. The current breakdown is core 4,058, protocol 699,
capabilities 13,863, host 4,756, app-server 4,871, acp 922, and CLI 7,689
lines. The runtime gate (`core + protocol + host + app-server`) is
14,384/20,000; capabilities and ACP remain separately visible and excluded
from that gate. All Rust source is 36,858/30,000, so the repository-wide line
budget remains an intentional follow-up gate. Host no longer re-exports
capability providers; frontends import them from the dedicated capabilities
crate. This proposal does not claim that the repository-wide line budget has
passed; the next cleanup stage must remove duplication or split the budgeted
change before release.

Follow-up verification on 2026-08-29 at `e317b14` passed
`cargo test --workspace --all-targets -- --test-threads=1` and
`cargo clippy --workspace --all-targets --locked -- -D warnings` on Windows.
The same revision passed `cargo +1.88.0 check --workspace --all-targets
--locked` and `cargo build --workspace --release --locked`.
The worktree is clean. Native macOS/Linux runtime, candidate CI, and a real
provider Goal run remain unverified.

These are local Windows results plus a Linux-target compile check. Native
macOS/Linux runtime, candidate CI, and a real provider run remain release
verification work; deterministic protocol tests cover the local multi-process
framing contract.

The latest remote baseline is also recorded separately from this committed
working tree: [CI #56 for `4934ac9`](https://github.com/civaapple-alt/mini-agent-harness/actions/runs/33144991077)
passed Minimum Rust 1.88 and the Ubuntu, macOS, and Windows test/build/smoke
jobs. Its quality job failed only at the existing line-budget gate
(`30,828/30,000`); formatting and linting passed. This is evidence for the
baseline revision, not for the current architecture revision.

## Boundary addressed

The project exposes the Harness to CLI, embedded, subprocess, and experimental
ACP clients without moving transport concerns into `mini-agent-core` or making
the CLI the runtime composition root.

## Layering

Keep four conceptual layers:

```text
Layer 4: mini-agent-cli
  CLI frontend, REPL input, headless output, local client

Layer 3: mini-agent-app-server
  Agent service, JSON-RPC dispatch, Thread/Turn API, notifications,
  approvals, connection and session lifecycle

Layer 2: mini-agent-host + workflows
  Provider, concrete tools, MCP, skills, sandbox, security, workspace,
  world, persistence, Goal, Plan, Mentor, and Persona

Layer 1: mini-agent-core + mini-agent-protocol
  Execution kernel and transport-neutral model/tool/turn/event contracts
```

The same architecture can be read as the runtime call direction:

```text
CLI client
    ↓
App Server service boundary
    ↓
Host / Workflows application host
    ↓
Core / Protocol execution foundation
```

The arrows describe dependency and request flow, not ownership of every
implementation detail. The CLI submits user intent and renders responses. The
App Server translates client requests into Thread/Turn operations and projects
events back to the client. Host and Workflows provide concrete models, tools,
policies, persistence, and product workflows. Core and Protocol remain the
authority for execution semantics and shared contracts.

For local one-shot execution, the CLI may use an in-process App Server or a
direct Host-to-Core path for efficiency. The latter is an implementation
shortcut; it must preserve the same Thread/Turn and event semantics that an
external client observes through the App Server boundary.

The app-server is above the host layer in the runtime composition: it exposes
host-backed capabilities through a stable client boundary and delegates actual
execution to `Thread`/`Harness` in core.

```text
CLI / VS Code / ACP client
          │
          │ JSON-RPC or local service calls
          ▼
mini-agent-app-server
          ├─ Thread/Turn service
          ├─ notification projection
          ├─ approval interaction
          └─ HostRuntime
               ├─ provider and tools
               └─ workflows
                    ▼
              mini-agent-core
                    ▼
              mini-agent-protocol
```

This is a conceptual layer change, not a requirement to create a deep crate
tree. `host` and `workflows` remain one application-host layer, while the
transport and server contracts may be split into crates only when they become
stable external surfaces.

## Protocol boundary

The existing `mini-agent-protocol` remains the kernel contract. It must not
gain JSON-RPC request envelopes, socket details, provider credentials, or ACP
specific types.

The separate `mini-agent-app-server-protocol` crate defines:

- `initialize` and capability negotiation;
- `thread/start`, `thread/resume`, `thread/fork`, `thread/read`;
- `turn/start`, `turn/steer`, `turn/interrupt`;
- request/response correlation and idempotency keys;
- approval and user-input requests from server to client;
- typed server notifications for turn, item, tool, and error progress;
- protocol version and experimental capability markers.

The app-server protocol projects core `EventEnvelope` values into client
notifications, but the wire schema is not the definition of the core event
model. `mini-agent-acp` maps ACP messages to the app-server service rather than
making ACP a dependency of core.

## App-server responsibilities

`mini-agent-app-server` implements these responsibilities:

1. Own connection initialization and capability negotiation.
2. Resolve and manage multiple Thread identities.
3. Start, resume, fork, inspect, and close threads.
4. Start, steer, interrupt, and observe turns.
5. Project ordered core events into client notifications.
6. Route approval requests and other server-to-client interactions.
7. Apply request-scoped and thread-scoped settings through HostRuntime.
8. Return settled turn state and usage after event streaming completes.

It must not implement another model loop, duplicate `Context`, or reimplement
tool execution. The core Thread remains the authority for turn semantics.

The mpsc/broadcast worker is the local service backend, and JSONL framing calls
the same service without changing core behavior.

## Host and workflow boundary

`HostRuntime`/`RuntimeBuilder` is reusable outside the CLI and assembles:

- provider model instances and image projection;
- workspace, shell, web, process, and subagent tools;
- MCP and skills/marketplace extensions;
- approval, security policy, and sandbox implementations;
- workspace/world context;
- session and result persistence;
- Goal/Plan/Mentor/Persona services.

The builder may keep local secrets and filesystem paths. Those values must be
translated into safe thread/turn settings before crossing the app-server wire.
Provider API keys, approval callbacks, and local process handles are never
serialized as protocol fields.

Goal and Plan should consume Thread events and settled checkpoints. They may
drive follow-up turns through the app-server service, but must not depend on
the CLI REPL worker.

## Configuration reservation

Keep configuration in three explicit categories:

```text
HostConfig
  provider credentials, local paths, MCP commands, sandbox implementation

ServerConfig
  transport, listen address, connection limits, notification backpressure

Thread/Turn settings
  model, cwd, approval profile, sandbox profile, skills, goal/workflow mode
```

Only the third category is eligible for client-request overrides. Server and
host defaults remain authoritative, and every override is validated before a
Thread or Turn is started.

The configuration types in core remain limited to execution semantics such as
step, context, response, and tool-output limits. Core must not learn about
listen addresses, API keys, MCP process commands, or ACP capability names.

## Transport and ACP reservation

Transport remains an edge concern:

```text
mini-agent-app-server-protocol  ← wire DTOs/schema
mini-agent-app-server-transport ← stdio JSONL / WebSocket / Unix socket
mini-agent-app-server            ← service and dispatch
```

The implemented transport is newline-delimited JSON over stdio, which is useful
for local clients and subprocess integration. WebSocket or Unix socket support
can be added without changing HostRuntime or core.

ACP support is an adapter at the same edge:

```text
ACP client ↔ mini-agent-acp adapter ↔ AppServer service
```

This supports the implemented baseline mapping without committing the execution
kernel to either wire protocol.

## Migration stages

1. **Complete** — Document the in-process adapter as a local service
   backend.
2. **Complete** — Extract `HostRuntime` and provider/tool assembly from
   `mini-agent-cli`.
3. **Complete** — Define a Rust service interface shared by local and
   transport-backed calls (`AppServerConnection` and `LocalAppServerClient`).
4. **Complete** — Add versioned app-server request/response/notification types.
5. **Complete** — Add stdio JSONL transport and a local service client. The CLI
   retains a direct path as an efficiency option while the service exposes
   equivalent Thread/Turn semantics.
6. **Complete for the current host contract** — Thread list/read/start,
   factory-backed resume/fork, close, settled turn inspection, and approval
   request/response are implemented. Durable cross-process session storage is
   still owned by the host and is not serialized by the service itself.
7. **Experimental** — `mini-agent-acp` maps the baseline session methods at
   the edge. Full ACP surfaces and conformance remain outside this release.

## Non-goals

- Do not move JSON-RPC or ACP types into `mini-agent-core`.
- Do not make `mini-agent-cli` the dependency of app-server or ACP clients.
- Do not expose provider secrets or host process handles over the wire.
- Do not require every one-shot local invocation to spawn a child process.
- Do not claim ACP compatibility before a protocol mapping and conformance
  tests exist.

## Verification criteria

- CLI, an in-process client, and a future JSON-RPC client can drive the same
  Thread/Turn semantics.
- Core tests remain independent of network transports and host credentials.
- Event identity and ordering remain stable across local and wire projections.
- Approval, steering, interruption, and settled checkpoint behavior are
  observable through the external service boundary.
- Adding an ACP adapter does not change `Harness`, `Thread`, or `RunControl`.

The criteria are covered by the workspace integration tests and the ACP bridge
test. Full ACP conformance, authentication, batching, and platform CI remain
release scope rather than claims of this architecture decision.
