# Goal Mode Runtime Repair and End-to-End Evidence

Status: proposed
Date: 2026-08-28
Reviewed revision: `6f3a90e`
Review interval: `f85a965..6f3a90e` (6 commits)

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
- Repair stage working tree: `cargo test -p mini-agent-core` passes 30 tests
  (28 unit tests and 2 integration tests), `cargo test -p mini-agent-cli`
  passes 171 unit tests and 27 interactive integration tests, and
  `cargo clippy -p mini-agent-cli --all-targets -- -D warnings` passes.
- Current `python scripts/line_budget.py`: core 2,842 / 20,000 lines; all Rust
  source 27,700 / 30,000 lines, including tests. Only 2,300 workspace lines
  remain.
- A standalone reproduction using the local Tokio and core build artifacts
  confirmed both errors below without making model requests.

The current focused integration tests are local mock-provider runs of `/goal`
through the built CLI. They now cover a tool-bearing successful run, malformed
verdict failure, and a verifier tool-call attempt. The full workspace suite,
current remote CI, other operating systems, timeout-failure and retry-exhaustion
paths, restart behavior, and real-provider Goal behavior were not verified. No
paid provider calls were made. These results do not by themselves establish
release readiness.

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

The focused CLI tests, package tests, Clippy, formatting, line-budget, and diff
checks pass on Windows. Full workspace, cross-platform, timeout-failure,
rejection/retry, restart integration, and real-provider cases remain acceptance
work; this note stays proposed until those gates are complete.

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
