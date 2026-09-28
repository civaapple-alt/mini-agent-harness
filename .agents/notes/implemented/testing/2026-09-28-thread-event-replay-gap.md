# Thread event replay gap evidence

## Decision

The shared 512-event replay window records each Thread's latest emitted
sequence. If no events for a requested Thread remain, replay reports a gap
when that sequence is newer than the client's cursor.

## Bounded scenario

| Field | Evidence |
| --- | --- |
| Hypothesis | Evicting every event for one Thread does not make an outdated cursor appear complete. |
| Public path | App Server replay path used by JSON-RPC `turn/events`. |
| Setup | Two registered Threads; seed one event for the first, then fill the shared window with events from the second. |
| Stimulus | Read the first Thread from sequence zero after its event was evicted. |
| Trace | The page has no data and no oldest retained sequence, while `hasGap` is true. |
| Settlement | The caller can reconcile the Thread from canonical history. |
| Failure case | `hasGap: false` would claim a complete replay despite the missing event. |
| Command | `cargo test -p mini-agent-app-server reports_gap_after_the_global_replay_window_evicts_a_thread` |
| Gap | Does not exercise a Gateway or Web Studio reconnect reconciliation. |

## Verification

`cargo test -p mini-agent-app-server` passed (81 library tests and one binary
test); targeted Clippy, fmt, and line budget passed.
