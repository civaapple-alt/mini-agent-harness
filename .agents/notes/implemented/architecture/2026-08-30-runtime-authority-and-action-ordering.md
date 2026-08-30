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
- RuntimeRevision: the monotonic state-tree revision used for stale-write
  detection. Runtime mutations carry the revision they observed and the actor
  rejects the mutation when that token is no longer current.

## Consequences

- FIFO ordering is now an explicit App Server concept rather than an accidental
  property of individual Command branches.
- Concurrent callers are ordered at worker admission, not by JSON-RPC request
  identifiers or client wall-clock timing.
- Core event ordering remains owned by Core and is not conflated with input
  action ordering.
- Runtime management and workflow state now share `RuntimeActorState` and one
  revision. Successful mutations advance the revision before their replies are
  delivered; direct Thread lifecycle mutations advance the same revision.
- A context update now applies to the live Thread and its Session checkpoint
  within one actor action. A persistence failure restores the Thread
  checkpoint, and a failed Session append truncates its own partial write.
- `thread_ids` remains a worker-maintained lifecycle index, but successful
  lifecycle changes participate in the same revision stream.

## Verification

- ActionSequencer tests independent action identity and server-admission
  sequence allocation.
- Existing App Server tests continue to exercise lifecycle, updates, turn
  control, and multi-thread behavior through the public facade.
- The App Server CAS test verifies that concurrent stale mutation tokens allow
  exactly one mutation to commit.
- Session restart coverage verifies that a newly created session has a durable
  empty checkpoint before the first context update.
- No external protocol fields or historical compatibility paths are added.
