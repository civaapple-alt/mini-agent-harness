# Edit capability surface

## Question

For a localized change to an existing file, should the harness expose exact
unique replacement or unrestricted whole-file rewrite?

## Fixed WHAT

- one deterministic model and prompt;
- identical four-line workspace fixtures;
- the same read capability, system prompt, three-step limit, and approvals;
- target verifier: `mode = slow` becomes `mode = fast`;
- collateral verifier: every other byte remains unchanged.

The model reads before editing. Under the rewrite surface it reproduces the
functional settings but loses two comments, representing a plausible lossy
full-file reconstruction.

## Treatments

1. `exact_unique_replacement` accepts path, old text, and new text, and rejects
   zero or multiple matches.
2. `whole_file_rewrite` accepts path and complete replacement content.

Both treatments live in the experiment test. The production host exposes the
winning existing-file behavior directly rather than adding a selectable edit
policy.

## Reproduce

```sh
cargo test -p mini-agent-cli workspace::edit_experiment -- --nocapture
```

## Result

| Treatment | Steps | Calls | Errors | Target | Collateral |
| --- | ---: | ---: | ---: | --- | --- |
| exact unique replacement | 3 | 2 | 0 | pass | pass |
| whole-file rewrite | 3 | 2 | 0 | pass | fail |

Both surfaces achieved the requested semantic change, so a target-only
verifier would miss the damage. Exact replacement provided a materially
stronger invariant at no extra model-step or tool-call cost in this fixture.

The resulting default is asymmetric: `edit_file` changes existing files and
`write_file` creates new files but refuses replacement. This does not prove
whole-file rewrite is never useful; it keeps that broader capability out of
the default harness until a task demonstrates a need for it.
