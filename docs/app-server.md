# App Server

`mini-agent-app-server` exposes the host backed `Thread` service to a
subprocess client. The current transport is newline delimited JSON over
stdin/stdout. Each input line is one JSON-RPC request; turn progress is emitted
on the same output stream as `turn/event` notifications.

The default binary owns one configured thread per process. Embedded callers can
construct a service with several preconfigured thread identities and address
them through the same methods. The service also exposes bounded thread
list/read/close, fork and resume, turn result reads, cooperative steering and
interruption, and approval request/response routing. ACP is kept in the
separate experimental `mini-agent-acp` edge adapter.

Run it after configuring the provider environment:

```sh
cargo run --release -p mini-agent-app-server --bin mini-agent-app-server
```

The first request must negotiate protocol version 1:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientName":"example","clientVersion":"0","capabilities":{}}}
```

Then start the configured thread and submit a turn:

```json
{"jsonrpc":"2.0","id":2,"method":"thread/start","params":{}}
{"jsonrpc":"2.0","id":3,"method":"turn/start","params":{"threadId":"default","input":{"mode":"start","text":"inspect the workspace"}}}
```

The service returns a correlated response for each request and ordered
`turn/event` notifications containing the core event type, thread/turn
identity, and sequence number. `turn/steer` validates the supplied active
`turnId`; `turn/interrupt` requests cooperative cancellation; `turn/read`
returns the settled result and messages. When the host runtime is wired with
an `ApprovalBroker`, sensitive tool calls emit an `approval/request`
notification and continue after the client replies with `approval/respond`.

The Rust `LocalAppServerClient` uses the same DTOs and dispatch without stdio,
which lets an embedded frontend migrate to the service boundary before it
spawns a child process.
