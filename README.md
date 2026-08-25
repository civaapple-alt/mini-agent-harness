# Mini Agent Harness

Mini Agent Harness is a small native agent harness for studying why some
harnesses help a model and others get in its way. Its command is `mini-agent`.

Version 0.1 is the first supported release contract: a native interactive CLI,
a script-facing `ask` command, bounded workspace tools, deterministic tests,
diagnostics, and reproducible release archives. It intentionally does not claim
feature parity with Codex, Pi, fx, or Qi.

The working definition is deliberately narrow:

```text
agent = model + harness
```

The model proposes **what** to say or do. The harness decides **how** context,
tools, limits, failures, and observations are handled. The boundary is useful
only while both sides remain easy to read and change.

## Constraints

- `mini-agent-core` stays between 10,000 and 20,000 Rust source lines when it
  becomes feature-complete. Its hard ceiling is 20,000 lines.
- All Rust source, including tests and binaries, has a hard ceiling of 30,000
  lines.
- The terminal is a stream of conversation and tool activity, not a screenful
  of permanent UI state.
- Defaults are the product. Configuration is added only when two useful
  behaviors genuinely need to coexist.
- A feature enters core only when it changes the model/harness experiment. Host
  integration belongs at the edge.
- User input, model output, tool cardinality, tool results, model steps, and
  total request context all have direct hard bounds.

Run `python scripts/line_budget.py` to enforce the ceilings.

## Architecture

The workspace starts with two crates:

- `mini-agent-core` owns the small contracts and the explicit agent loop.
- `mini-agent-cli` owns terminal presentation and provider/tool adapters.

Core intentionally has no provider, filesystem, process, MCP, plugin, session,
or TUI framework. The harness loop is concrete rather than hidden behind a
policy framework. Experiments should change the loop, record its events, and
compare outcomes before extracting another abstraction.

Pi v2's durable harness design is an important reference, but not the default
scope. Mini Agent Harness keeps only three lessons at the foundation:

1. external effects have visible prepare, execute, and settle boundaries;
2. the current run state is explicit rather than inferred from missing data;
3. passive events observe execution but cannot alter it.

Durable storage, conversation trees, lanes, queues, hooks, schema migration,
and crash recovery are separate experiments. None enters core until an
experiment demonstrates that its benefit justifies its permanent cost. The
explicit auto mode is that experiment for context compaction: it summarizes a
settled conversation before the next sampling request when history reaches
half of the hard context ceiling.

The deterministic demo proves the complete model -> tool -> model -> answer
path without network or credentials. The default command opens a small
multi-turn terminal using a streaming OpenAI Responses adapter at the CLI
edge. Reasoning and final-answer deltas use separate `thinking>` and
`assistant>` lanes. Terminal tags use distinct colors; redirected output and
`NO_COLOR` sessions stay plain. Input entered while a turn is running is
accepted into a bounded FIFO queue; `/queue` reports how much work is pending.
History and queued work live only for the life of the process, and `/new`
clears history.

## Install

Prebuilt archives are produced for Linux x86_64, macOS x86_64 and arm64, and
Windows x86_64. Download the archive and matching `.sha256` file from the
repository's Releases page, verify the checksum, extract it, and place
`mini-agent` (or `mini-agent.exe`) on `PATH`.

To build from source, install Rust 1.88 or newer and run:

```sh
cargo build --release --locked -p mini-agent-cli
./target/release/mini-agent --version
```

On Windows, use `target\release\mini-agent.exe`. Runtime shell tools require
PowerShell 7 (`pwsh`) on Windows and `sh` on Unix systems.

## Quick start

Copy `.env.demo` to `.env` and fill in `OPENAI_API_KEY`. The demo defaults to
DeepSeek's Responses API with the `deepseek-v4-flash` model. Process environment
values take precedence over `.env`. This repository ignores `.env`; verify the
same before using it in another
workspace, and prefer process secrets in CI.

```sh
mini-agent doctor
mini-agent status
mini-agent
mini-agent ask "summarize this repository"
mini-agent ask --json "summarize the current changes"
mini-agent help ask
mini-agent auto "inspect this repository, improve it, and run the tests"
mini-agent --trace trace.jsonl
```

Use `--` before a prompt that begins with `-`, for example
`mini-agent ask -- --explain-this`.

`ask` streams reasoning/tool progress to stderr. In a terminal, assistant text
streams to stdout under one colored `assistant>` tag and is not repeated at the
end. Redirected stdout and `--json` hold the assistant answer until completion
so machine output remains valid. It also accepts a bounded prompt from stdin.
`ask --json` emits one JSON object containing output, exit code, model, steps,
usage, and tool-call statuses. Noninteractive sensitive tool calls fail closed;
`ask --auto` permits them without approval and should be used only in a trusted
or disposable execution environment.

The deterministic provider-free path remains available to verify the complete
model → tool → model loop:

```sh
mini-agent demo "make this loud"
```

The real modes expose `read_file`, `edit_file`, `write_file`, `shell`,
`read_tool_result`, and managed-process tools. Reads and direct file writes are
confined to the current workspace; `.git` is protected. `edit_file` makes one
exact unique replacement; `write_file` creates new files and refuses to replace
existing ones. Interactive and `run` modes ask before writes, shell commands,
and process starts. `auto` without a prompt starts an interactive auto session;
`/auto` enables automatic execution in any interactive session and `/auto off`
restores per-action approval. `auto` with a prompt remains a one-shot copilot.
Auto mode runs up to 128 model steps, performs effects without per-step
approval, and compacts context between settled tool batches so work can
continue. It prints a warning because process execution is not sandboxed and
can escape the workspace even though direct file tools cannot.

Foreground `shell` commands have a 120-second deadline and bounded concurrent
stdout/stderr capture; timeout terminates the process tree. A large result is
returned as a short head/tail preview plus a process-local handle that
`read_tool_result` can inspect by byte range or literal query. Long-lived
services should use `process_start`, `process_read`, `process_list`, and
`process_stop`; their logs and process count are bounded, and remaining process
trees are stopped when the CLI exits. Conversation, result handles, queued
input, and managed processes deliberately remain non-durable: auto mode
survives long context growth, not process termination.

## Skills, plugins, marketplaces, and MCP

Mini Agent Harness provides one bounded, project-scoped compatibility layer:

| Capability | Project location | Supported formats |
| --- | --- | --- |
| installed skill | `.agents/skills/<skill>/SKILL.md` | Agent Skills, standards-strict |
| cloned skill collection | `.agents/skillsets/<repo>/` | root `SKILL.md` or immediate `skills/*/SKILL.md` |
| installed plugin | `.agents/plugins/<plugin>/` | Agent Plugins v1, Claude plugin, or Grok plugin |
| cloned marketplace | `.agents/marketplaces/<repo>/` | Claude or Grok marketplace, selected by `.agents/marketplaces.json` |
| standalone MCP | `.agents/mcp.json` or `.agents/mcp/<server>.json` | aggregate or one-server configuration |

[Agent Plugins 1.0.0](https://agent-plugins.org/) use root `plugin.json`,
`skills/`, and `mcp.json`. Claude and Grok plugins use their native hidden
manifest, `skills/`, compatible `agents/*.md` instructions, and `.mcp.json`.
Client-specific commands, hooks, LSP, UI, and nested-agent execution are not
emulated. A compatible plugin agent becomes on-demand instructions for the
main harness and is labeled `plugin-agent` in model-visible metadata.

Marketplace repositories are inert until local selectors are named in
`.agents/marketplaces.json`. A selector first matches an immediate
`skills/<name>/SKILL.md`, allowing one skill to be enabled from a collection;
otherwise it matches a marketplace plugin entry. Mini-agent resolves string
sources such as `./plugins/name` and Grok `{ "type": "local", "path": "..." }`
sources, including marketplace entries with explicit `skills` arrays. It
deliberately does not download remote `url` or `git-subdir` entries.

Discovery injects only each instruction's name, description, kind, and
workspace-relative location into the stable system prompt. The model reads the
complete file only when the task matches, preserving progressive disclosure.
Skill collections and legacy ecosystems accept real-world install names that
differ from source folder names; direct skills and portable plugins retain the
stricter Agent Skills rule.

MCP supports `stdio` plus modern `http` / `streamable-http` through the official
Rust MCP SDK; legacy SSE is rejected. HTTP configs may declare headers with
`${NAME}`, `${env:NAME}`, or `${NAME:-default}` environment placeholders.
Interactive OAuth browser flows are not implemented; use an anonymous endpoint
or provide an already-issued credential through an explicit header.
Connection and every MCP tool call require approval unless auto mode is
explicitly enabled. Tools are exposed as `mcp__<plugin>_<server>__<tool>` and
remain bounded. Stdio servers receive a small ambient environment allowlist,
declared `env`, `PLUGIN_ROOT`, `CLAUDE_PLUGIN_ROOT`, and persistent
`.agents/plugin-data/<plugin>`; provider credentials are not inherited.

See [configuration](docs/configuration.md) and the copyable
[extension examples](examples/extensions/README.md).

## Operational contract

- `mini-agent --version` reports the Cargo release version.
- `mini-agent status [--json]` reports effective non-secret startup settings.
- `mini-agent doctor [--json]` validates configuration, workspace, and shell
  availability without contacting a model provider.
- A UTF-8 root `AGENTS.md` is appended once to the stable system prompt with a
  16 KiB hard limit.
- Project skills, plugins, marketplaces, and MCP use fixed `.agents/` locations and do
  not rewrite conversation history.
- Mini Agent Harness sends no telemetry, update checks, or crash reports.
- Shell execution is approval-gated but not sandboxed.
- Interactive sessions are process-local in v0.1 and cannot yet be resumed.

See [configuration](docs/configuration.md),
[troubleshooting](docs/troubleshooting.md), [data and privacy](docs/privacy.md),
[security](SECURITY.md), the [changelog](CHANGELOG.md), and the
[release procedure](docs/releasing.md).

See [the experiment protocol](docs/experiments.md), the
[unknown-tool comparison](docs/experiments/unknown-tool.md), the
[edit-surface comparison](docs/experiments/edit-surface.md), the
[tool-output comparison](docs/experiments/tool-output-retention.md), the
[real-model prompt-weight protocol](docs/experiments/prompt-weight.md), the
[effect-recovery comparison](docs/experiments/effect-recovery.md), and the
[harness boundary](docs/harness-boundary.md). The exact defaults and failure
behavior are listed in [harness limits](docs/limits.md).

Licensed under the [MIT License](LICENSE).
