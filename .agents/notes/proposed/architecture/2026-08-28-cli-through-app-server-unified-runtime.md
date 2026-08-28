# CLI Through App Server: Unified Execution Base

Status: in-progress

## Implementation update (2026-08-28)

The first migration slice is complete:

- `mini-agent-app-server::AppServerRuntime` now composes the Host provider,
  tools, approval policy, images, and session persistence behind an initialized
  local App Server client.
- `mini-agent-cli ask`, one-shot `mini-agent auto`, and interactive REPL submit turns through the
  local client, consume ordered App Server events, read the settled result, and
  persist it through the runtime adapter.
- The provider-free `demo` path also starts a generic App Server and consumes
  its event stream instead of calling `Harness::run` directly.
- The REPL worker no longer owns a `Harness` or `Thread`; context updates,
  config changes, MCP tool extensions, `/new` identity reset, and session
  checkpoint reads use serialized App Server operations.
- The `mentor` command and Goal verifier now run their tool-free review turns
  through the App Server local client as well; Host retains provider/session
  helpers but no longer owns a second mentor turn loop.
- Existing CLI integration coverage passes for these paths, including shell
  execution, output routing, steer/follow, restart, Goal/Plan, MCP, and durable
  sessions.
- The App Server worker command loop is now isolated in `src/worker.rs`; the
  public service facade remains focused on the typed boundary and delegates
  command serialization to that module.
- The local host runtime reuses the protocol `TurnReadResult` directly instead
  of maintaining a second settled-result struct, reducing boundary duplication.
- Provider execution now has one wire adapter, `openai/responses.rs`, and one
  portable `ModelResponse`; the GLM-5.3-Flash Chat Completions adapter,
  model-name branch, and `OPENAI_CHAT_BASE_URL` configuration were removed.

The proposal remains open pending workspace line-budget cleanup and external
evidence (cross-platform CI and a real provider Goal run). The source tree now
has one CLI turn owner, and concrete provider implementations are isolated in
`mini-agent-capabilities` behind Host profile seams.

The runtime simplification follow-up now makes session JSONL the single durable
record and removes the external trace writer/replay surface. Result handles are
reloaded from `result_stored` records on resume, and CLI/App Server persistence
is always on. The line-budget report now treats `mini-agent-capabilities` as a
separately reported provider implementation group. The established runtime
gate remains `core + protocol + host + app-server`: it is currently
14,414/20,000 lines; capabilities is 13,863 lines and ACP remains excluded.
All Rust source is 36,888/30,000, so the workspace gate remains open and is
not hidden by the layer split.

Evidence from this slice:

```text
cargo check -p mini-agent-app-server -p mini-agent-cli       PASS
cargo test -p mini-agent-app-server                         20 passed
cargo test --workspace                                     PASS
cargo test -p mini-agent-cli --test interactive              32 passed
cargo test -p mini-agent-acp -p mini-agent-app-server-protocol 2 + 4 passed
cargo fmt --all --check                                  PASS
cargo clippy --workspace --all-targets -- -D warnings      PASS
cargo run -p mini-agent-cli -- demo "hello app server"     PASS (offline App Server path)
cargo run -p mini-agent-cli -- --help                     PASS
python -m unittest scripts/test_line_budget.py            PASS (3 tests)
cargo package --workspace --locked --no-verify --allow-dirty PASS (yanked chacha20 warning)
python scripts/line_budget.py                              FAIL (workspace 36888/30000)
```

Follow-up verification after the worker extraction (2026-08-28):

```text
cargo fmt --all                                          PASS
cargo check -p mini-agent-app-server --all-targets       PASS
cargo clippy --workspace --all-targets -- -D warnings    PASS
cargo test -p mini-agent-app-server                      20 passed
cargo test --workspace                                   PASS
cargo test -p mini-agent-cli --test interactive goal_mode PASS (5 passed)
cargo test -p mini-agent-cli --test interactive steer      PASS (1 passed)
cargo test -p mini-agent-app-server local_and_json_rpc_clients_preserve_the_same_event_trace PASS
cargo test -p mini-agent-acp maps_the_complete_acp_event_trace_from_the_app_server PASS
cargo +1.88.0 check --workspace --locked              PASS
cargo build --workspace --release                     PASS
cargo run -p mini-agent-cli -- demo "hello app server"     PASS
cargo run -p mini-agent-cli -- --help                     PASS
cargo package --workspace --locked --no-verify --allow-dirty PASS (yanked chacha20 warning)
python -m unittest scripts/test_package_release.py       PASS (4 tests)
python scripts/line_budget.py                            FAIL (workspace 36888/30000)
```

Verification boundaries:

- The passing test results are local Cargo results on the current Windows
  workspace. They do not substitute for macOS/Linux or remote CI runs.
- A Linux-target `cargo check` was attempted. It stopped in native `ring` /
  `aws-lc-sys` build scripts because this Windows host has no working
  `x86_64-linux-gnu-gcc` toolchain; this is not counted as Linux evidence.
- The deterministic test models cover the service lifecycle and CLI mappings;
  a paid or otherwise real provider Goal run has not been executed as part of
  this change.
- The workspace line-budget failure is a real remaining blocker. Capabilities
  are excluded from the established runtime gate because they are a separately
  reported provider group, not because their source lines are hidden; the full
  Rust total remains visible and still fails the 30,000-line ceiling.

## Context

At the proposal baseline, the workspace had two independent execution paths:

```text
mini-agent-cli  ->  mini-agent-host  ->  mini-agent-core

mini-agent-app-server  ->  mini-agent-host  ->  mini-agent-core
mini-agent-acp         ->  mini-agent-app-server
```

`mini-agent-cli` does not depend on `mini-agent-app-server`. Its REPL, `ask`,
and `auto` paths still construct `RuntimeBuilder`, `Harness`, and `Thread`
directly. The app-server separately implements its own command worker, event
projection, JSON-RPC dispatch, approval bridge, and thread lifecycle.

This makes the app-server an unused parallel execution base for the primary
binary. It also permits CLI and external clients to observe different turn,
event, approval, persistence, or error behavior. The additional service code
is not replacing the CLI path, so the workspace pays for two orchestration
paths.

## Proposal

Make `mini-agent-app-server` the single service boundary for all agent turns.
The CLI remains a frontend for input, command parsing, rendering, and exit
codes, but it must submit typed requests to an in-process app-server client.
The app-server owns service lifecycle and delegates execution to Host/Workflow
runtime objects and the Core/Protocol execution kernel.

The target dependency direction is:

```text
mini-agent-cli
    -> mini-agent-app-server (local client/service facade)
        -> mini-agent-host
            -> mini-agent-core + mini-agent-protocol

mini-agent-acp
    -> mini-agent-app-server (same service facade)
        -> mini-agent-host
```

The CLI should not directly construct a `Harness`, `Thread`, provider, tool
registry, approval controller, session store, or runtime builder for an agent
turn. It should consume the same typed result and event stream that a JSON-RPC
or ACP client consumes.

## Service shape

The app-server should expose one typed service API and use transports as thin
adapters:

```text
CLI local client  ─┐
JSON-RPC transport ├─> AppServerService ─> HostRuntime ─> Thread/Harness
ACP adapter       ─┘
```

`AppServerService` is the sole owner of:

- thread and turn identity;
- serialized commands and active-turn state;
- steer, follow-up, and interrupt semantics;
- ordered event delivery;
- settled turn and checkpoint reads;
- approval request/response correlation;
- host runtime construction and restart boundaries.

The local client and JSON-RPC transport must not each reimplement the service
workflow. They may only encode/decode requests, subscribe to events, and map
transport errors.

## CLI migration scope

The following user-facing modes will use the local app-server service:

- interactive REPL;
- `ask` and `run`;
- interactive `auto`;
- one-shot `auto`;
- `steer`, `follow`, cancellation, and queued input;
- Goal/Plan turns, session resume, and fork operations.

These commands remain CLI responsibilities because they do not execute agent
turns:

- help and version;
- status and doctor configuration inspection;
- session listing and diagnostics (no external trace replay command);
- provider-free demo may use a deterministic app-server backend, but must not
  call `Harness::run` directly from the CLI.

## Migration stages

### Stage 1: Freeze the service contract

- Define the minimum typed request/result/event API used by both local and
  JSON-RPC clients.
- Keep Core event and stop semantics authoritative; do not add CLI-specific
  events to Core.
- Decide which current lifecycle methods are required before migrating the
  frontend. Defer unused fork/resume/approval variants instead of expanding
  the contract by default.

### Stage 2: Add a host-backed local service constructor

- Move host runtime composition behind an app-server constructor or factory.
- Reuse `RuntimeBuilder`; do not duplicate provider/tool/session setup in the
  CLI or in a second app-server builder.
- Make configuration, security preset, sandbox, persistence, and model mode
  explicit service startup inputs.
- Preserve a deterministic backend for tests and `demo`.

### Stage 3: Migrate `ask` and one-shot `auto`

- Replace direct `RuntimeBuilder`/`Harness` construction in `ask.rs` and
  `main.rs` with the local client.
- Convert service events into the existing text and JSON output formats.
- Preserve exit-code, stdout/stderr, session checkpoint, and trace behavior.

### Stage 4: Migrate REPL and interactive `auto`

- Keep the CLI input thread and presentation loop.
- Replace the CLI worker's owned `Harness` with a local service client and
  event subscription.
- Route `/steer` as an immediate service request and `/follow` as a queued
  request; do not create a second queue in the CLI.
- Keep rendering and slash-command help in the CLI.

### Stage 5: Remove duplicate orchestration

- Remove direct CLI dependencies on Host modules that are only used to build
  or run an agent turn.
- Remove the CLI-owned Harness worker and duplicate turn result/error mapping.
- Collapse `LocalAppServerClient`, JSON-RPC dispatch, and service calls around
  one internal service interface.
- Update ACP to call the same service interface and verify event equivalence.

### Stage 6: Enable external process mode

- Keep the in-process client as the default for CLI startup and tests.
- Add an opt-in child-process mode that speaks the same JSON-RPC protocol to
  `mini-agent-app-server`.
- Ensure process mode and in-process mode produce the same event sequence,
  stop reason, approval behavior, and settled checkpoint.

### Stage status (2026-08-28)

- Stages 1–5 are implemented in the current worktree: the typed service
  boundary is used by every CLI agent turn, and Local/JSON-RPC/ACP event
  projections have deterministic fixtures.
- Stage 6 remains future work. The stdio transport exists, but the CLI does
  not yet expose a switch that launches and speaks to a child App Server
  process instead of using the in-process client.

## Acceptance criteria

1. `mini-agent-cli` no longer constructs or runs `Harness`/`Thread` for any
   agent turn.
2. The CLI uses the local app-server client for REPL, `ask`, `run`, and
   `auto`.
3. `mini-agent-app-server` and `mini-agent-acp` observe the same typed turn
   results, stop reasons, and ordered event sequence as the CLI.
4. `/steer` remains immediate and `/follow` remains queued after migration;
   the queue is owned by one service layer.
5. Persisted session and Goal/Plan behavior remains equivalent for completed,
   steered, cancelled, failed, and resumed turns.
6. JSON-RPC and ACP transports contain no duplicated execution workflow.
7. `cargo test --workspace`, CLI integration tests, app-server protocol tests,
   and ACP mapping tests pass.
8. The line-budget report shows a reduced total rather than counting a new
   parallel CLI orchestration path. The runtime and workspace gates are
   evaluated after duplicate code is removed.

## Non-goals

- Moving provider, filesystem, MCP, skills, persistence, or security policy
  into Core.
- Making ACP types part of Core or the base `mini-agent-protocol` crate.
- Replacing the CLI renderer with a JSON-RPC UI.
- Adding a remote network daemon before the in-process service path is stable.
- Preserving every speculative app-server lifecycle method before a client
  requires it.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| CLI output or exit behavior changes | Keep output formatting in CLI and add end-to-end snapshots/fixtures for text and JSON modes. |
| Event ordering differs between local and stdio transports | Assert one ordered event trace for both transports using a deterministic model. |
| Approval callback blocks the service worker | Keep approval correlation bounded and test disconnect, rejection, and retry paths. |
| Session/Goal logic is duplicated during migration | Move only turn execution ownership first; keep persistence/workflow calls behind Host service methods. |
| Generic model types leak into the CLI | Expose an app-server client/result abstraction or a concrete host-backed service constructor. |
| Migration adds another adapter layer | Delete the direct CLI worker in the same stage that enables the local client. |
| App-server scope remains too large | Start with turn start/steer/interrupt/event and add lifecycle methods only with a client and test. |

## Line-budget expectation

At the proposal baseline, the runtime included 2,893 app-server lines while
the CLI did not use that service. After this migration, the duplicate CLI turn
owner is gone and concrete provider implementations are isolated in the
separately reported `mini-agent-capabilities` group. The established runtime
gate is now 14,414/20,000 lines. The full workspace remains
36,888/30,000 lines, so the next cleanup must remove nonessential duplication
or explicitly retire optional code without restoring a second CLI orchestration
path. If the migration leaves both paths in place, the proposal has failed even
if all tests pass.

## Acceptance status

| Criterion | Current result |
|---|---|
| CLI turn ownership | Met: `ask`, `run`, one-shot `auto`, interactive REPL, `demo`, `mentor`, and Goal verifier turns use an App Server client; no direct `Harness::run`/`Thread` execution remains in CLI or Host production paths. |
| Shared steer/follow service control | Met locally: the REPL shares `RunControl` with the App Server worker, and interactive coverage passes. |
| Session, Goal/Plan, MCP, restart behavior | Met by current local CLI integration coverage. |
| App Server and ACP transport mapping | Met locally: Local-vs-JSON-RPC and ACP-vs-App-Server complete event trace fixtures pass; settled result checks also pass through the existing protocol/CLI tests. |
| Workspace and lint checks | Met locally: workspace tests and Clippy pass. |
| Runtime/workspace line budget | Runtime met: `14414/20000`; workspace open: `36888/30000` all Rust source. |
| macOS/Linux/CI evidence | Open; not available from this local run. |
| Real provider Goal behavior | Open; requires provider credentials and an explicitly authorized run. |
