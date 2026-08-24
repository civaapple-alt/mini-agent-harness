# Recovery across an uncertain effect

## Question

If the process crashes after an external effect but before recording its
outcome, can the harness safely replay the call? Is a durable intent record by
itself sufficient?

## Fixed HOW failure

Every scenario crashes at the same boundary:

```text
durable intent -> external effect -> crash -> missing settlement
```

The external fixture supports a read and a non-idempotent increment. The
increment verifier requires exactly one applied write and a known completed
outcome.

## Treatments

1. `naive_replay` forgets the uncertain boundary and executes again.
2. `intent_guard` replays a read but quarantines an uncertain write.
3. `idempotent_replay` repeats a write using the same identity against an
   effect implementation that deduplicates identities.

## Reproduce

```sh
cargo test -p mini-agent-core --test effect_recovery_experiment -- --nocapture
```

## Result

| Treatment | Effect | Attempts | Applied writes | Completed | Verifier |
| --- | --- | ---: | ---: | --- | --- |
| naive replay | read | 2 | 0 | yes | pass |
| naive replay | increment | 2 | 2 | yes | fail |
| intent guard | read | 2 | 0 | yes | pass |
| intent guard | increment | 1 | 1 | no, uncertain | fail |
| idempotent replay | increment | 2 | 1 | yes | pass |

## Decision

Automatic replay is safe for this read and unsafe for the non-idempotent
write. A prepare/intent record prevents the harness from forgetting that the
outcome is uncertain, but cannot prove whether the external write happened. It
therefore improves safety without providing automatic completion.

mini-agent does not add generic persistence after this result. If durable
recovery becomes a product requirement, the smallest honest design must expose
an effect's recovery class: replayable, externally idempotent under a stable
call identity, or uncertain and requiring reconciliation. An intent journal
without that contract would give false confidence and remains outside core.
