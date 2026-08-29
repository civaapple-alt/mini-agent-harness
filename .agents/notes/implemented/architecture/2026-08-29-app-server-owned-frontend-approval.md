# App Server Owned Frontend Approval Contract

Status: implemented

## Decision

`mini-agent-app-server::frontend::ApprovalController`, `RuntimeProfile`, and
`observer::RunObserver` are now App Server owned wrappers around capability and
Host implementations. They expose only the frontend operations needed by CLI
startup and interaction (profile selection, bounded manifest projection,
approval construction, mode changes, output observation, and workflow path
markers).

`LocalRuntimeLaunch` keeps resolved Host launch state private and exposes
accessors for the bounded startup values. Its `start` method unwraps the
controller only at the App Server to Host composition boundary. The CLI does
not name or compile against the Capabilities approval controller or Host
profile implementation types.

Sandbox and security enum values remain frontend startup inputs, and the
observer wrapper remains a presentation adapter. They are intentionally still
available through the facade because they are CLI boundary configuration, not
session/world/MCP management state.

## Verification

```text
cargo check -p mini-agent-app-server -p mini-agent-cli --all-targets --locked PASS
cargo clippy -p mini-agent-app-server -p mini-agent-cli --all-targets --locked -- -D warnings PASS
cargo test -p mini-agent-cli --bin mini-agent args::tests --locked PASS (16 tests)
cargo test -p mini-agent-cli --all-targets --locked -- --test-threads=1 PASS (51 tests)
cargo test -p mini-agent-app-server --all-targets --locked -- --test-threads=1 PASS (25 tests)
```
