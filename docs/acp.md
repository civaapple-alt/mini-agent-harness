# ACP Boundary

`mini-agent-acp` is an experimental edge adapter for the
[Agent Client Protocol](https://github.com/agentclientprotocol/agent-client-protocol).
ACP also uses JSON-RPC, but its baseline session vocabulary (`session/new`,
`session/prompt`, `session/cancel`, and `session/update`) differs from the
mini-agent app-server methods.

The adapter translates those session requests to the existing app-server
connection. Core remains unaware of ACP. The current mapping supports text
prompt blocks, one in-memory session per bridge, cancellation, and progress
updates carrying the underlying ordered core event. Session resume/fork,
permission requests, filesystem/terminal capability calls, batching, and
conformance certification are not implemented and are not advertised as
supported capabilities.

Use `AcpBridge` when embedding the service and keep the existing stdio JSON-RPC
transport for app-server clients. A dedicated ACP transport can be added at
the edge after session lifecycle and approval semantics are stable.
