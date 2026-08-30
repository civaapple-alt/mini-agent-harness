# Runtime Authority and Action Ordering

Status: implemented

## Decision

The runtime keeps three different responsibilities separate:

| Responsibility | Authority | Current synchronization boundary |
| --- | --- | --- |
| Agent loop and live Thread state | Core Thread / Harness | One App Server worker owns each stored Thread exclusively |
| Runtime command admission and Thread lifecycle | App Server worker | One bounded Tokio mpsc queue, consumed by one worker task |
| Durable history, host policy, and workflow files | Host adapters and stores | Called by the App Server management/workflow services |

The CLI and JSON-RPC transport are clients of the App Server control plane. They
do not own a second mutable Thread or decide the order of runtime mutations.
The Host supplies capabilities, policy, persistence, and workflow storage; it
does not own the App Server command order.

The App Server worker now wraps every queued command in an internal
ActionEnvelope. The worker assigns an ActionId and an ActionSequence when it
admits the command, then includes an ActionReceipt in the internal reply. The
public facade currently projects the receipt away, so this change does not
alter the 0.4.0 external API.

The following counters remain independent:

- ActionId: identity of one admitted command.
- ActionSequence: total order assigned by the server worker at admission.
- EventEnvelope.sequence: order of Core output events within the Thread.
- RuntimeRevision: the future state-tree revision used for stale-write
  detection. Phase two records the initial base revision (0); mutation and
  compare-and-swap enforcement are intentionally deferred until runtime state
  is consolidated behind the same actor queue.

## Consequences

- FIFO ordering is now an explicit App Server concept rather than an accidental
  property of individual Command branches.
- Concurrent callers are ordered at worker admission, not by JSON-RPC request
  identifiers or client wall-clock timing.
- Core event ordering remains owned by Core and is not conflated with input
  action ordering.
- Runtime management, workflow, approval, and the thread_ids index still
  contain separate state or synchronization paths. They are the next
  consolidation targets; this change intentionally does not pretend they are
  already one atomic state tree.

## Verification

- ActionSequencer tests independent action identity and server-admission
  sequence allocation.
- Existing App Server tests continue to exercise lifecycle, updates, turn
  control, and multi-thread behavior through the public facade.
- No external protocol fields or historical compatibility paths are added.
