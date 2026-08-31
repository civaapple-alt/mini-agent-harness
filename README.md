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
- Bounded workspace file tools, shell commands, web fetches, images, and MCP.
- Explicit Plan Mode and autonomous Goal Mode.
- Durable sessions with resume, fork, live events, and result continuation.
- Tool-free Goal verification against settled checkpoints.
- Native process handling on macOS, Linux, and Windows, with optional Docker
  isolation.

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
  image adapters, workspace/process/web tools, permissions, MCP and skills,
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
  turn commands, steering, interruption, settled results, approval requests,
  ordered event notifications, tool-free Goal verification turns, and local
  Session/World/MCP/Goal/Plan management through the runtime service. The App
  Server owns workflow commands; Host only supplies the wrapped
  `HostWorkflowStore` persistence seam. The
  JSON-RPC surface also exposes `workflow/state`, `workflow/plan/set`, and
   typed Goal lifecycle methods.
  `serve_stdio` provides newline-delimited JSON-RPC framing for subprocess
  clients. Runtime actions are serialized by the App Server Actor and return
  `actionId`, `actionSequence`, and `stateRevision`; Core event sequence numbers
  remain separate.
- `mini-agent-cli` is the frontend: REPL input, headless commands, output
  rendering, and local session interaction. Agent turns go through the local
  App Server runtime; the CLI does not own provider, tool, Thread, or Harness
  assembly. The App Server's `local` bootstrap adapter resolves runtime
  profiles and launch settings for embedded frontends, keeping that setup out
  of the REPL and headless command paths. The CLI imports launch, approval,
  observation, and event contracts from `mini-agent-app-server::frontend` and
  has no direct Host or Capabilities dependency.

The mainline is the CLI over the App Server boundary. Other frontends should
exercise the same App Server management and event contracts rather than add
another runtime execution path.

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
| `--no-tools` | interactive, `ask`, `auto` | Disable workspace, shell, web, image, process, and MCP tools. |
| `--security-preset PRESET` | interactive, `ask`, `auto` | Choose `default`, `turbomode`, or `full-machine`; default is `default`. |
| `--sandbox KIND` | interactive, `ask`, `auto` | Choose `native` or `docker`; default is `native`. |
| `--web-search` / `--search` | interactive, `ask`, `auto` | Enable built-in Responses `web_search`. |
| `--no-web-search` / `--no-search` | interactive, `ask`, `auto` | Disable built-in Responses `web_search`. |
| `--json` | `ask` | Emit machine-readable output. |

`ask` reads at most 32 KiB from stdin when no prompt is supplied. `auto PROMPT`
runs one autonomous turn; bare `auto` opens an interactive copilot.
`resume SESSION_ID` resumes a session directly, while `fork SESSION_ID` creates
an independent session. Goal verification is initiated by Goal Mode and is not
a standalone CLI command.

Interactive, one-shot, and auto sessions always append their settled history and
stored result handles to `~/.mini-agent/sessions/`. Running processes, queued input, and
other live effects are not resumed.

## Safety and boundaries

- File reads and writes are confined to the active workspace; `.git` is
  protected.
- Model input, output, tool calls, tool results, and process activity have
  direct hard limits. See [limits](docs/limits.md).
- Non-interactive `ask` fails closed for sensitive tools unless
  `--auto-approve` (or `-y`) is explicitly supplied.
- `--sandbox docker` provides container isolation when Docker is available.
  Native process handling prevents orphaned process trees, but shell execution
  is not itself a security boundary.
- There is no telemetry, update check, or crash-report service.

Plan Mode (`/plan`) locks workspace mutations while keeping a living session
plan. Goal Mode (`/goal <objective>`) tracks milestones and can require an
independent verifier before advancing.

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
- [Harness iteration notes](.agents/notes/implemented/architecture/2026-08-31-vscode-harness-lessons-next-iteration.md) — VS Code harness lessons, bounded scenarios, and
  six-question validation records.

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

The current hard-budget snapshot is runtime `16,074 / 20,000` lines and all
Rust source `29,369 / 30,000` lines. The approximate `26,900` Stage 1 target
is currently exceeded and remains optimization debt rather than a reason to
delete protected behavior.

The first bounded harness scenario baseline is active: 8 representative CLI
scenarios pass, with App Server `28/28` and CLI interactive `13/13` regression
coverage. Changes that affect prompt, tool schema, loop-control, context,
events, or persistence must add scenario/eval evidence beyond unit tests.

The Stage 2 boundary evidence also includes a test-only fault-injection model
and Responses parser/provider cases for malformed or missing tool arguments, partial
model streams, retryable tool results, bounded HTTP 429 API error classification without
implicit retry, MCP connection refusal, and shell refusal before sandbox execution. Provider-
level retry/backoff policy and Docker sandbox availability/isolation remain open; CLI public-
path unknown-tool recovery, MCP connection/call refusal, and a bounded cross-file refactor are
now covered. Broader failure/retry matrices remain open follow-ups. The App Server public boundary also verifies
that `NeedsApproval` results keep a non-empty reason in events, checkpoints,
and the next model round.

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

The interactive target contains the 8 baseline scenarios plus 3 public CLI
regressions. The App Server also exposes `JsonlTrace` for local callers; it
writes bounded, redacted per-round JSONL with input, tool-manifest, and payload
hashes. The shortcut above currently captures test output and the budget
snapshot only; automatic CLI Trace wiring remains a tracked next-iteration task.
Do not use a paid provider for this baseline.

The line-budget report breaks Rust source down by the runtime layers: `core`,
  `protocol`, `capabilities`, `host`, `app-server`, and `cli`, followed by the enforced
  workspace total. `capabilities` is the separately reported provider
  implementation group behind Host and is not part of the runtime-layer gate.
  Each layer and the workspace total also show `production`, `unit`, and
  `integration` lines:
  inline `#[cfg(test)]` modules and `*_tests.rs` files are counted as unit
  tests, while Rust files below a `tests/` directory are counted as
  integration tests. The enforced ceilings are 20,000 lines for the runtime
  layers (`core` + `protocol` + `host` + `app-server`). The 30,000-line
  workspace total is enforced for
  the 0.5.0 release, including tests. Both ceilings block the release gate.
  The report still includes all Rust source, including the CLI, so cleanup
  remains measurable.

The CI matrix covers Ubuntu, macOS, and Windows. Current development is
validated on macOS arm64; Windows remains a first-class target and is checked
by CI and the Windows release build.

## License

Licensed under the [MIT License](LICENSE).
