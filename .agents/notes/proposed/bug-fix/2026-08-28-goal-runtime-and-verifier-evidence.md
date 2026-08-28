# Goal Mode Runtime Repair and End-to-End Evidence

Status: proposed
Date: 2026-08-28
Reviewed revision: `6f3a90e`
Review interval: `f85a965..6f3a90e` (6 commits)
Current evidence tree: `4934ac9` plus uncommitted working-tree changes

## Current architecture update (2026-08-28)

The Goal state machine, checkpoint persistence, and provider configuration
remain Host responsibilities. Mentor and Goal verifier turn orchestration now
lives in `mini-agent-app-server::mentor` and executes through the App Server
local client, so the verifier no longer owns a separate direct `Harness::run`
loop. The acceptance evidence below still applies to the Goal behavior; this
update only records the execution boundary used by the current worktree.

## Context

This is a focused follow-up to [Stabilization and Evidence Gates](../process/2026-08-27-stabilization-and-evidence-gates.md).
The latest changes improve runtime boundaries, add bounded real-provider
scenarios, and connect Goal execution to checkpoint settlement, verification,
and milestone advancement. However, two locally reproduced defects prevent
the intended Goal workflow from completing.

The earlier stabilization note describes the continuation added in `8243e56`
as operational. At the reviewed revision, that description should be treated
as an intended control flow, not evidence of a working user journey. This
proposal takes precedence for the current Goal readiness assessment; it does
not reopen every historical finding in the broader proposal.

| Previous finding | Status at the reviewed revision |
| --- | --- |
| DNS admission based only on hostname text | Resolved-address validation, connection IP pinning, and rejection of cross-host redirects are implemented. This is not a complete network security audit or verification of every proxy configuration. |
| Compaction summary exceeds the User item limit after prefix insertion | Fixed by truncating the complete prefixed item; the new regression test passes. |
| Goal state helpers are disconnected from the execution loop | Continuation is now connected, but runtime construction and verifier history admission still block the product path. |

## Evidence and Limits

The initial review ran on Windows against `6f3a90e`; the implementation
evidence below is from the current working tree:

- Baseline `6f3a90e`: `cargo test -p mini-agent-core` passed 29 tests (27 unit
  tests and 2 integration tests); the focused CLI Goal test did not exist.
- Current working tree: `cargo test --workspace --quiet` passes every package,
  including 40 core tests, 161 host tests, 20 app-server tests, 4 wire protocol
  tests, 2 ACP tests, 32 built-CLI interactive tests, and the remaining CLI and
  protocol suites. `cargo clippy --workspace --all-targets -- -D warnings`
  also passes.
- `cargo +1.88.0 check --workspace --locked` passes on Windows, covering the
  new host, app-server, wire protocol, and ACP crates against the declared
  minimum compiler.
- `cargo build --workspace --release` passes on Windows. This validates the
  release profile locally, but is not a packaged cross-platform release.
- `cargo package --workspace --locked --no-verify --allow-dirty` and
  `python -m unittest scripts/test_package_release.py` pass locally. The
  package command uses `--allow-dirty` only because this candidate is not yet
  committed.
- The release binary smoke path (`mini-agent --version`, `status --json`, and
  `demo "make this loud"`) passes locally without making a provider request.
- Baseline remote [CI #56 for `4934ac9`](https://github.com/civaapple-alt/mini-agent-harness/actions/runs/33144991077)
  passed the Ubuntu, macOS, Windows, and Rust 1.88 jobs; its quality job
  failed only the line-budget check. That run does not include this working
  tree.
- Current `python scripts/line_budget.py`: runtime layers (core + protocol +
  host + app-server) 26,792 / 20,000 lines; all Rust source 35,116 / 30,000
  lines, including tests. The ACP edge is reported separately and excluded
  from the runtime gate. The runtime gate is over budget by 6,792 lines and
  the repository-wide gate is over budget by 5,116 lines. The diagnostic layer
  breakdown is core 4,058, protocol 699, host 17,836, app-server 4,199,
  acp 767, and CLI 7,557 lines. The same report separates production/unit/integration
  lines as core 1,781/1,928/349, protocol 519/180/0, host 12,796/5,040/0,
  app-server 3,274/941/0, acp 557/210/0, and CLI 4,614/486/2,457.
- Using temporary Zig compiler/linker/archive wrappers on Windows, the Linux
  target `cargo check` passes. A stronger
  `cargo test --workspace --target x86_64-unknown-linux-gnu --no-run` attempt
  reaches test-binary linking but cannot resolve Linux system libraries
  (`util`, `rt`, `dl`, `pthread`, and `c`) from this Windows-only toolchain.
  It is recorded as blocked cross-target linking, not as a test pass. Native
  Linux runtime and CI remain authoritative for that lane.
- A standalone reproduction using the local Tokio and core build artifacts
  confirmed both errors below without making model requests.

The focused integration tests are local mock-provider runs of `/goal` through
the built CLI. The current tree covers a tool-bearing success, malformed and
tool-using verifier failures, deterministic timeout, rejection/retry/
exhaustion, and restart. The full workspace suite passes locally. Remote CI
for the current uncommitted tree and real-provider Goal behavior remain
unverified; no paid provider calls were authorized or made. These results do
not by themselves establish release readiness.

## Placement in the four-layer architecture

The Goal state repair remains a Host workflow; its verifier turn is exposed
through the App Server and does not move orchestration into the execution
kernel:

```text
CLI client
    ↓
App Server service boundary  ← Mentor/verifier turn orchestration
    ↓
Host / Workflows application host  ← Goal, persistence, provider setup
    ↓
Core / Protocol execution foundation ← Thread, Harness, limits, turns, events
```

The CLI owns REPL input and worker scheduling. The app-server boundary can
project the same settled Thread/Turn evidence, while `mini-agent-host` owns
Goal state, verifier configuration, and durable session files. Core receives
only bounded history and remains unaware of Goal or provider credentials.

## Implementation Update

The working tree now contains the first repair stage and its focused evidence:

- Goal timeout construction is inside the worker's `block_on(async { ... })`
  boundary, so timer creation has a Tokio reactor.
- Mentor and Goal verifier harnesses restore bounded settled history with the
  normal tool-call ceiling, then replace the configuration with a tool-free
  inference configuration. Historical tool evidence remains available while
  new verifier tool calls remain disabled.
- `verifier_can_restore_tool_history_before_disabling_new_tool_calls` covers the
  configuration transition at the core boundary.
- `goal_mode_runs_a_tool_turn_and_verifies_the_settled_history` drives the
  built CLI against a local primary/verifier fixture. It executes a shell tool,
  verifies the resulting history three times, checks empty verifier tools and
  evidence binding, and observes `goal/state.json` reaching `converged`.
- Malformed verifier output is classified as `Invalid`, persisted as a derived
  verdict artifact, and fails the Goal without advancing its milestone.
- Verifier tool-call attempts fail the Goal before any tool executes; the
  verifier request still carries an empty tool list.
- Verifier and execution failures now consistently persist `failed` state and
  clear the active Goal. A resumed session with a leftover Running Goal is
  marked `user_paused`, requiring an explicit new `/goal` command.
- `advance_goal_milestone` is idempotent for terminal Goal states, so a late
  result cannot mutate a converged or failed state.
- Goal limits are resolved from the host `.env` (`MINI_AGENT_GOAL_MAX_LOOPS`,
  `MINI_AGENT_GOAL_STEP_BUDGET`, and `MINI_AGENT_GOAL_TIMEOUT_SECS`) so timeout
  and retry exhaustion can be exercised with bounded fixtures.
- Session resume reclaims a lock only when the PID recorded in the lock file is
  no longer alive, using an atomic rename before removal to avoid deleting a
  newly acquired lock.

The focused CLI tests now include deterministic timeout, rejected/retry/
exhausted-retry, and restart cases. Package tests and Clippy pass on Windows;
the full workspace test also passes. The line-budget gate remains over the
30,000-line ceiling after the boundary extraction, and the current uncommitted
tree has not run in remote CI. Real-provider Goal behavior remains an
explicitly authorized release smoke test; no paid calls were made here. This
note stays proposed until the remaining release gates are either passed or
explicitly narrowed.

### Current acceptance matrix

| Requirement | Evidence in the current tree | Result |
| --- | --- | --- |
| Timeout branch | `goal_mode_timeout_is_deterministic_and_keeps_repl_alive` with `MINI_AGENT_GOAL_TIMEOUT_SECS=1` and a stalled local provider | passed |
| Rejected, retry, exhausted retry | `goal_mode_retries_rejected_milestones_and_exhausts_budget` with a two-attempt budget | passed |
| Restart behavior | `running_goal_is_paused_when_a_session_restarts`, including stale lock reclamation | passed |
| Verifier history/tool denial | successful, malformed, and tool-call verifier fixtures plus host admission tests | passed |
| Full workspace | `cargo test --workspace --quiet` | passed locally on Windows |
| Minimum compiler | `cargo +1.88.0 check --workspace --locked` | passed locally on Windows |
| Linux target | `cargo check --workspace --target x86_64-unknown-linux-gnu` with temporary Zig wrappers; test-binary `--no-run` attempt | check passed as cross-compile; test linking lacks Linux system libraries; native runtime pending |
| macOS/Linux/CI | baseline CI #56 passed all OS jobs, but did not contain this uncommitted tree | candidate evidence pending |
| Real provider Goal behavior | not run; paid provider calls were not authorized for this verification pass | intentionally pending |
| Release profile | `cargo build --workspace --release` | passed locally on Windows; packaging and other OS release builds pending |
| Built binary smoke | `target/release/mini-agent.exe --version`, `status --json`, `demo` | passed locally on Windows without provider I/O |
| Package/release scripts | `cargo package --workspace --locked --no-verify --allow-dirty`; `python -m unittest scripts/test_package_release.py` | passed locally; clean-tree CI still pending |
| Repository line budget | `python scripts/line_budget.py`: 34,377/30,000 Rust lines | failed; cleanup or an explicitly approved budget decision is required |

## Confirmed Defects

### P1: Goal timeout construction panics outside the Tokio runtime

In [repl.rs](../../../../crates/mini-agent-cli/src/repl.rs), around line 478,
the ordinary worker thread evaluates:

```rust
model_runtime.block_on(tokio::time::timeout(
    timeout,
    harness.run(prompt.clone(), &mut observer),
))
```

Rust evaluates the argument before calling `block_on`. Constructing the Tokio
timer therefore happens before entering the runtime. Starting Goal Mode with
a durable session and verifier configuration reaches this path and panics.
The standalone worker-thread reproduction reports:

```text
there is no reactor running, must be called from the context of a Tokio 1.x runtime
```

### P1: Tool-free verifier settings reject historical tool evidence

In [mentor.rs](../../../../crates/mini-agent-cli/src/mentor.rs), around lines
169-178, `verify_checkpoint` configures `max_tool_calls_per_step: 0` before
calling `restore_history`. The latter validates historical assistant tool
calls against the same limit in
[harness.rs](../../../../crates/mini-agent-core/src/harness.rs).

A settled history containing one assistant tool call and its tool result fails
before contacting the verifier:

```text
LimitExceeded { kind: ToolCallsPerStep, limit: 0, actual: 1 }
```

Fixing the timer alone will therefore not make normal tool-using milestones
verifiable. The standalone `mentor` command uses the same configuration
ordering and should receive the same narrowly scoped repair.

### Evidence gap: the Goal scenario bypasses both failing boundaries

The Goal scenario in [real_llm.rs](../../../../crates/mini-agent-cli/examples/real_llm.rs),
around lines 1248-1274, constructs its own harness, supplies an artificial
history without tool calls, and requests the exact outcome `verdict: approved`
with a score of 100. It does not execute the REPL worker or call the production
`mentor::verify_checkpoint` path.

This can check provider connectivity, verdict parsing, persistence, and a
positive state transition. It cannot prove the `/goal` workflow works or that
the verifier independently distinguishes sufficient from insufficient
evidence. The [runner note](../../implemented/testing/2026-08-27-real-llm-scenario-runner.md)
already limits its scope; release evidence must preserve that distinction.

## Proposal

### 1. Construct the timeout inside the worker runtime

Keep ownership in the existing CLI worker and construct/await the timer inside
`model_runtime.block_on(async { ... })`. Do not add a runtime abstraction or
move Goal orchestration into core.

Add a regression through the built CLI with `--persist`, a local mock primary
provider, a local mock verifier, and the actual `/goal <objective>` input.
It must reach the first provider request without panic. Exercise a stalled
asynchronous provider response with a bounded fixture to verify the timeout
branch and an explicit failed state, without waiting the production 600 seconds.

Tokio timeout is cooperative: it does not interrupt synchronous `Tool::execute`
work. Preserve existing tool/process bounds and do not claim that this timer
repair provides hard cancellation of all effects. On interruption, do not
publish an incomplete tool group as a settled checkpoint or replay uncertain
effects on resume.

### 2. Separate history admission from new verifier tool-call restrictions

Prefer a host-only fix using existing APIs: restore the settled evidence with
the normal bounded history limits, then apply the tool-free verifier run
configuration before inference. Keep the verifier registry and request tool
list empty, with new model tool calls rejected before execution.

Preserve all existing per-item and total-context byte limits during restoration.
Do not bypass validation, use unlimited call counts, drop historical tool
messages, or rewrite the source session. Reject unsupported oversized evidence
explicitly. Avoid a new core policy type or public configuration field for
this repair unless the existing configuration transition proves insufficient.

Apply this distinction to both `verify_checkpoint` and the standalone mentor
entry point. The verifier must receive the tool evidence bound to the settled
source checkpoint, while its output remains a separate derived artifact.

### 3. Make the acceptance gate exercise production control flow

Extend the existing local HTTP/SSE and child-process fixtures in
[interactive.rs](../../../../crates/mini-agent-cli/tests/interactive.rs).
Mock only the external providers; do not replace the worker, checkpoint store,
verifier entry point, or milestone transition with an alternative implementation.

Require an actual local tool call and result in the worker history. Inspect
the verifier request and persisted artifacts, rather than relying only on
assistant prose or a successful exit code.

| Case | Required observable result |
| --- | --- |
| Startup and approved milestone | `/goal` reaches the worker provider, executes a fixture tool, settles a checkpoint, invokes the verifier with that evidence, records its source checkpoint sequence, and advances exactly once. |
| Final approval | The last milestone becomes `converged`; no additional worker/verifier requests follow. |
| Rejected milestone | Milestone number is unchanged; verdict is persisted and available to the bounded retry. Later approval advances it; exhausted retries produce a failed state. |
| Missing or invalid verification | Provider error, missing evidence, or malformed verdict never advances or converges. Record an explicit stopped/failed outcome instead of presenting success. |
| Verifier attempts a tool call | No tool executes and the milestone does not advance, even though historical tool evidence was accepted. |
| Execution timeout or step exhaustion | Goal fails without an approval or advancement; the CLI remains responsive and the last settled checkpoint remains readable. |
| Standalone mentor with tool history | A normal settled tool turn is admitted without changing the source checkpoint or permitting new verifier effects. |

Bound child lifetimes, requests, retries, and captured output. Use disposable
workspaces and local services only. No paid call is necessary for this gate.

Retain the real-provider Goal scenario as a clearly labelled component smoke
test, or revise it to call the production verifier entry point with realistic
tool evidence. Do not count an explicitly requested `approved` answer as a
quality evaluation. Any later paid evaluation needs separate authorization,
positive and negative evidence cases, neutral output-format instructions, and
recorded failure results as well as successes.

## Delivery and Acceptance

Deliver reviewable stages: timer repair with its CLI regression; verifier
history repair with evidence/denial regressions; then workflow failure cases
and documentation alignment. Keep each non-mechanical stage below roughly
500 changed lines. Reuse fixtures, avoid expanding the existing 1,898-line
scenario runner, and do not raise the workspace line ceiling.

Acceptance requires both reproduced failures to disappear on the production
path and the cases above to have deterministic evidence. Run the repository
verification contract on the candidate revision: formatting, Clippy, workspace
tests, and the line-budget check. Record the revision and environment; inspect
relevant Windows, macOS, Linux, and MSRV CI results before making corresponding
support claims. A passing component suite alone is insufficient.

Update README, current Goal documentation, and the earlier stabilization note
only to the level supported by those results. Keep this note proposed until
the implementation and evidence land; then move it to `implemented/bug-fix/`
with actual commands and outcomes.

## Non-Goals

This proposal does not authorize implementation, a release, or paid evaluation.
It does not add providers, tools, a scheduler, a policy framework, new
model-visible message shapes, or automatic recovery of uncertain effects.
It does not certify all network, cancellation, or session-resume behavior.
