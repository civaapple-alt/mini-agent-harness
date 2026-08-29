# Mainline Simplification: 29k to 26k Rust Lines

Status: implemented

## Context

The first 0.4.0 simplification stage reduced the workspace from 39,587 to
29,934 Rust source lines, but left only a 66-line margin under the enforced
30,000-line total. That margin was not safe for normal maintenance. This
follow-up keeps the same mainline and removes another approximately 3,000
lines of compatibility code, stale adapters, and duplicate test seams.

## Decision

The post-follow-up source baseline is:

| Measure | Before this stage | After this stage | Change |
|---|---:|---:|---:|
| All Rust source, including tests | 29,934 | 26,984 | -2,950 |
| Runtime layers (`core` + `protocol` + `host` + `app-server`) | 15,050 | 13,846 | -1,204 |
| Production code | 22,161 | 19,696 | -2,465 |
| Unit-test code | 6,485 | 6,147 | -338 |
| Integration-test code | 1,288 | 1,141 | -147 |

The current budget report is:

```text
runtime: 13846/20000
all Rust source: 26984/30000
```

The mainline remains:

```text
Protocol -> Core execution kernel -> Capabilities -> Host composition and
workflow persistence -> App Server service boundary -> CLI frontend
```

Capabilities remains one implementation crate. Host owns runtime assembly,
policy, persistence, and workflow data. App Server owns the service and
frontend control boundary. The CLI does not regain direct Host or Capabilities
orchestration.

## What was removed

- The obsolete session-event replay adapter and the unused derived mentor
  storage model were deleted. Goal verification still runs as a separate,
  tool-free check and persists its bounded verdict, but it is not a second
  replayable conversation history.
- Unused persona variants, dynamic persona file-contract parameters, and the
  associated prompt branches were removed. The bounded persona set is now
  `reviewer`, `implementer`, and `researcher`.
- Duplicate profile construction helpers, Host goal compatibility wrappers,
  capability root re-exports, discovery counters, approval clearing, and
  security accessor wrappers were removed. Canonical constructors and
  evaluators remain.
- Duplicate App Server runtime proxy methods were removed. Callers use the
  service client for management and the small internal runtime surface for
  execution.
- Unused session and model convenience accessors, process sandbox kind
  storage, and other one-purpose compatibility methods were removed.

These deletions reduce the number of names and call paths as well as the line
count. They do not remove durable session resume/fork, Goal/Plan workflows,
MCP stdio and streamable HTTP, web fetch, image handling, sandbox/security
policy, or model-provider execution.

## Verification

The following checks passed after the source cleanup:

```text
cargo fmt --all
cargo test -p mini-agent-capabilities       # 62 passed
cargo test -p mini-agent-host                # 40 passed
cargo test -p mini-agent-app-server         # 20 passed
cargo test -p mini-agent-cli                # 14 unit + 11 integration passed
cargo clippy --workspace --all-targets -- -D warnings
python scripts/line_budget.py
```

This checkout has no `justfile`, so the repository's `just test` wrapper was
not available. The changed crates were tested directly with Cargo, and the
workspace Clippy and hard line-budget gates were run. Core and protocol were
unchanged in this stage.

## Commits

The follow-up was landed in focused commits:

1. `1539a6c` through `b0ecbea`: remove CLI, extension, capability, and
   compatibility branches while keeping the canonical service path;
2. `9698702`: trim stale runtime adapters, replay/derived state, duplicate
   profiles, and redundant test seams.

Future changes should treat the 26,984-line result as the new baseline. A new
compatibility API or alternate runtime path must replace an existing concept,
be justified in an architecture note, and preserve both hard budgets.
