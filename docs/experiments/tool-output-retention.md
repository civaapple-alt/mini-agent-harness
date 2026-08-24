# Tool-output retention

## Question

When tool output exceeds the context budget, should the harness retain only
the beginning or retain both the beginning and end?

## Fixed WHAT

- one deterministic build-log output containing a header, repeated noise, and
  a final verdict;
- one 96-byte output budget;
- one deterministic model that proceeds only when it sees
  `VERDICT=SAFE_TO_PROCEED`;
- two model steps: request the tool, then interpret its retained result.

## Treatments

1. `head_only` fills the budget from byte zero.
2. `head_and_tail` splits the remaining budget around an explicit truncation
   marker while respecting UTF-8 boundaries.

The comparison stays in a test module. The production harness directly uses
the winning retention rule rather than exposing a truncation-policy option.

## Reproduce

```sh
cargo test -p mini-codex-core tool_output_experiment -- --nocapture
```

## Result

| Treatment | Retained bytes | Header | Final verdict | Verifier |
| --- | ---: | --- | --- | --- |
| head only | 95 | visible | missing | fail |
| head and tail | 94 | visible | visible | pass |

For this log-shaped output, head-and-tail retained both orientation and the
decisive outcome within the same hard budget. It is therefore the default.
The `tool_finished` trace event also records `truncated` explicitly so an
experiment does not need to infer this state from display text.
