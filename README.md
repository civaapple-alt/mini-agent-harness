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
- A credential-free deterministic `demo` for checking the complete loop.
- Bounded workspace file tools, shell commands, web fetches, images, and MCP.
- Explicit Plan Mode and autonomous Goal Mode.
- Durable sessions with resume, fork, trace replay, and trace metrics.
- Independent, tool-free mentor insight and verification.
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
- `mini-agent-host` is the reusable application host: provider adapters,
  workspace tools, permissions, MCP and skills, sessions, Goal/Plan workflows,
  and world state. `RuntimeBuilder` composes these capabilities into a
  provider-backed `HostRuntime`.
- `mini-agent-app-server` is the service boundary over a core `Thread`. Its
  typed facade and versioned `mini-agent-app-server-protocol` support
  initialization, thread lifecycle, turn commands, steering, interruption,
  settled results, approval requests, and ordered event notifications.
  `serve_stdio` provides newline-delimited JSON-RPC framing for subprocess
  clients.
- `mini-agent-acp` is an experimental edge adapter that maps ACP-style
  `session/new`, `session/prompt`, and `session/cancel` messages to the
  app-server. It does not modify the core execution contracts or claim full
  ACP conformance yet.
- `mini-agent-cli` is the frontend: REPL input, headless commands, output
  rendering, and local session interaction. It depends on the host instead of
  owning provider and tool assembly.

The conceptual runtime direction is:

```text
CLI client
    ↓
App Server service boundary
    ↓
Host / Workflows application host
    ↓
Core / Protocol execution foundation
```

The crate dependency direction keeps the foundation independent:
`mini-agent-core → mini-agent-protocol`; `mini-agent-host` builds on core and
protocol; the app-server service depends on core, protocol, and the app-server
wire DTOs; the CLI depends on host and the foundation. A local CLI path may
invoke host/core directly for efficiency, while external clients use the
app-server boundary and observe the same Thread/Turn event semantics.

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

These commands do not need provider credentials:

```sh
mini-agent --version
mini-agent doctor
mini-agent status
mini-agent demo "make this loud"
```

`demo` runs a deterministic model → tool → model → answer flow locally.
`status` shows non-secret configuration and detected environment. `doctor`
checks startup requirements and returns a non-zero status while the provider is
not configured.

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
See [configuration](docs/configuration.md) for GLM, mentor, web search, and
extension settings.

## Common commands

```sh
mini-agent                         # interactive session
mini-agent ask "summarize this repo"
mini-agent ask --json "review the current changes"
mini-agent auto "inspect the repo and run the tests"
mini-agent sessions
mini-agent resume SESSION_ID
mini-agent fork SESSION_ID
mini-agent trace replay trace.jsonl
mini-agent trace summary trace.jsonl --json
mini-agent mentor insight SESSION_ID
mini-agent mentor verify SESSION_ID -- "tests pass and the diff is clean"
```

Use `--` before a prompt that begins with `-`. Run `mini-agent help` or
`mini-agent help <command>` for the complete command reference.

Interactive sessions are memory-only by default. Use `--persist` to save
settled checkpoints under `~/.mini-agent/sessions/`; `--ephemeral` (or
`--no-persist`) explicitly keeps a session in memory. `auto` sessions persist
by default and accept `--ephemeral`. Running processes, queued input, and
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
| Skill collection | `.agents/skillsets/` or `.agents/skillsets.json` |
| Plugin | `.agents/plugins/<plugin>/` |
| Marketplace | `.agents/marketplaces/` or `.agents/marketplaces.json` |
| MCP | `.agents/mcp.json` or `.agents/mcp/<server>.json` |

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
- [Real LLM checks](docs/real-llm-testing.md) — opt-in, budgeted provider scenarios.
- [App Server](docs/app-server.md) — versioned JSON-RPC methods and stdio usage.
- [ACP boundary](docs/acp.md) — experimental session mapping and support scope.
- [Release process](docs/releasing.md) — how to prepare and publish a release.
- [Changelog](CHANGELOG.md) — version history.
- [Agent Notes](.agents/notes/README.md) — architecture decisions and
  experiments.

## Development

Run the complete local contract before submitting Rust changes:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/line_budget.py
```

The CI matrix covers Ubuntu, macOS, and Windows. Current development is
validated on macOS arm64; Windows remains a first-class target and is checked
by CI and the Windows release build.

## License

Licensed under the [MIT License](LICENSE).
