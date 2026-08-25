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

Durable settled-turn storage is an opt-in CLI-host experiment. Conversation
trees, lanes, hooks, operation registers, and effect recovery remain separate;
they do not enter core until an experiment justifies their permanent cost. The
explicit auto mode is the context-compaction experiment: it summarizes a
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
clears history while restoring the current bounded world-state item. `/world`
shows the detected execution environment and `/world refresh` appends a new
snapshot when it changes. If an MCP server is denied or fails during startup,
`/mcp` retries it without clearing conversation history. The welcome block lists a
bounded set of discovered skill and plugin names and shows configured MCP
servers as inactive until connection approval succeeds.

Interactive history remains in-memory by default. `--persist` creates a
project-local durable session, `sessions` lists bounded session files, and
`resume SESSION_ID` restores the latest completely settled checkpoint. `/new`
starts a new thread inside a durable session; `/session` shows its identity.
The append-only JSONL record distinguishes session, thread, turn, and item
identities and stores a checkpoint only after settlement. It deliberately does
not replay a turn interrupted during a provider or tool effect.

`mentor insight SESSION_ID` and `mentor verify SESSION_ID CRITERIA` run an
independently configured, tool-free model against that session's latest settled
checkpoint. The result is appended to the same JSONL file as a derived item
linked to the source checkpoint sequence and fingerprint. It is never replayed
into the primary conversation.

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

After the source build, first-use needs no provider credentials. On Windows,
run the same arguments with `target\release\mini-agent.exe`.

```sh
mini-agent --version
mini-agent doctor
mini-agent status
mini-agent demo "make this loud"
```

`--version` prints the Cargo release version. `doctor` and `status` report
non-secret startup checks without contacting a provider. `doctor` exits
non-zero while `OPENAI_API_KEY` or `OPENAI_MODEL` is missing; `status` still
prints the snapshot. `demo` runs the deterministic model → tool → model →
answer path locally.

Before `ask`, the interactive terminal, `auto`, or mentor commands, set
`OPENAI_API_KEY` and `OPENAI_MODEL`. For a PATH-installed binary, keep secrets
in the user file rather than a project directory. On Windows that file is
`%USERPROFILE%\.mini-agent\.env`; on Unix it is `~/.mini-agent/.env`:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

A workspace `.env` overrides the user file; process environment values override
both. This repository ignores `.env`; verify the same before using a workspace
file elsewhere, and prefer process secrets in CI.

Set `MENTOR_OPENAI_MODEL` to enable mentor commands. The mentor inherits the
primary key and base URL by default; `MENTOR_OPENAI_API_KEY` and
`MENTOR_OPENAI_BASE_URL` provide explicit overrides. This permits a distinct
model or provider without coupling it to normal agent turns.

```sh
mini-agent
mini-agent ask "summarize this repository"
mini-agent ask --json "summarize the current changes"
mini-agent help ask
mini-agent auto "inspect this repository, improve it, and run the tests"
mini-agent --persist
mini-agent sessions
mini-agent resume SESSION_ID
mini-agent mentor insight SESSION_ID
mini-agent mentor verify SESSION_ID -- "tests pass and requested behavior is evidenced"
mini-agent --trace trace.jsonl
```

Use `--` before a prompt that begins with `-`, for example
`mini-agent ask -- --explain-this`.

`ask` streams reasoning/tool progress to stderr. In a terminal, assistant text
streams to stdout under one colored `assistant>` tag and is not repeated at the
end. Redirected stdout and `--json` hold the assistant answer until completion
so machine output remains valid. It also accepts a bounded prompt from stdin.
`ask --json` emits one JSON object containing output, exit code, model, steps,
usage, and tool-call statuses. On a TTY, `ask` runs tools without per-step
approval. When stdin is not a TTY, sensitive tool calls fail closed; `ask --auto`
permits them and should be used only in a trusted or disposable execution
environment. `run` is an alias of `ask`.

The real modes expose `read_file`, `edit_file`, `write_file`, `shell`,
`read_tool_result`, and managed-process tools. Reads and direct file writes are
confined to the current workspace; `.git` is protected. `edit_file` makes one
exact unique replacement; `write_file` creates new files and refuses to replace
existing ones. Reads never prompt. The interactive REPL and TTY `ask` run
writes, shell commands, process starts, and MCP without per-step approval;
shell is not sandboxed. `/auto off` restores per-action prompts. `auto` without
a prompt starts the REPL in the copilot loop; `/auto` enables that loop in any
interactive session. `auto` with a prompt remains a one-shot copilot. Copilot
mode runs up to 128 model steps and compacts context between settled tool
batches. It prints a warning because process execution is not sandboxed and
can escape the workspace even though direct file tools cannot.

At startup mini-agent detects a fixed, bounded set of local command
capabilities such as `git`, `rg`, `fd`, `tree`, `curl`, Cargo, Java/Maven,
Go, Python, and Node tooling, plus root project markers and workspace wrappers.
It appends these facts with the current shell, OS, mode, approval behavior, and
sandbox boundary as a typed world-state item. Mode changes append a new item;
they do not rewrite the stable system prompt, preserving provider prefix-cache
opportunities. See [world state](docs/world-state.md).

Foreground `shell` commands have a 120-second deadline and bounded concurrent
stdout/stderr capture; timeout terminates the process tree. A large result is
returned as a short head/tail preview plus a process-local handle that
`read_tool_result` can inspect by byte range or literal query. Long-lived
services should use `process_start`, `process_read`, `process_list`, and
`process_stop`; their logs and process count are bounded, and remaining process
trees are stopped when the CLI exits. Result handles, queued input, running
operations, and managed processes remain non-durable. Opt-in sessions retain
settled conversation checkpoints, not live effects or process state.

## Skills, plugins, marketplaces, and MCP

Mini Agent Harness provides one bounded, project-scoped compatibility layer:

| Capability | Project location | Supported formats |
| --- | --- | --- |
| installed skill | `.agents/skills/<skill>/SKILL.md` | Agent Skills, standards-strict |
| cloned skill collection | `.agents/skillsets/<repo>/` or `.agents/skillsets.json` | root `SKILL.md` or `skills/*/SKILL.md`; json selects names and optional local `path` |
| installed plugin | `.agents/plugins/<plugin>/` | Agent Plugins v1, Claude plugin, or Grok plugin |
| cloned marketplace | `.agents/marketplaces/<repo>/` or `path` in `.agents/marketplaces.json` | Claude or Grok plugins, or nested `SKILL.md` skills |
| standalone MCP | `.agents/mcp.json` or `.agents/mcp/<server>.json` | aggregate or one-server configuration |

[Agent Plugins 1.0.0](https://agent-plugins.org/) use root `plugin.json`,
`skills/`, and `mcp.json`. Claude and Grok plugins use their native hidden
manifest, `skills/`, compatible `agents/*.md` instructions, and `.mcp.json`.
Client-specific commands, hooks, LSP, UI, and nested-agent execution are not
emulated. A compatible plugin agent becomes on-demand instructions for the
main harness and is labeled `plugin-agent` in model-visible metadata.

Marketplace repositories are inert until local selectors are named in
`.agents/marketplaces.json`. An object with `skills` and `plugins` keys names
each kind separately. Optional `path` points at an existing local clone;
otherwise the clone must live at `.agents/marketplaces/<key>`. `skills`
matches an immediate `skills/<name>/SKILL.md` or, failing that, a bounded
nested `SKILL.md` directory of that name inside the clone, without requiring
a Claude or Grok marketplace manifest. `plugins` matches a marketplace plugin
entry. A legacy string array still means "immediate skill, else plugin" under
`.agents/marketplaces/<key>`. Mini-agent resolves string sources such as
`./plugins/name` and Grok `{ "type": "local", "path": "..." }` sources,
including marketplace entries with explicit `skills` arrays. It deliberately
does not download remote `url` or `git-subdir` entries.

If `.agents/skillsets.json` is present, only the listed skillsets and skill
directory names are enabled. Without that file, every immediate child of
`.agents/skillsets/` still loads its whole collection.

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
explicitly enabled. A server rejected or unavailable during interactive startup
can be retried with `/mcp`; successfully discovered tools are added to the
current conversation without resetting history. Tools are exposed as
`mcp__<plugin>_<server>__<tool>` and remain bounded. Stdio servers receive a
small ambient environment allowlist, declared `env`, `PLUGIN_ROOT`,
`CLAUDE_PLUGIN_ROOT`, and persistent `.agents/plugin-data/<plugin>`; provider
credentials are not inherited.

See [configuration](docs/configuration.md) and the copyable
[extension examples](examples/extensions/README.md).

## Operational contract

- `mini-agent --version` reports the Cargo release version and git revision,
  for example `mini-agent 0.1.0 (c0ffee12abcd)`. The interactive welcome prints
  the same line.
- `mini-agent status [--json]` reports effective non-secret startup settings.
- `mini-agent doctor [--json]` validates configuration, workspace, and shell
  availability without contacting a model provider.
- A UTF-8 root `AGENTS.md` is appended once to the stable system prompt with a
  64 KiB hard limit. Oversized files keep a UTF-8-safe head and tail with an
  explicit truncation marker; invalid UTF-8 still fails startup.
- World state is an append-only, 8 KiB-bounded context item; `status` and
  `/world` expose the same non-secret snapshot.
- Project skills, plugins, marketplaces, and MCP use fixed `.agents/` locations and do
  not rewrite conversation history.
- Mini Agent Harness sends no telemetry, update checks, or crash reports.
- Shell execution is approval-gated but not sandboxed.
- Interactive sessions are process-local by default; `--persist` and `resume`
  opt into project-local settled-turn persistence.
- Mentor insight and verification require a durable settled checkpoint, expose
  no tools, and append only a non-replayed derived item.

See [configuration](docs/configuration.md),
[troubleshooting](docs/troubleshooting.md), [data and privacy](docs/privacy.md),
[security](SECURITY.md), the [changelog](CHANGELOG.md), and the
[release procedure](docs/releasing.md). The durable item and mentor direction
is recorded in [world state](docs/world-state.md).

See [the experiment protocol](docs/experiments.md), the
[unknown-tool comparison](docs/experiments/unknown-tool.md), the
[edit-surface comparison](docs/experiments/edit-surface.md), the
[tool-output comparison](docs/experiments/tool-output-retention.md), the
[real-model prompt-weight protocol](docs/experiments/prompt-weight.md), the
[effect-recovery comparison](docs/experiments/effect-recovery.md), and the
[harness boundary](docs/harness-boundary.md). The exact defaults and failure
behavior are listed in [harness limits](docs/limits.md).

Licensed under the [MIT License](LICENSE).
