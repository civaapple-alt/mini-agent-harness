# Thread-qualified Turn identity

## Decision

Core-generated Turn IDs include the owning Thread ID and the Thread-local
sequence. App Server retains a single turn/read lookup, so IDs must be unique
across Threads. Clients treat returned IDs as opaque values. A recovered
checkpoint keeps its stored Turn ID.

## Bounded scenario

| Field | Evidence |
| --- | --- |
| Hypothesis | Completing a Turn in another Thread does not replace the first Turn's `turn/read` result. |
| Public path | `AppServer.turn_start_for` and `AppServer.turn_read`. |
| Setup | Two preconfigured Threads (`thread-1`, `thread-2`) use deterministic `DoneModel`; no provider call. |
| Stimulus | Start and settle one Turn in each Thread, then read both results. |
| Trace | `thread-1` returns `turn-thread-1-1`; `thread-2` returns `turn-thread-2-1`; both reads return their own ID. |
| Settlement | Both retained results remain readable after the second Turn settles. |
| Failure case | Reused per-Thread ID would fail the distinct-ID assertion or return the wrong retained result. |
| Command | `cargo test -p mini-agent-app-server routes_multiple_preconfigured_threads_by_identity` |
| Gap | Does not prove uniqueness across separate App Server processes; recovered legacy checkpoint IDs retain their stored values. |

## Verification

`cargo test -p mini-agent-app-server` passed (77 library tests and one binary
test); `cargo test -p mini-agent-core` passed (46 tests); targeted Clippy with
`-D warnings`, `cargo fmt --all`, and the line budget passed.
