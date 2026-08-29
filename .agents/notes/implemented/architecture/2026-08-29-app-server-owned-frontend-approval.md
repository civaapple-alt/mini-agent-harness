# App Server Owned Frontend Approval Contract

Status: implemented

## Decision

`mini-agent-app-server::frontend::ApprovalController` is now an App Server
owned wrapper around the capability approval implementation. It exposes only
the frontend operations needed by CLI startup and interaction (`new`, preset
or callback construction, mode changes, and workflow path markers).

`LocalRuntimeLaunch::start` unwraps the controller only at the App Server to
Host composition boundary. The CLI does not name or compile against the
Capabilities approval controller type.

Sandbox and security enum values remain frontend startup inputs, and observer
formatting remains a presentation adapter. They are intentionally still
available through the facade because they are CLI boundary configuration, not
session/world/MCP management state.

## Verification

```text
cargo check -p mini-agent-app-server -p mini-agent-cli --all-targets --locked PASS
cargo test -p mini-agent-cli --bin mini-agent args::tests --locked PASS (16 tests)
```
