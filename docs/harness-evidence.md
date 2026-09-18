# Harness scenarios and evidence

Use a bounded scenario when a change affects model-visible input, tool schema,
loop control, context, events, Session persistence, approval, or recovery. A
unit test can prove a local function. A scenario proves the ordered behavior
across the public boundary that a client or model actually uses.

## What to record

Each scenario must state the following facts:

| Field | Required evidence |
| --- | --- |
| Hypothesis | The behavior that the scenario proves or disproves. |
| Public path | A CLI command, App Server method, SDK call, or Gateway route. |
| Setup | The temporary workspace, Session state, tools, policy, and cleanup rule. |
| Stimulus | Input, fault injection, or deterministic race ordering. |
| Trace | Relevant Thread, Turn, item, event, action, and checkpoint order. |
| Settlement | The final Session, filesystem, operation, or Goal state. |
| Failure case | An input or schedule that must be rejected or produce a bounded failure. |
| Command | A local command that reruns the evidence without a paid provider call. |
| Gap | What the scenario does not prove. |

Do not claim that a mock provider proves provider quality, that one platform
proves sandbox behavior on every platform, or that a successful retry proves an
interrupted side effect is replay-safe.

## Required boundaries

The relevant scenario set must cover both successful and rejected behavior for
the changed boundary:

| Boundary | Evidence to retain |
| --- | --- |
| Tool admission | Deny precedes approval and execution. A denied action produces a bounded model-visible outcome. |
| Approval | A grant matches the structured action key and becomes invalid when its owner or workspace revision changes. |
| Cancellation and steering | The runtime settles at a safe boundary and persists a checkpoint before later work resumes. |
| Context and compaction | UTF-8-safe bounded input and output remain below the configured request limit. |
| Session and fork | Resume uses a settled checkpoint. A child Session does not copy in-flight work or create a second history authority. |
| Child operations | Queue, retry, recovery, and cancellation preserve one operation identity and do not start duplicate child Turns. |
| Notebook | A child can read the admitted parent snapshot but cannot mutate it. Resume restores only bounded context. |
| Gateway and Web Studio | A reconnect gap triggers canonical Thread/item reconciliation rather than a fabricated event history. |

## Run evidence

For a Rust change, run the affected package checks before recording the scenario:

```sh
cargo fmt --all
cargo clippy -p <affected-package> --all-targets -- -D warnings
cargo test -p <affected-package>
python scripts/line_budget.py
```

For a cross-repository change, add the matching SDK, Gateway, and Studio test
or fixture. The default evidence path must not call a paid provider. Use a mock
model, deterministic fixture, or local test server instead.

## Historical evidence

Frozen batch reports and dated test inventories live in `.agents/notes/`. They
explain why a scenario was added. This document defines the current evidence
standard and must change only when that standard changes.
