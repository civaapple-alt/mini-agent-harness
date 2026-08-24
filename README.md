# mini-codex

`mini-codex` is a small native agent harness for studying why some harnesses
help a model and others get in its way.

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

Durable storage, conversation trees, lanes, queues, hooks, compaction, schema
migration, and crash recovery are separate experiments. None enters core until
an experiment demonstrates that its benefit justifies its permanent cost.

The deterministic demo proves the complete model -> tool -> model -> answer
path without network or credentials. The default command opens a small
multi-turn terminal using a streaming OpenAI Responses adapter at the CLI
edge. It keeps history only for the life of the process; `/new` clears it.

## Run

Copy `.env.demo` to `.env` and fill in `OPENAI_API_KEY`. The demo defaults to
DeepSeek's Responses API with the `deepseek-v4-flash` model. Process environment
values take precedence over `.env`; the local file is ignored by Git.

```sh
cargo test
cargo run -p mini-codex-cli -- demo "make this loud"
OPENAI_API_KEY=... OPENAI_MODEL=... cargo run -p mini-codex-cli
OPENAI_API_KEY=... OPENAI_MODEL=... cargo run -p mini-codex-cli -- run "say hello"
cargo run -p mini-codex-cli -- --trace trace.jsonl
python scripts/line_budget.py
```

The real `run` command exposes `read_file`, `edit_file`, `write_file`, and
`shell`. Reads are confined to the current workspace. `.git` is protected.
`edit_file` makes one exact unique replacement; `write_file` creates new files
and refuses to replace existing ones. Writes and shell commands ask for
approval every time and fail closed when stdin is not an interactive terminal.
Shell execution is not sandboxed yet; the approval boundary is explicit rather
than presented as isolation. Oversized tool results retain their beginning and
end inside a hard byte budget, with truncation explicit in the event trace.
Shell processes have a 120-second deadline and bounded concurrent stdout/stderr
capture; timeout terminates the process tree.
Conversation persistence, resume, and compaction are deliberately absent from
this first terminal slice.

See [the experiment protocol](docs/experiments.md), the
[unknown-tool comparison](docs/experiments/unknown-tool.md), the
[edit-surface comparison](docs/experiments/edit-surface.md), the
[tool-output comparison](docs/experiments/tool-output-retention.md), the
[real-model prompt-weight protocol](docs/experiments/prompt-weight.md), the
[effect-recovery comparison](docs/experiments/effect-recovery.md), and the
[harness boundary](docs/harness-boundary.md). The exact defaults and failure
behavior are listed in [harness limits](docs/limits.md).
