# Stabilization Phase: Runtime Contracts and Evidence Gates

Status: proposed

## Context

Mini Agent Harness studies harness behavior. Feature count and similarity to
other agents are not the success criteria. The next development phase should
make existing runtime behavior match its documented contracts before expanding
the capability surface again.

This assessment covers `8a53cbf9b04b6d646369bdabf8b2b2bba2a72bdc`
through `f85a965b113a81df688fb774eef49546c161dedb`. By author timestamps,
the interval spans approximately 24 hours and contains 52 commits, including
9 feature commits, 21 fixes, and 2 refactors. Rust source grew by 7,120 lines,
from 17,724 to 24,844, approximately 40%. Tests count toward these totals.
The workspace has 5,156 lines remaining under its 30,000-line ceiling;
core occupies 2,787 of its 20,000-line ceiling. Neither ceiling is a target.

The changes include useful separation of CLI dispatch, harness assembly, and
provider protocols; message restoration checks; tool-group compaction; and
session image attachment recovery. Most integration complexity remains outside
core. However, Plan, Goal, subagents, session layout, web access, and vision were
expanded together. Follow-up fixes repeatedly addressed gaps in complete user
flows, making integration evidence more urgent than another capability.

## Maturity Assessment and Evidence Limits

| Area | Assessment at the reviewed revision |
| --- | --- |
| Core loop | Small and inspectable, with explicit contracts, limits, and passive events. Existing boundaries are worth preserving. |
| Engineering infrastructure | CI is configured for Linux, macOS, Windows, MSRV, lint, line budgets, release packaging, and binary smoke tests. Configuration is not proof of successful runs. |
| Host workflows | Useful Alpha functionality, with uneven integration across planning, goals, delegation, persistence, and providers. |
| Safety and unattended execution | Not ready for a broad reliability claim while permission and network boundaries remain incomplete. |
| Harness research | Trace and experiment infrastructure exists; added capabilities still need task-level evidence of benefit and cost. |

The assessment is a static quick review, not a complete security audit or a
cross-platform certification. Local verification ran
`cargo test -p mini-agent-core --offline`: 28 tests passed. The line-budget check
also passed. Full workspace tests, remote CI results, and live provider behavior
were not verified in this review. No paid provider calls were made.

The boundary fixes from this review were shipped in `8243e56`. This note
remains proposed because the evidence-gate process and real-provider value
evaluation were not completed by that change; the findings below are retained
as the acceptance checklist and historical rationale, not as a claim that the
current source still has each defect.

The original development environment was Windows; the current validation
environment is macOS. Treat those as separate acceptance lanes: Windows-only
PowerShell, Job Object, and Mark-of-the-Web behavior requires Windows CI or a
Windows developer run, while macOS shell, path, and `open` behavior must be
compiled and exercised locally. A green macOS run is not evidence that the
Windows lane is healthy, and the reverse also applies.

The inspected GitHub Actions run [CI #51](https://github.com/civaapple-alt/mini-agent-harness/actions/runs/33062182849), for `f85a965`, failed its quality and Rust 1.88 gates while all three OS test jobs passed. Quality reported Windows-only helper dead-code and non-Windows unused-variable warnings; the current working-tree changes close those warnings. The MSRV job reported three Rust 1.88 errors from `if let` match guards in `repl.rs` and `web.rs`; the current working-tree changes replace them with stable control flow. This run predates the current working-tree changes and is not evidence that the candidate revision passes.

The later baseline [CI #56](https://github.com/civaapple-alt/mini-agent-harness/actions/runs/33144991077)
passed the Ubuntu, macOS, Windows, and Rust 1.88 jobs for `4934ac9`; its only
quality failure was the line budget (`30,828/30,000`). The current working tree
extends the boundary into host, app-server, wire-protocol, and ACP crates and
reported `34,377/30,000` at that earlier review point. This keeps the evidence gate active: local
tests and cross-target checks do not waive the size gate or substitute for CI
on the candidate revision.

The current Windows pass also includes the release profile, dirty-tree package
validation, and the four release-packager unit tests. Linux target library
compilation succeeds through temporary Zig wrappers, but cross-target test
binary linking cannot find the Linux system libraries on this Windows host; a
native Linux runner remains required for runtime evidence.

Current evidence update (2026-08-29, `e317b14`): the complete workspace
`cargo test --workspace --all-targets -- --test-threads=1` run passed, as did
workspace Clippy with `-D warnings`. The line report is now runtime
`14,384/20,000` and all Rust source `36,858/30,000`; the latter remains an
open gate. The current tree is committed and clean, but candidate CI and
native macOS/Linux runtime evidence are still absent. MSRV
`cargo +1.88.0 check --workspace --all-targets --locked` and
`cargo build --workspace --release --locked` also passed on Windows.

## Proposal

Use the next development iteration for stabilization. Exit by evidence, not by
elapsed time, number of commits, or number of implemented features. Pause new
providers, tools, personas, and orchestration features unless a narrowly scoped
change is necessary to close a named acceptance gap below.

The working hypothesis is that enforcing existing boundaries and completing
existing workflows will improve reliable task completion more than adding
another capability. Deterministic fixtures can establish contract correctness;
they cannot establish a real-model success-rate improvement on their own.

### Stage 1: Close Boundary Defects

These findings are grounded in the reviewed source. Reproduce each with a
bounded regression fixture before implementing the fix.

| Priority | Finding and source | Required acceptance evidence |
| --- | --- | --- |
| P1 | Plan Mode checks file mutation paths, but `Shell::execute` does not check planning state. `SpawnAgent` starts a child with automatic approval without inheriting the planning restriction. See the current [workspace boundary](../../../../crates/mini-agent-capabilities/src/workspace.rs); the former CLI `subagent.rs` path was removed during mainline simplification. | Enter Plan Mode through the CLI. The living plan remains writable; direct edits, mutating shell/process actions, and delegated mutations cannot modify a sentinel workspace file. Check MCP and follow-up delegation paths too. |
| P1 | `classify_domain` admits public-looking names without checking their resolved addresses. `fetch_admitted` delegates DNS to the HTTP client; redirect checks repeat the textual classification. See [web.rs](../../../../crates/mini-agent-capabilities/src/web.rs). | A controlled resolver fixture proves that a public-looking hostname cannot reach private, metadata, or loopback addresses, including redirects. Bind the connection to checked addresses so a second lookup cannot bypass the decision. Explicitly allowed loopback URLs must still work. Do not probe real internal services. |
| P2 | `assemble_compacted` truncates summary text to `max_user_input_bytes` before adding its prefix, so the complete generated User item can exceed that limit and fail restoration. See [harness.rs](../../../../crates/mini-agent-core/src/harness.rs). | Summaries at and above the limit, including multibyte UTF-8 and small configured limits, produce complete items within the limit. Persisting and restoring the resulting conversation succeeds without losing history. |

Keep the fixes at their existing boundaries. Planning permissions and network
admission belong in the CLI host. Summary size accounting belongs in the
existing core limit implementation; it needs no new message type or policy
framework. Model instructions alone do not enforce read-only behavior.

Arbitrary shell commands and remote tools must be denied in Plan Mode unless
their permitted behavior can actually be enforced. Do not treat command-name
heuristics as a read-only guarantee. Automatic approval must not weaken a mode
restriction, and descendants must not receive broader authority than the
parent. Native process containment must not be described as filesystem or
network isolation.

### Stage 2: Verify Complete Host Workflows

Goal Mode was the clearest gap between implementation and documentation. The
stabilization implementation shipped in `8243e56` now runs a bounded
milestone, settles it in a durable checkpoint, invokes a tool-free independent
verifier, records the
verdict with its source checkpoint sequence, and advances only on approval.
Rejection keeps the milestone in place; step, time, and model failures mark the
goal failed. Stored milestone budgets are now applied to the active harness
configuration, with the host timeout enforcing the wall-clock bound. See
[repl.rs](../../../../crates/mini-agent-cli/src/repl.rs) and
[goal.rs](../../../../crates/mini-agent-host/src/goal.rs).

For Goal Mode, prefer the smallest CLI-host continuation that demonstrates
execution, settled checkpoint, independent verification, and a persisted
advance or stop decision. If that cannot fit this stabilization scope without
a new orchestration framework, explicitly narrow the shipped behavior and
mark automatic milestone progression as experimental or unavailable. Do not
retain a complete-autonomy claim backed only by helper functions and tests.

Extend existing integration fixtures to cover these user journeys:

- **Plan:** enter with a prompt, update the session living plan, reject other
  mutations, and leave the mode. Resume must restore the restriction or clearly
  report that it is not restored; historical instructions must not claim a
  restriction that the host no longer enforces.
- **Goal:** execute a milestone, bind verification to its settled checkpoint,
  and advance only on approval. Rejection, missing verification, step/time
  exhaustion, and restart must have explicit outcomes. If progression remains
  unavailable, CLI output and documentation must state that limitation.
- **Subagent:** spawn, receive a result, send a follow-up, and inspect the latest
  status. Cover timeout and step-limit recovery without silently losing history
  or widening permissions. A separate process still shares the workspace unless
  an actual isolation mechanism says otherwise.
- **Session and provider:** complete a tool turn, compact, restart, resume, and
  fork. Retained image references must resolve to the intended attachments;
  expired process/result handles must not masquerade as live effects. Cover
  text and image request shapes using local provider fixtures.

Reuse existing session records, trace events, and test servers. Each journey
needs at least a success case and a meaningful failure case through the built
binary. Passing helper-level state transitions alone does not close a journey.

### Stage 3: Align Claims and Measure Value

Reconcile README, CLI help, current specifications, and active implemented
notes against exercised behavior. Known examples include response and AGENTS.md
limits, persistence defaults, the distinction between core and CLI zero-step
semantics, and Goal/Plan guarantees. Historical archived notes remain frozen.
Do not relabel an implementation as complete merely because its proposal was
written or a state file exists.

For each retained claim, record a reproducible command or fixture, expected
outcome, and the relevant trace/session evidence. Include the revision and
environment, and redact credentials, private prompts, and image content from
any published evidence. Keep the evidence set small and bounded.

Once contract tests pass, compare a fixed set of tasks using existing trace
tools: completion against acceptance criteria, model steps, tool failures,
compactions, latency, and provider-reported usage. Hold fixtures and model
settings constant, report failures as well as successes, and record repetition
counts and sampling settings. Do not infer general model quality from a single
successful run or synthetic fixtures. Real-provider comparisons require
separate explicit authorization for paid calls; they are not a prerequisite
for the offline safety fixes. The existing
[prompt-weight proposal](../../rejected/feature/2026-08-26-prompt-weight-evaluation.md)
can supply a later evaluation without creating another benchmark framework.

## Delivery and Exit Criteria

Prefer one behavior change per reviewable batch: reproduction, minimal fix,
integration evidence, and affected documentation. Defer unrelated refactors.
Add tests where the behavior already has a natural home; remove unused concepts
when they provide no demonstrated value. No line-budget increase is proposed.

The stabilization phase should exit only when:

- Every listed boundary defect is fixed and has a regression test.
- Each workflow claim retained in the release has a reproducible built-binary
  success/failure path. Incomplete claims are explicitly narrowed or withdrawn.
- Resume and compaction do not silently discard valid conversation state, and
  planning/delegation restrictions hold across the tested execution boundaries.
- The full development contract passes for the candidate revision:
  `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, and `python scripts/line_budget.py` (`python3` where
  the `python` executable is unavailable). Relevant CLI paths are exercised.
- CI results for the candidate revision are inspected, including all three
  platforms and MSRV. Unverified environments are recorded as gaps.
- Documentation distinguishes demonstrated behavior, experimental behavior,
  and unverified provider behavior. No unattended-safety claim exceeds evidence.

Afterward, resume feature work one experiment at a time. A proposed core concept
must still identify its hypothesis, distinguishing trace evidence, why a host
adapter is insufficient, and its permanent complexity, as required by
[AGENTS.md](../../../../AGENTS.md).

## Risks and Non-Goals

The pause delays new capabilities and may expose compatibility trade-offs in
defaults or permissions. Document those trade-offs with the affected behavior;
do not solve every case by adding configuration. Stronger Plan restrictions
may temporarily reduce available tools, which is preferable to claiming a
restriction the host cannot enforce.

This proposal does not authorize implementation, a release, paid evaluation,
or a new scheduler, policy engine, persistence framework, or model-visible
event family. It does not claim that the quick review found every defect or
that passing the proposed checks would establish production readiness.
