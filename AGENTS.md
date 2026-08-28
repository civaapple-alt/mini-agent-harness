# Mini Agent Harness development contract

## Purpose

Mini Agent Harness studies agent harness behavior. It is not a smaller copy of
every feature in Codex, Pi, fx, or Qi.

## Hard boundaries

- `mini-agent-core` owns only portable model/tool contracts, the explicit run
  loop, limits, stop classification, and observation events.
- Providers, files, processes, approval UI, persistence, and terminal output
  stay outside core.
- Do not add a framework for policies, hooks, plugins, storage, scheduling, or
  dependency injection before a concrete experiment needs it.
- Passive observers never alter execution.
- Default behavior is preferred over configuration. Add an option only when
  two useful behaviors must coexist.
- Never make model-visible input unbounded. New message, tool, or event shapes
  must fit an existing hard limit or introduce one directly.

## Size budget

- Runtime hard limit: 20,000 Rust source lines across `core`, `protocol`,
  `host`, and `app-server`. The separately reported `acp` edge is excluded
  from this runtime limit.
- Whole workspace hard limit: 30,000 Rust source lines.
- The CLI is excluded from the runtime limit but included in the whole
  workspace limit. Tests count toward both limits.
- Run `python scripts/line_budget.py` after code changes.

The limit is a ceiling, not a target. Removing a concept is better than fitting
it behind a shorter abstraction.

## Change test

Before adding a core concept, identify:

1. the harness hypothesis it tests;
2. the trace evidence that distinguishes outcomes;
3. why the feature cannot live in a host adapter;
4. the permanent complexity it adds.

If these are unclear, do not add it. Consult `.agents/notes/README.md` for architecture decisions and guardrails, and `docs/` for current specifications.

## Verification

After Rust changes, run:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python scripts/line_budget.py
```

For CLI behavior, also run the built binary through the changed path. Do not
use paid provider calls unless the user explicitly authorizes them.
