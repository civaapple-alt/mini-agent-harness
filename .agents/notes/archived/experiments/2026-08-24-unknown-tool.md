# Unknown-Tool Recovery Experiment

Status: implemented
Archived: 2026-08-24

## Question

When the model names a tool that the harness did not expose, should the harness end the run or return the explicit error to the model?

## Fixed WHAT

- one deterministic model;
- one user prompt;
- an empty tool registry;
- the same system prompt and two-step budget;
- completion evidence: the exact text `RECOVERED_FROM_TOOL_ERROR`.

The model first requests the nonexistent `workspace_search` tool. If it sees a tool error in context, it emits the completion evidence on its next step.

## Treatments

1. `stop_on_unknown_tool` ends after the failed call.
2. `project_error_to_model` appends the failed tool result and lets the normal loop continue.

The treatment implementations remain in the test rather than becoming a runtime policy interface.

## Reproduce

```sh
cargo test -p mini-agent-core --test unknown_tool_experiment -- --nocapture
```

## Result

| Treatment | Model steps | Tool errors | Completed | Verifier |
| --- | ---: | ---: | --- | --- |
| stop on unknown tool | 1 | 1 | no | fail |
| project error to model | 2 | 1 | yes | pass |

For this recovery-capable model, projecting the explicit error is the smallest treatment that allows task completion. This result supports the current default; it does not claim that every model or every tool failure is recoverable.
