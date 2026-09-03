# Mini Agent Harness

Mini Agent Harness is a small native coding-agent harness for studying how
models, tools, limits, failures, and observations work together. The command
line program is `mini-agent`.

It is intentionally focused. It is not a full copy of Codex, Pi, fx, or Qi,
and it does not promise feature parity with them.

Project reference: [GitHub](https://github.com/civaapple-alt/mini-agent-harness).
Created by [civaapple-alt](https://github.com/civaapple-alt) and released under
the [MIT License](LICENSE).

## What you get

- Interactive agent sessions and one-shot `ask` commands.
- An autonomous `auto` mode with bounded context compaction.
- Four bounded default Builtin tools: `read_file`, `apply_patch`, `shell`, and
  `read_image`; `write_file`, `edit_file`, and `web_fetch` remain opt-in
  compatibility tools, while MCP remains an explicit extension.
- Active Threads can select a bounded Builtin subset through
  `thread/settings/update` `builtinTools`; omission preserves the current
  selection and external/MCP tools remain separate.
- App Server workflow APIs for Plan Mode and autonomous Goal Mode.
- Durable sessions with resume, fork, live events, and bounded result artifacts.
- Tool-free Goal verification against settled checkpoints.
- Managed-process tools and result-continuation tools are not part of the
  Builtin catalog; large outputs remain bounded internal artifacts.

The design boundary is simple:

```text
agent = model + harness
```

The model proposes an answer or action. The harness owns context, tools,
limits, failures, and observation events.

## Crate layers

- `mini-agent-protocol` defines the in-process contracts for models, tools,
  messages, events, stop reasons, and limits.
- `mini-agent-core` implements the execution kernel: context preparation,
  model/tool steps, compaction, hard limits, and cooperative run control.
- `mini-agent-capabilities` owns concrete provider implementations: model and
  image adapters, workspace/web tools, permissions, MCP and skills,
  sessions, and Result Store. `mini-agent-host` is the reusable application
  host for profile resolution, context/workflow composition, and runtime
  assembly; `HostRuntimeFactory` composes selected capabilities into a
  provider-backed `HostRuntime`. Capabilities keeps its implementation modules
  private and exposes a root facade; embedders can register an external tool
  provider with `CapabilityRegistry::with_tool_provider`; see
  `crates/mini-agent-capabilities/examples/external_tool_provider.rs`.
- `mini-agent-app-server` is the service boundary over a core `Thread`. Its
  host-backed `AppServerRuntime`, typed facade, and versioned
  `mini-agent-app-server-protocol` support initialization, thread lifecycle,
  turn commands, steering, interruption, settled results, approval request and
  resolution notifications, ordered event notifications, tool-free Goal
  verification turns, Goal settled-checkpoint continuation, and local
  Session/World/MCP/Goal/Plan management through the runtime service. The App
  Server owns workflow commands; Host only supplies the wrapped
  `HostWorkflowStore` persistence seam. The
  JSON-RPC surface exposes `thread/settings/update` and its
  `thread/settings/updated` notification with the typed `collaborationMode`
  and bounded `builtinTools` settings, `workflow/state` as a read-only
  aggregate, and the canonical typed Goal lifecycle methods. The former
  `workflow/plan/set` and manual `workflow/goal/*` controls are intentionally
  removed; clients must use the settings and `thread/goal/*` methods. Active
  Goals bind relative `goal/...` tool paths to the session Goal workspace, and
  Goal objectives are capped at 8 KiB.
  `serve_stdio` provides newline-delimited JSON-RPC framing for subprocess
  clients. Runtime actions are serialized by the App Server Actor and return
  `actionId`, `actionSequence`, and `stateRevision`; Core event sequence numbers
  remain separate.
- `mini-agent-cli` is the frontend: REPL input, headless commands, output
  rendering, and local session interaction. Agent turns go through the local
  App Server runtime; the CLI does not own provider, tool, Thread, or Harness
  assembly. The REPL is intentionally a core-capability reference client for
  turns, streaming events, approval, `/steer`, startup-selected manual/auto
  execution, and session persistence/resume entry points; full Plan/Goal
  workflow and management presentation, including session metadata inspection,
  belongs to App Server clients such as Studio. The REPL intentionally does not
  provide `/help`, `/queue`, `/new`, runtime mode toggling, or duplicate
  `/status`, `/info`, and `/session` management displays. The App Server's
  `local` bootstrap adapter resolves
  runtime profiles and launch
  settings for embedded frontends, keeping that setup out of the REPL and
  headless command paths. The CLI imports launch, approval, and event
  contracts from `mini-agent-app-server::frontend`; its terminal observer is
  an experimental CLI-owned edge and is outside the runtime line gate.

The mainline is the CLI over the App Server boundary. Other frontends should
exercise the same App Server management and event contracts rather than add
another runtime execution path.

Approval is currently a distributed Core/Host boundary. Core `ToolRouter` resolves
by name and dispatches through the protocol-level `ToolExecutionDelegate`;
`ToolHandler` owns tool-specific schema, argument parsing, and admission
description; Host `ToolOrchestrator` owns approval and lifecycle order; and
`ToolRuntime` owns the concrete side effect. The `Tool` trait composes Handler and
Runtime for the existing registry/provider/delegate boundary. Typed admission
covers Shell, EditFile, WriteFile, MCP calls, and outside-workspace ReadImage
paths, while read-only tools remain on the Legacy path. Sandbox policy
is selected by the Host profile/Capabilities assembly and applied by the concrete
runtime; it is not duplicated in the orchestrator. MCP server startup approval
remains a separate Host assembly gate. App Server transports approval notifications
and
persists settled results. The public Shell path correlates `requestId`, `turnId`,
and `callId` from `turn/start` through `approval/request`, `approval/respond`,
`approval/resolved`, and `turn/event`. Approval remains synchronous internally;
the App Server worker isolates that wait from the connection runtime. Typed admission
now covers the approval-gated model tool calls; read-only tools retain the legacy path.
In Plan Mode, Shell admits only a conservative bounded set of read-only inspection
commands (including the normal PowerShell listing pipeline); those calls still use
the approval path, while mutation syntax is rejected before approval and execution.
On Windows, keep these commands to simple cmdlet pipelines such as
`Get-ChildItem | Select-Object Name`; script blocks (`Where-Object { ... }`), variables,
subexpressions, redirection, process/build commands, and side-effect flags such as
`git branch -D`, `fd --exec`, and `rg --pre` remain blocked.

Runtime state has one authority: the App Server Runtime Actor orders Thread,
World, Workflow, MCP, Session, and revision changes. Host implements the
capability and persistence seams, while CLI and JSON-RPC clients submit actions
and consume results and events.

The conceptual runtime direction is:

```text
CLI client
    ↓
App Server service boundary
    ↓
Host runtime and persistence seams
    ↓
Core / Protocol execution foundation
```

## Built-in prompt templates

Stable built-in prompt bodies are kept as crate-owned UTF-8 Markdown assets and
embedded at compile time with `include_str!`. The current sources are:

- `crates/mini-agent-core/builtin/prompts/system/`: the default system prompt
  and bounded compaction instruction;
- `crates/mini-agent-capabilities/builtin/prompts/agents/`: `explore`, `plan`,
  and `general` foundational agent contracts;
- `crates/mini-agent-capabilities/builtin/prompts/personas/`: the currently
  enabled `reviewer`, `implementer`, and `researcher` persona contracts;
- `crates/mini-agent-app-server/builtin/prompts/system/`: the independent Goal
  verifier instruction.

The Host still composes these bounded built-ins with project `AGENTS.md`,
extension/skill metadata, world state, and workflow instructions. Those dynamic
sources are not copied into the template files and remain subject to the
existing context limits. App Server startup may select an allowlisted profile,
which can select an agent/persona through the workspace profile. Its internal
local `ThreadUpdate::ReplaceConfig` seam remains available to the CLI, but the
public JSON-RPC API does not accept arbitrary system-prompt text.

The crate dependency direction keeps the foundation independent:
`mini-agent-core → mini-agent-protocol`; `mini-agent-host` builds on core and
protocol; the app-server service depends on host, core, protocol, and the
app-server wire DTOs; the CLI depends on the app-server service for agent turns
and keeps only frontend concerns. JSON-RPC clients observe the same
Thread/Turn event semantics.

## Install

### Download a release

Download the archive and matching `.sha256` file from
[GitHub Releases](https://github.com/civaapple-alt/mini-agent-harness/releases).
Verify the checksum, extract the archive, and put `mini-agent` (or
`mini-agent.exe`) on `PATH`.

Release archives are available for:

- Linux x86_64
- macOS x86_64
- macOS arm64
- Windows x86_64

Each archive contains the binary, `README.md`, `LICENSE`, and `CHANGELOG.md`.

### Build from source

Install Rust 1.88 or newer:

```sh
cargo build --release --locked -p mini-agent-cli
./target/release/mini-agent --version
```

On Windows, run `target\release\mini-agent.exe`. Shell tools use `sh` on
Unix-like systems and require PowerShell 7 (`pwsh`) on Windows.

## Quick start

Check the installed binary first:

```sh
mini-agent --version
```

To use `ask`, the interactive terminal, or `auto`, configure a provider. The
recommended location is the user file below, which keeps credentials out of
project directories:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

Use `~/.mini-agent/.env` on macOS/Linux and
`%USERPROFILE%\.mini-agent\.env` on Windows. Process environment values take
precedence over a workspace `.env`, which takes precedence over the user file.
See [configuration](docs/configuration.md) for provider, Goal verifier, web search, and
extension settings.

## Common commands

```sh
mini-agent                         # interactive session
mini-agent ask "summarize this repo"
mini-agent ask --json "review the current changes"
mini-agent ask --trace-jsonl .agents/scratch/trace.jsonl "review the current changes"
mini-agent auto "inspect the repo and run the tests"
mini-agent resume SESSION_ID
mini-agent fork SESSION_ID
```

Use `--` before a prompt that begins with `-`. Run `mini-agent help` or
`mini-agent help <command>` for the complete command reference.

## Arguments

Turn commands accept the following options:

| Option | Applies to | Meaning |
| --- | --- | --- |
| `--session-id ID` (also `--session ID`) | interactive, `ask`, `auto` | Resume a durable session instead of opening a new one. |
| `--auto-approve`, `-y` (also `--yes`, `--auto`) | `ask` | Allow sensitive tools without an interactive approval prompt. |
| `--max-steps N` | `ask` | Limit model steps; default is 8 for `ask`, and `0` means unlimited. |
| `--no-tools` | interactive, `ask`, `auto` | Disable all Builtin and extension tools. |
| `--security-preset PRESET` | interactive, `ask`, `auto` | Choose `default`, `turbomode`, or `full-machine`; default is `default`. |
| `--sandbox KIND` | interactive, `ask`, `auto` | Choose `native` or `docker`; default is `native`. |
| `--web-search` / `--search` | interactive, `ask`, `auto` | Enable built-in Responses `web_search`. |
| `--no-web-search` / `--no-search` | interactive, `ask`, `auto` | Disable built-in Responses `web_search`. |
| `--json` | `ask` | Emit machine-readable output. |
| `--trace-jsonl PATH` | `ask` | Write an opt-in bounded, redacted event trace; `PATH` must not already exist. |

`ask` reads at most 32 KiB from stdin when no prompt is supplied. `auto PROMPT`
runs one autonomous turn; bare `auto` opens an interactive copilot.
`resume SESSION_ID` resumes a session directly, while `fork SESSION_ID` creates
an independent session. Goal verification is initiated automatically by the
App Server `thread/goal/set|get|clear` contract and is not a standalone REPL
command.

`--trace-jsonl PATH` is an explicit one-shot diagnostic artifact. The parent directory
must already exist, the file is created without overwrite, each record is capped at
8 KiB, and the complete JSONL artifact is capped at 256 KiB. It contains event
metadata, counts, and hashes only; prompt, tool arguments/results, and Session history
are not copied. A trace write or finalization error fails the command.

Interactive, one-shot, and auto sessions always append their settled history and
bounded result artifacts to `~/.mini-agent/sessions/`. Running processes, queued input, and
other live effects are not resumed.

## Safety and boundaries

- File reads and writes are confined to the active workspace; `.git` is
  protected.
- Model input, output, tool calls, tool results, and shell activity have
  direct hard limits. See [limits](docs/limits.md).
- Non-interactive `ask` fails closed for sensitive tools unless
  `--auto-approve` (or `-y`) is explicitly supplied.
- `--sandbox docker` provides bounded container execution with the workspace
  mounted at `/workspace` when Docker is available. The current contract does
  not claim complete network, capability, or resource isolation. Shell execution
  is not itself a security boundary. Stronger Docker restrictions are policy-gated; see
  [SECURITY.md](SECURITY.md) and the [Docker isolation policy proposal](.agents/notes/proposed/architecture/2026-08-31-docker-sandbox-isolation-policy.md)
  for the required threat model, compatibility, failure, and cross-platform
  evidence before adding them.
- There is no telemetry, update check, or crash-report service.

Plan and Goal are App Server workflow contracts: Plan Mode locks workspace
mutations while keeping a living session plan, and Goal Mode tracks milestones
with an optional independent verifier. The REPL deliberately does not expose
`/plan` or `/goal`; use an App Server client such as Studio or the SDK for the
full workflow surface.

## Extensions

Project-scoped extensions live under `.agents/`:

| Extension | Location |
| --- | --- |
| Agent Skill | `.agents/skills/<skill>/SKILL.md` |
| Plugin | `.agents/plugins/<plugin>/` |
| MCP | `.agents/mcp/<server>.json` |

Discovery is bounded and approval-aware. Client-specific UI, hooks, LSP, and
nested-agent behavior are not emulated. See the
[extension examples](examples/extensions/README.md) and
[configuration](docs/configuration.md).

Skill frontmatter may declare bounded tool dependencies under
`dependencies.tools` using `type: builtin|mcp`. Discovery and explicit local
activation expose only this metadata; provider resolution, startup, approval,
and execution remain Host-owned and are not triggered by Skill discovery.
Selecting a validated Plugin name retains that Plugin's MCP provider inputs,
but does not start a server or create a Plugin-specific execution path.
App Server `turn/event` and `turn/read` also expose bounded ThreadItems derived
from existing Core events/messages; these items are a projection, not a second
history store. Tool items reuse `callId`, carry the same bounded/redacted
arguments through started and completed states, and keep bounded output in the
existing event/read projection. Core, Goal, and settings notifications for a
runtime connection share one ordered App Server notification stream.

## Documentation

- [Configuration](docs/configuration.md) — providers, sessions, extensions,
  and environment variables.
- [Harness limits](docs/limits.md) — default byte, count, and timeout bounds.
- [Troubleshooting](docs/troubleshooting.md) — common setup and runtime issues.
- [Security policy](SECURITY.md) — reporting security problems.
- [Privacy](docs/privacy.md) — local data and provider requests.
- [App Server](docs/app-server.md) — versioned JSON-RPC methods and stdio usage.
- [Release process](docs/releasing.md) — how to prepare and publish a release.
- [Changelog](CHANGELOG.md) — version history.
- [Agent Notes](.agents/notes/README.md) — architecture decisions and
  experiments.
- [Agent instructions](AGENTS.md) — change admission, boundary, and
  verification rules for repository work.
- [Change admission checklist](.github/pull_request_template.md) — required
  architecture, line-budget, and boundary questions for every change.
- [Harness framework and next-iteration note](.agents/notes/implemented/architecture/2026-08-31-vscode-harness-lessons-next-iteration.md) — mini/Codex
  framework and Turn-flow comparison, VS Code harness lessons, bounded scenarios,
  and six-question validation records.
- [Goal Runtime implementation appendix](.agents/notes/proposed/architecture/2026-09-01-goal-runtime-thread-goal-plan.md) —
  Codex-shaped `thread/goal/*`, serialized GoalRuntime ownership,
  settled-checkpoint continuation, settings notifications, resume/clear race
  handling, and manual-API retirement are implemented.
- [Codex-aligned capabilities and ThreadItems proposal](.agents/notes/proposed/architecture/2026-09-01-codex-aligned-capabilities-thread-items.md) — canonical
  Skill/Plugin/Builtin/Host/MCP/Dynamic Tool boundaries, `Thread` → `Turn` →
  `ThreadItem` projection, Goal Runtime integration, approval correlation, and
  sidecar Artifact references.

## Development

For normal Rust changes, run the affected package contract before submitting:

```sh
cargo fmt --all
cargo clippy -p <affected-package> --all-targets -- -D warnings
cargo test -p <affected-package>
python3 scripts/line_budget.py
```

For release or explicitly approved full-workspace validation, also run:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Every change must also answer the six questions in the
[change admission checklist](.github/pull_request_template.md): ownership
layer, duplicate responsibility, replace-vs-add reasoning, expected and actual
net line delta, model-visible/event/persistence/protocol impact, and boundary
test evidence. New code defaults to net-zero growth or must name an explicit
offset. Do not remove Core tests or Actor/CAS/Session authority merely to fit a
line target.

Pull-request CI also checks that the six-question section is present, all six
questions have answers, placeholders are replaced, and each of the six designated
admission confirmations is checked exactly once; reviewers remain responsible for
answer quality.

The current hard-budget snapshot is runtime `18,614 / 20,000` lines and release
Rust source `28,470 / 30,000` lines, excluding the experimental CLI/REPL. The
CLI is still reported separately for visibility. The approximate `26,900` Stage
1 target is now within the enforced release-source total and remains an
optimization reference rather than a reason to delete protected behavior.

The Goal Runtime is now aligned to the Codex Thread model: the canonical
`thread/goal/set|get|clear` control plane, serialized App Server owner,
settled-checkpoint verifier/continuation, settings notifications, resume/clear
race guards, and retirement of manual Goal controls are implemented. Follow the
[Goal Runtime implementation appendix](.agents/notes/proposed/architecture/2026-09-01-goal-runtime-thread-goal-plan.md)
for the state machine and boundary evidence. Pending verifier results are
checked against their original settled checkpoint, and preparation failures
become durable failed Goal states instead of leaving a Goal stuck.

Goal execution now applies the persisted milestone step and timeout limits at
the existing Core/App Server turn boundary. Timeout is cooperative and waits
for safe turn settlement; provider-reported input/output usage is accumulated
and stops a Goal at its token budget with `budgetLimited`. Step and timeout
exhaustion use `usageLimited` and retain a bounded reason.

GoalRuntime also has direct fault-injection evidence for rejected verdicts,
verifier execution errors, and late-result disposal. The public App Server path
asserts the bounded `active(turnId) -> blocked(turnId)` Goal notification
sequence. A fresh App Server rebind public scenario verifies that a settled
Goal resumes through verifier preparation without replaying its main turn. A
real provider-backed verifier remains optional follow-up evidence. Verifier
input keeps only the newest bounded history window; it does not create a second
conversation source. Cross-stream Core/Goal/settings ordering is now emitted
through one runtime notification bus, with a public settings-before-Goal
ordering scenario. This is transport-level ordering; no new global wire
sequence or durable-write receipt is exposed.

The current ThreadItem lifecycle scope is the bounded projection in
`turn/event` and `turn/read`: stable `callId`, started/completed status, bounded
output, and bounded/redacted `arguments`. Dedicated `item/started`,
`item/completed`, Item listing, and persisted Item replay remain deferred. The
raw `ModelResponded` event retains its existing event payload; the redaction
guarantee applies to the ThreadItem projection.

The first bounded harness scenario baseline is active: 8 representative CLI
scenarios pass, with current App Server boundary evidence and CLI interactive
`12/12` regression coverage. Changes that affect prompt, tool schema, loop-control, context,
events, or persistence must add scenario/eval evidence beyond unit tests.

The Stage 2 boundary evidence also includes a test-only fault-injection model
and Responses parser/provider cases for malformed or missing tool arguments, partial
model streams, retryable tool results, bounded HTTP 429 API error classification without
implicit retry, MCP connection refusal, and shell refusal before sandbox execution. Provider-
level retry/backoff policy remains deferred; the current default is one bounded fail-fast
429 failure without implicit retry. Docker sandbox availability is now verified on this host:
Docker CLI/server 29.6.1 and the `alpine` image are available, and a runtime probe verifies the
`/workspace` mount plus container-only temporary files. This is not complete network, capability,
or resource-isolation proof; the current command still needs an explicit security-policy decision
before stronger claims or flags are added. A current-host feasibility probe accepted the candidate
strict flags, verified strict `/workspace` mounting, and observed read-only root, writable `/tmp`,
zero effective capabilities, no routes, and bounded cgroup values; this is not cross-platform
acceptance. The failure/
timeout/retry evidence matrix distinguishes covered public paths from unit-only and deferred
evidence; MCP timeout is covered at the capability boundary and its App Server public
projection, while the CLI public MCP transport projection remains open. CLI public-
path unknown-tool recovery, MCP connection/call refusal, and a bounded cross-file refactor are
now covered. Broader failure/retry matrices remain open follow-ups. The App Server public boundary also verifies
that `NeedsApproval` and MCP timeout results keep a non-empty reason in events,
checkpoints, and the next model round.

The next iteration is evidence-triggered rather than another broad cleanup:

1. Freeze the two hard ceilings and require the six admission answers for every
   batch; no new Rust feature starts without a net-zero plan or an explicit offset.
2. The bounded opt-in CLI Trace contract is implemented: `ask --trace-jsonl PATH`
   creates a new redacted JSONL artifact with per-record and total limits; its
   public success, redaction, and overwrite-failure scenarios are covered. The
   baseline recipe remains explicit and does not create trace files implicitly.
3. Preserve the completed typed-admission set and App Server public approval
   correlation path. New sensitive tools require the same six-question record and
   a net-zero plan or explicit line-budget offset; read-only tools stay legacy.
4. Revisit CLI public MCP-timeout projection only when a bounded fault-injection
   seam exists; otherwise keep the capability/App Server evidence and mark the
   CLI transport gap deferred.
5. The measurable compaction-trigger scenario is now recorded in Core: it checks
   a trigger at least 70% of the bounded fixture budget and recent-tail retention;
   production remains at the documented 50% trigger unless later evidence proves
   that threshold unsuitable.
6. Docker availability and both current and strict workspace-mount evidence are now recorded. Review the
   [Docker isolation policy proposal](.agents/notes/proposed/architecture/2026-08-31-docker-sandbox-isolation-policy.md)
   and accept a threat model, supported platforms, defaults/opt-outs,
   compatibility, and fail-closed behavior before implementing stronger isolation.
7. Consider provider comparison or retry/backoff only after a second provider or
   an explicit retry policy exists; do not use paid-provider CI as a default.

Each batch stays within a few hundred changed lines, runs affected tests and
Clippy plus `python scripts/line_budget.py`, updates the relevant note and
CHANGELOG, and lands as a separate commit. A missing bounded seam or evidence
stops that item rather than triggering speculative plumbing.

To rerun the baseline locally from the repository root (the first run may spend
time compiling; warm-cache runs are intended to stay within a few minutes),
capture a human-readable comparison report with:

```powershell
$report = ".agents/scratch/harness-baseline-$((Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ')).txt"
New-Item -ItemType Directory -Force .agents/scratch | Out-Null
cargo test -p mini-agent-cli --test interactive -- --test-threads=1 2>&1 |
  Tee-Object -FilePath $report
python scripts/line_budget.py 2>&1 | Tee-Object -FilePath $report -Append
```

The interactive target contains the 8 baseline scenarios plus 7 public CLI
regressions. The App Server exposes `JsonlTrace` for local callers, and `ask`
now offers explicit `--trace-jsonl PATH` export with the same bounded, redacted
per-round records. The shortcut above captures test output and the budget
snapshot only; it does not create an implicit trace artifact.
Do not use a paid provider for this baseline. The built-in registry currently
exposes one OpenAI-compatible provider; the external model factory is a Host
composition seam, not cross-provider quality evidence.

The line-budget report breaks Rust source down by the runtime layers: `core`,
  `protocol`, `capabilities`, `host`, `app-server`, and `cli`, followed by the enforced
  release-source total. `capabilities` is the separately reported provider
  implementation group behind Host and is not part of the runtime-layer gate.
  The experimental `cli`/REPL layer is informational and is excluded from the
  release-source total.
  Each layer and the release-source total also show `production`, `unit`, and
  `integration` lines:
  inline `#[cfg(test)]` modules and `*_tests.rs` files are counted as unit
  tests, while Rust files below a `tests/` directory are counted as
  integration tests. The enforced ceilings are 20,000 lines for the runtime
  layers (`core` + `protocol` + `host` + `app-server`). The 30,000-line
  release-source total is enforced for the 0.7.0 release, including tests in
  supported packages. Both ceilings block the release gate; experimental
  CLI/REPL lines remain visible but do not block the release.

The CI matrix covers Ubuntu, macOS, and Windows. Current development is
validated on macOS arm64; Windows remains a first-class target and is checked
by CI and the Windows release build.

## License

Licensed under the [MIT License](LICENSE).
