# File Boundaries and Runtime Start Options

Status: implemented

## Decision

The 0.4.0 runtime keeps the existing crate graph and makes the large modules
internally modular. File boundaries now follow the responsibility that the
parent module coordinates:

```text
mini-agent-app-server/src/json_rpc.rs
  json_rpc/{dispatch in parent, thread, turn, workflow, world, transport}
  json_rpc_tests.rs

mini-agent-capabilities/src/workspace.rs
  workspace/{approval, files, shell}
  workspace_tests.rs

mini-agent-capabilities/src/skills.rs
  skills/{discovery, plugins, mcp_config}

mini-agent-capabilities/src/session.rs
  session/storage.rs

mini-agent-cli/src/repl_worker.rs
  repl_worker/prompt.rs

mini-agent-host/src/goal.rs
  goal_tests.rs
```

The parent files keep the public facade, shared state, and orchestration. The
child modules own one cohesive implementation area and use `pub(super)` for
internal seams. No new crate or wire protocol was introduced, and the test
modules remain attached to their original parent modules.

Runtime construction has one explicit input object:
`RuntimeStartOptions`. `AppServerRuntime::start` uses the built-in OpenAI
factory, while `start_with_model_factory` keeps only the intentionally generic
external-provider seam. The previous multi-argument compatibility startup
forms were removed.

## Consequences

- `json_rpc.rs`, `workspace.rs`, `skills.rs`, `session.rs`, `repl_worker.rs`,
  and `goal.rs` are focused coordinator files rather than mixed implementation
  and test containers.
- The App Server runtime startup boundary is self-documenting at each call
  site: policy, harness, session, control, profile, and registry travel in one
  named value.
- Internal module extraction is behavior-preserving; local and JSON-RPC paths
  continue to use the same App Server control plane.
- CLI `run` and worker-launch argument cleanup remains a separate frontend
  concern; it is not part of the host-backed `AppServerRuntime` startup API.

## Measured result

```text
all Rust source: 27,200 / 30,000 lines
runtime:         13,993 / 20,000 lines

json_rpc.rs: 487
workspace.rs: 336
skills.rs: 421
session.rs: 615
repl_worker.rs: 722
goal.rs: about 500 production lines (tests are in goal_tests.rs)
```

## Verification

```text
cargo fmt --all PASS
cargo test -p mini-agent-capabilities PASS (62 tests)
cargo test -p mini-agent-host PASS (40 tests)
cargo test -p mini-agent-app-server PASS (20 tests)
cargo test -p mini-agent-cli PASS (14 unit + 11 integration tests)
cargo clippy -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server -p mini-agent-cli --all-targets -- -D warnings PASS
python scripts/line_budget.py PASS
```
