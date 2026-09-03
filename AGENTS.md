# Mini Agent Harness development contract

## Purpose

Mini Agent Harness studies agent harness behavior. It is not a smaller copy of
every feature in Codex, Pi, fx, or Qi.

## Documentation topology

- `README.md` is the lightweight project entry point. It owns project identity,
  the shortest install/run path, and the cross-directory documentation map.
- `docs/README.md` is the local index for stable specifications and runbooks in
  `docs/`; each `docs/*.md` file owns one topic and is not a second project map.
- `.agents/notes/README.md` is a lightweight index for decisions and proposals.
  Dated notes are evidence and rationale, not the current product specification.
- A README under a directory describes only that directory and its children. Do
  not repeat neighboring README content or add cross-directory documentation
  maps below the root.
- Keep one canonical explanation per topic. Move CLI detail to `docs/cli.md`,
  protocol detail to `docs/app-server.md`, configuration to
  `docs/configuration.md`, boundaries to `docs/harness-boundaries.md`, and
  limits to `docs/limits.md`; do not append these sections back to root README.
- Historical records and dated notes are frozen or status-indexed. Do not turn
  an old note into a running changelog by appending every subsequent batch.

## Hard boundaries

- `mini-agent-core` owns only portable model/tool contracts, the explicit run
  loop, limits, stop classification, and observation events.
- Providers, files, processes, approval UI, persistence, and terminal output
  stay outside core.
- Tool boundaries are explicit: Core `ToolRouter` resolves by name; protocol
  `ToolHandler` parses/describes admission; Host `ToolOrchestrator` orders
  admission, approval, and execution; `ToolRuntime` owns the concrete side
  effect and its configured sandbox. Core still owns the turn loop, events, and
  conversation-history writeback.
- Stable built-in prompt bodies belong to crate-owned `builtin/prompts` Markdown
  assets and are embedded at compile time. Host-owned project, extension, world,
  and workflow instructions remain bounded runtime composition. App Server may
  select an allowlisted startup profile, but must not expose arbitrary raw
  system-prompt replacement over the public protocol; local `ReplaceConfig` is
  an internal frontend control-plane seam.
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
- Release-source hard limit: 30,000 Rust source lines across Core, Protocol,
  Capabilities, Host, and App Server.
- The CLI, including the experimental REPL, is reported separately and is
  excluded from the release-source limit. Tests in release packages count.
- Run `python scripts/line_budget.py` after code changes.

The limit is a ceiling, not a target. Removing a concept is better than fitting
it behind a shorter abstraction.

## Change admission

For every feature, refactor, test change, or protocol change, answer the six
questions in `.github/pull_request_template.md` before implementation:

1. Does the change belong to Core, Host, Capabilities, App Server, or CLI?
2. Does an existing path or type already own the same responsibility?
3. Can an old concept be removed or replaced instead of adding another layer?
4. What is the expected and actual net line delta for runtime and release
   source (excluding experimental CLI/REPL)?
5. Does it expand model-visible input, events, persistence, or public protocol?
6. Can existing public boundary tests cover it, and what evidence is missing?

On pull requests, CI mechanically checks that the six-question section is
present, all answer placeholders are replaced, all six questions have answers,
and all six admission boxes are checked. The check validates completion only;
reviewers still judge the answer quality and architecture.

New code defaults to net-zero growth or must identify an explicit offset. Never
remove Core tests, Actor/CAS/Session authority, or public protocol behavior only
to satisfy the approximate Stage 1 target. The 20,000-line runtime and
30,000-line release-source ceilings remain hard gates; experimental CLI/REPL
growth is informational until it is promoted into the supported surface.

If a change affects prompt, tool schema, loop-control, context, events, or
persistence, public unit tests alone are not sufficient: add bounded Harness
Scenario/Eval evidence and update the current project documentation as needed.

## Change test

Before adding a core concept, identify:

1. the harness hypothesis it tests;
2. the trace evidence that distinguishes outcomes;
3. why the feature cannot live in a host adapter;
4. the permanent complexity it adds.

If these are unclear, do not add it. Use `docs/` for current specifications and
`.agents/notes/` for dated decisions and experiment records.

## Verification

After Rust changes, run the affected package tests and lint checks, then:

```sh
cargo fmt --all
cargo clippy -p <affected-package> --all-targets -- -D warnings
cargo test -p <affected-package>
python scripts/line_budget.py
```

Run `cargo clippy --workspace --all-targets -- -D warnings` when the change
crosses package boundaries or as part of release/CI validation. Run
`cargo test --workspace` only with explicit approval for a local full-suite
run; CI runs the full workspace matrix. For CLI behavior, also run the built
binary through the changed path. Do not use paid provider calls unless the user
explicitly authorizes them.
