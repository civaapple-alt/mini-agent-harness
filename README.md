# mini-codex

`mini-codex` is a small native agent harness for studying why some harnesses
help a model and others get in its way.

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

- `mini-codex-core` stays between 10,000 and 20,000 Rust source lines when it
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

- `mini-codex-core` owns the small contracts and the explicit agent loop.
- `mini-codex-cli` owns terminal presentation and provider/tool adapters.

Core intentionally has no provider, filesystem, process, MCP, plugin, session,
or TUI framework. The harness loop is concrete rather than hidden behind a
policy framework. Experiments should change the loop, record its events, and
compare outcomes before extracting another abstraction.

Pi v2's durable harness design is an important reference, but not the default
scope. mini-codex keeps only three lessons at the foundation:

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
`mini-codex` (or `mini-codex.exe`) on `PATH`.

To build from source, install Rust 1.88 or newer and run:

```sh
cargo build --release --locked -p mini-codex-cli
./target/release/mini-codex --version
```

On Windows, use `target\release\mini-codex.exe`. Runtime shell tools require
PowerShell 7 (`pwsh`) on Windows and `sh` on Unix systems.

## Quick start

Copy `.env.demo` to `.env` and fill in `OPENAI_API_KEY`. The demo defaults to
DeepSeek's Responses API with the `deepseek-v4-flash` model. Process environment
values take precedence over `.env`. This repository ignores `.env`; verify the
same before using it in another
workspace, and prefer process secrets in CI.

```sh
mini-codex doctor
mini-codex status
mini-codex
mini-codex ask "summarize this repository"
mini-codex ask --json "summarize the current changes"
mini-codex help ask
mini-codex auto "inspect this repository, improve it, and run the tests"
mini-codex --trace trace.jsonl
```

Use `--` before a prompt that begins with `-`, for example
`mini-codex ask -- --explain-this`.

`ask` writes final assistant Markdown exactly once to stdout and streams only
reasoning/tool progress to stderr, so it composes with shell pipelines. It also
accepts a bounded prompt from stdin.
`ask --json` emits one JSON object containing output, exit code, model, steps,
usage, and tool-call statuses. Noninteractive sensitive tool calls fail closed;
`ask --auto` permits them without approval and should be used only in a trusted
or disposable execution environment.

The deterministic provider-free path remains available to verify the complete
model → tool → model loop:

```sh
mini-codex demo "make this loud"
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

## Operational contract

- `mini-codex --version` reports the Cargo release version.
- `mini-codex status [--json]` reports effective non-secret startup settings.
- `mini-codex doctor [--json]` validates configuration, workspace, and shell
  availability without contacting a model provider.
- A UTF-8 root `AGENTS.md` is appended once to the stable system prompt with a
  16 KiB hard limit.
- mini-codex sends no telemetry, update checks, or crash reports.
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

Licensed under the [Apache License 2.0](LICENSE).
