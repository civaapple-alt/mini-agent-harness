# App Server

`mini-agent-app-server` exposes the host backed `Thread` service to a
subprocess client. The current transport is newline delimited JSON over
stdin/stdout. Each input line is one JSON-RPC request; turn progress is emitted
on the same output stream as `turn/event` notifications.

The default binary owns one configured thread per process. Embedded callers can
construct a service with several preconfigured thread identities and address
them through the same methods. The service also exposes bounded thread
list/read/close, fork and resume, turn result reads, cooperative steering and
interruption, and approval request/response routing. External adapters should
use the same App Server boundary.

Run it after configuring the provider environment:

```sh
cargo run --release -p mini-agent-app-server --bin mini-agent-app-server
```

The first request must negotiate protocol version 1:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientName":"example","clientVersion":"0","capabilities":{},"profile":"interactive","providers":{"model":"openai","tools":"builtin","extensions":"builtin","policy":"builtin"}}}
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

Host construction is kept outside the service worker: callers use
`HostRuntimeFactory` with an explicit `RuntimeProfile`, then hand the resulting
runtime to the App Server or an edge adapter. The App Server does not discover
extensions or infer capabilities from transport method names.

`initialize.params.profile` is optional for the stdio server. The standalone
server reads the first initialize request before constructing its first Thread,
resolves the requested allowlisted profile plus the bounded workspace profile,
and freezes that selection for the service lifetime. Unavailable profile names
are rejected before Thread construction. For an already constructed embedded
runtime, the request must match the active profile. The initialize result includes the
selected `profile` and structured `capabilityManifest` with enabled,
disabled, extension depth, selected extension names, prompt, and rule source
metadata, precedence, typed rule policy, per-source rule status, resolver
state, bounded source fingerprints, conflicts, and context limits. The
manifest reports the selected bounded extension names and load depth.
For the regular `general` agent, this manifest describes the selected base
prompt fingerprint, independent prompt/rule source admission, and bounded
context limits. A typed base-prompt/output/context preset is a follow-up wire
addition; the current protocol never accepts arbitrary prompt or rule bodies.
The standalone server resolves its workspace profile before creating the
thread factory and reuses that frozen selection for new threads, keeping the
advertised manifest consistent for the lifetime of the process.
Set `MINI_AGENT_PROFILE` before starting the standalone binary to select one
of the three builtin profiles (`interactive`, `ask`, or `auto`); unknown names fail closed before a provider or
thread is created.

`initialize.params.providers` is an optional selector for the four local
provider categories (`model`, `tools`, `extensions`, and `policy`). The
standalone server applies these bounded IDs before constructing the first
Thread; an embedded runtime requires requested IDs to match the frozen
profile. Provider instances, credentials, commands, and paths never cross the
JSON-RPC boundary. The current registry exposes `openai` for models and
`builtin` for the other three categories.
