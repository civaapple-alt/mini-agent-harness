# Mainline Simplification: 39k to 29k Rust Lines

Status: implemented

## Context

The 0.4.0 `mini-agent-harness` codebase had reached more than 39,000 Rust
source lines. The number included production code and tests, and the growth
was concentrated in parallel execution paths, experimental adapters,
extension-discovery variants, and test layers that repeated higher-level
coverage.

The goal was to make the mainline easier to explain and maintain while
preserving the actual harness behavior: model/tool turns, bounded context and
steps, durable sessions, MCP and local extensions, approvals, host policy,
workflow control, and CLI/App Server operation.

The first prerequisite was test isolation. Home-directory and environment
dependent tests were isolated before using the test suite as evidence for the
refactor. This prevents concurrency flakes from being mistaken for behavior
regressions.

## Decision

The mainline is reduced by deleting non-mainline behavior and duplicate seams,
not by compressing the remaining design behind macros or opaque abstractions.
The current source budget is a hard release gate:

- all Rust source, including unit and integration tests: **30,000 lines**;
- runtime layers (`core` + `protocol` + `host` + `app-server`): **20,000 lines**;
- provider implementations in `capabilities` remain visible in the total
  budget but are reported separately from the runtime budget.

The measured result is:

| Measure | Before | After | Change |
|---|---:|---:|---:|
| All Rust source | 39,587 | 29,934 | -9,653 (-24.4%) |
| Production code | 27,270 | 22,161 | -5,109 |
| Unit-test code | 9,342 | 6,485 | -2,857 |
| Integration-test code | 2,975 | 1,288 | -1,687 |
| Runtime layers | 16,458 | 15,050 | -1,408 |

The current workspace is only 66 lines below the total gate. The budget is
therefore a design constraint, not merely a reporting metric.

## Simplification process

### 1. Establish a truthful baseline and gate

`scripts/line_budget.py` was changed to count all Rust source, including test
code, and to fail when either the 20k runtime limit or the 30k workspace limit
is exceeded. `scripts/test_line_budget.py` locks the report shape and the hard
failure behavior.

This corrected the earlier interpretation in which the 30k total was treated
as advisory. The source budget now reflects the actual 0.4.0 constraint.

### 2. Remove code that is not part of the mainline

The following branches were removed from the workspace and documentation:

- the separate experiments crate, prompt-weight examples, real-provider
  runner, and recovery experiments;
- the ACP adapter crate and its ACP-specific runtime profiles;
- the in-process delegated `subagent` tool, subagent trace/replay helpers, and
  related prompt/statistics code;
- marketplace and skillset discovery/configuration, including external clone
  traversal and its duplicate fixtures.

These were useful historical explorations, but they created alternate runtime
or configuration paths. They remain available as historical notes where
appropriate; they are not current `mini-agent-harness` behavior. A future
external frontend should use the App Server boundary rather than adding
another execution path inside the runtime.

### 3. Keep `Capabilities` as one crate, but narrow its boundary

`mini-agent-capabilities` remains one crate because its provider, extension,
MCP, skill, plugin, and registry concerns still form one implementation group
behind Host. Its public surface is kept small and its internal modules are
private where possible.

The supported extension surface is now deliberately bounded:

- workspace-local skills;
- installed plugins and plugin-provided skills;
- explicitly configured standalone MCP servers;
- bounded MCP tool discovery and failure/circuit-breaker behavior.

Capabilities provides implementations and registries. It does not own the
agent run loop, App Server workflow state, or CLI presentation.

### 4. Make workflow ownership explicit

The ownership model is now:

| Layer | Owns | Must not own |
|---|---|---|
| Protocol | portable messages, tool calls, events, limits, and stop contracts | runtime orchestration or persistence policy |
| Core | `Thread`, `Harness`, model/tool sequencing, context/step limits, and event semantics | filesystem/session policy, CLI, or provider discovery |
| Capabilities | provider construction, skills/plugins, MCP connections, and capability registry | workflow state or turn scheduling |
| Host | profile resolution, policy, world state, persistence seams, harness assembly, and workflow state | JSON-RPC transport or terminal presentation |
| App Server | local/JSON-RPC service boundary, thread/session/workflow methods, and frontend approval contract | a second runtime or duplicated workflow store |
| CLI | arguments, terminal interaction, command queueing, rendering, and App Server client calls | direct Host/Capabilities orchestration |

The conceptual call path is:

```text
CLI
  -> LocalAppServerClient / App Server frontend
    -> AppServerRuntime and JSON-RPC service
      -> Host profile + HostWorkflowStore + harness assembly
        -> Capabilities registry / selected MCP / skills / plugins
          -> Core Thread -> Harness -> model/tool steps -> events
            -> Protocol contracts
```

Host owns the workflow data and runtime assembly. App Server owns the public
service methods and delegates execution/state decisions to Host. CLI only
drives that service boundary. This keeps Goal, Plan, verifier, restart,
follow-up, and steer operations on one control path.

### 5. Delete compatibility duplicates and redundant test seams

The redundant `RuntimeBuilder`-style and App Server runtime convenience
wrappers were removed. The generic model-factory/control entry points remain;
new callers should use the canonical entry point instead of adding another
boolean/optional constructor variant.

Tests were reduced by removing cases that duplicated public behavior already
covered by App Server or CLI integration tests, fixtures for deleted features,
and static-value assertions. Coverage was retained for the meaningful paths:

- Core turn sequencing, loop detection, context and step limits;
- Host profile, policy, world, persistence, and harness assembly;
- MCP stdio/HTTP behavior, approvals, failure isolation, and circuit breaking;
- App Server public JSON-RPC behavior;
- CLI stdin, profiles, model-only mode, follow-up, steer, durable resume,
  restart, Mentor, Goal, timeout, and auto-mode workflows.

The test rule is therefore “fewer layers, stronger boundary coverage”: remove
duplicate implementation-level tests only when a public workflow test still
proves the behavior.

## Resulting crate map

The post-simplification line report is:

| Crate/layer | Total | Production | Unit | Integration |
|---|---:|---:|---:|---:|
| `mini-agent-core` | 3,143 | 1,725 | 1,418 | 0 |
| `mini-agent-protocol` | 699 | 519 | 180 | 0 |
| `mini-agent-capabilities` | 10,710 | 8,252 | 2,458 | 0 |
| `mini-agent-host` | 4,477 | 3,573 | 904 | 0 |
| `mini-agent-app-server` | 6,039 | 4,913 | 1,126 | 0 |
| `mini-agent-app-server-protocol` | 692 | 623 | 69 | 0 |
| `mini-agent-cli` | 4,174 | 2,556 | 330 | 1,288 |
| **Workspace total** | **29,934** | **22,161** | **6,485** | **1,288** |

This is a deliberately asymmetric split: Capabilities is the largest
implementation crate, while the runtime control path is kept compact. Splitting
Capabilities into more crates now would add dependency and API surface without
removing a current ownership conflict.

## Verification

The final state was checked with:

```text
cargo fmt --all
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python scripts/test_line_budget.py
python scripts/test_package_release.py
git diff HEAD --check
```

All checks passed. `python scripts/line_budget.py` reports 29,934 total lines
and 15,050 runtime lines.

The implementation was landed in small, reviewable commits:

1. enforce the corrected total source gate;
2. remove non-mainline experiments;
3. narrow Capabilities, remove ACP/subagent edges, and collapse duplicate test
   layers;
4. remove redundant runtime/test seams;
5. align line-budget tests and extension examples with the current mainline.

## Consequences and guardrails

- The mainline has one execution path: CLI/App Server → Host → Core, with
  Capabilities behind Host as the implementation group.
- New features must identify their owning layer and the canonical call path
  before code is added.
- New frontends must use App Server; they must not reintroduce a parallel
  runtime profile or direct CLI-to-Host orchestration.
- New extension discovery must stay bounded and workspace-local or explicitly
  configured; marketplace/skillset clone traversal is not part of the product
  surface.
- If a feature cannot fit under the remaining 66-line margin, it must be
  split, replace an existing concept, or be proposed as a separately scoped
  experiment.
- The older notes describing ACP, delegated subagents, or marketplace
  discovery are historical records. This note defines the current 0.4.0
  mainline after their removal.
