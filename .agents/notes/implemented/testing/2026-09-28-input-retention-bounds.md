# Input retention bounds evidence

## Decision

Reject oversized Turn input before emitting `TurnStarted`. Bound retained
steering and follow-up input by item and aggregate byte limits, and cap JSON-RPC
line reads before deserialization.

## Bounded scenarios

### Oversized Turn input

| Field | Evidence |
| --- | --- |
| Hypothesis | An oversized prompt is rejected without entering the event stream or replay cache. |
| Public path | `AppServer.turn_start_for`. |
| Setup | Deterministic model and a four-byte input limit. |
| Stimulus | Submit a five-byte prompt. |
| Trace | The result is `not_submitted`; no broadcast event or replay entry contains the prompt. |
| Settlement | No Turn starts and no prompt is retained by App Server replay. |
| Failure case | A `TurnStarted` event or retained replay entry indicates a boundary regression. |
| Command | `cargo test -p mini-agent-app-server rejects_oversized_input_before_broadcasting_or_caching_it` |
| Gap | Does not test every transport adapter that can call Core directly. |

### Pending input queue

| Field | Evidence |
| --- | --- |
| Hypothesis | An item above 64 KiB or aggregate retained input above 512 KiB is rejected before queue insertion. |
| Public path | Core `PendingInputQueue::submit`, used by `RunControl::submit`. |
| Setup | In-memory queue and deterministic text/metadata; no provider call. |
| Stimulus | Submit oversized text/metadata, then fill 16 items and exceed the aggregate byte limit by one byte. |
| Trace | Both rejected submissions return `ByteLimit`; queue length and contents remain bounded. |
| Settlement | The accepted queue retains at most the configured count and byte limits. |
| Failure case | Oversized values must not be retained until the Harness later consumes them. |
| Command | `cargo test -p mini-agent-core input::tests` |
| Gap | Does not measure allocator-specific fixed object overhead. |

### JSON-RPC line input

| Field | Evidence |
| --- | --- |
| Hypothesis | The JSON-RPC reader stops at 2 MiB plus one byte instead of allocating the full line. |
| Public path | JSON-Lines stdio transport. |
| Setup | Buffered in-memory input with no newline and no provider. |
| Stimulus | Supply a line larger than the 2 MiB limit. |
| Trace | The reader returns `InvalidData` after reading only the limit plus one byte. |
| Settlement | The oversized input stream is rejected before JSON deserialization. |
| Failure case | Reading the complete line would allocate unbounded request memory. |
| Command | `cargo test -p mini-agent-app-server rejects_json_rpc_lines_over_the_byte_limit` |
| Gap | Does not exercise OS stdio buffering behavior. |

## Verification

`cargo test -p mini-agent-core` passed (48 tests); `cargo test -p
mini-agent-app-server` passed (81 library tests and one binary test). Targeted
Clippy, fmt, and line budget passed.
