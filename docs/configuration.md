# Configuration

mini-agent resolves provider settings in this order:

1. process environment;
2. `.env` in the startup workspace;
3. `~/.mini-agent/.env` (`%USERPROFILE%\.mini-agent\.env` on Windows);
4. a built-in default, where one exists.

A PATH-installed binary should keep credentials in the user file so they are
not copied into every workspace. A workspace `.env` still overrides the user
file when a project needs a different key or model. `status` reports the
non-secret source of each value.

| Variable | Required | Meaning |
| --- | --- | --- |
| `OPENAI_API_KEY` | for primary commands | Bearer credential for the Responses endpoint |
| `OPENAI_MODEL` | for primary commands | Provider model identifier. DeepSeek flash/pro image-bearing requests use `deepseek-v4-flash-vision-exp` |
| `OPENAI_BASE_URL` | no | Responses API root; defaults to `https://api.openai.com/v1`. Files API is `{base}/files` |
| `MENTOR_OPENAI_MODEL` | for mentor commands | Independent mentor model identifier |
| `MENTOR_OPENAI_API_KEY` | no | Mentor credential override; otherwise inherits `OPENAI_API_KEY` |
| `MENTOR_OPENAI_BASE_URL` | no | Mentor API root override; otherwise inherits `OPENAI_BASE_URL` |
| `MINI_AGENT_MAX_STEPS` | no | Copilot/auto model-step cap; `0` means unlimited (the default) |
| `MINI_AGENT_GOAL_MAX_LOOPS` | no | Maximum Goal milestone attempts; defaults to `20` |
| `MINI_AGENT_GOAL_STEP_BUDGET` | no | Maximum model steps per Goal milestone; defaults to `50` |
| `MINI_AGENT_GOAL_TIMEOUT_SECS` | no | Wall-clock timeout for one Goal milestone; defaults to `600` seconds |
| `MINI_AGENT_PROFILE` | standalone App Server only | Startup profile name: `interactive`, `ask`, `auto`, or `demo` |

## Runtime profiles and prompt/rule sources

Each frontend selects a bounded host profile before the App Server starts:
`interactive`, `ask`, or `auto`. The profile chooses the model/tool
scope, extension load depth, foundational agent, persona, and Goal/Plan
workflow policy, plus sandbox and security selections. The regular `general` agent still has explicit prompt and
rule configuration. Its stable context is assembled from the built-in prompt,
project `AGENTS.md`, selected extension instructions, and workflow riders in
that order; safety and host policy rules retain higher precedence.

For `general`, the built-in prompt is the regular-agent base contract, while
`promptSources` and `ruleSources` independently control the project,
extension, and workflow inputs. Output behavior and context limits remain
typed runtime policy, rather than arbitrary prompt text. Foundational
`explore` and `plan` agents add their own bounded contract and read-only rule;
personas add an overlay to either contract. Profile files and App Server
requests currently select the bounded source switches; typed presets for the
regular-agent base prompt, output contract, and context policy are reserved for
the next profile stage. Neither interface can carry arbitrary prompt bodies,
commands, paths, or credentials.

Use `--no-tools` with `interactive`, `ask`, `run`, or `auto` to resolve the
same profile into a model-only runtime. Tool and extension construction is
skipped, while sessions, App Server turns, events, and persistence remain the
same. Startup output and `status --json` expose the bounded capability
manifest, including enabled/disabled groups and prompt/rule source names.
The host keeps prompt admission and rule admission as separate profile
settings. Their source names are visible independently in the manifest, along
with the effective typed policy for workspace writes, shell/process execution,
and workflow scope. The manifest also lists each rule source in precedence
order as active, shadowed, or disabled with a bounded reason. Explore and Plan
profiles enforce read-only workspace rules at the host boundary. The built-in
prompt and core safety rules are always retained.
Loaded prompt and rule sources may expose a deterministic 16-character
fingerprint in structured status for comparison; the source text itself is
never returned.
The manifest also reports the fixed prompt/rule precedence, the current rule
resolver phase, and the effective bounded context limits; it never includes
prompt bodies or secrets.

An optional `.agents/profile.json` can override bounded profile selections for
the local CLI and standalone App Server. It accepts `modelProvider`,
`toolProvider`, `extensionProvider`, `policyProvider`, `tools`, `extensionDepth`,
`selectedExtensions`, `agent`, `persona`, `workflows`, `promptSources`,
`ruleSources`, `sandbox`, and `security`; unknown fields, oversized files,
unsafe names, credentials, commands, paths, and arbitrary prompt text are
rejected. Explicit command-line deny flags such as `--no-tools` are applied
after the file.

```json
{
  "name": "repo-review",
  "modelProvider": "openai",
  "toolProvider": "builtin",
  "extensionProvider": "builtin",
  "policyProvider": "builtin",
  "tools": "all",
  "extensionDepth": "enabled",
  "selectedExtensions": ["review"],
  "agent": "general",
  "persona": "reviewer",
  "workflows": "plan",
  "promptSources": {"project": true, "extensions": true, "workflows": true},
  "ruleSources": {"project": true, "extensions": true, "workflows": true},
  "sandbox": "native",
  "security": "default"
}
```

### Embedding an external capability provider

Profiles contain provider IDs only. An embedding application registers the
implementation in a local `CapabilityRegistry`, then passes that registry to
`HostRuntimeFactory`; profile files cannot load arbitrary code. The repository
includes a complete, runnable tool-provider example at
`crates/mini-agent-capabilities/examples/external_tool_provider.rs`:

```rust
let registry = CapabilityRegistry::builtin()
    .with_tool_provider(Arc::new(ExampleToolProvider));

let runtime = HostRuntimeFactory::new(&config, approval, harness_config)
    .with_registry(registry)
    .build(profile, results)?;
```

An in-process App Server embedder can pass the same registry through
`AppServerRuntime::start_with_control_and_profile_and_registry`; the regular
`start*` methods continue to use the built-in registry.

`ExampleToolProvider` publishes the stable ID `example-echo` and constructs an
`external_echo` tool. The provider receives runtime-scoped workspace, sandbox,
approval, image, and result-store inputs through `ToolBuildRequest`; it does
not own the execution loop.

External model providers use a separate factory seam. The compile-checked
example at `crates/mini-agent-app-server/examples/external_model_provider.rs`
registers a stable model ID and starts `AppServerRuntime<EchoModel>` with:

```rust
AppServerRuntime::<EchoModel>::start_with_model_factory(
    runtime_config,
    approval,
    harness_config,
    SessionRequest::Disabled,
    Arc::new(RunControl::new()),
    profile,
    registry,
    echo_factory,
).await?;
```

The factory receives resolved `ModelProviderSettings` and an `ImageStore`; it
returns a type implementing the Core `Model` contract. External extension and
policy providers remain Host capability seams, while the App Server keeps the
same protocol and workflow control plane for every model implementation.

`read_image` bytes stay in the session `attachments/` directory (not `session.jsonl`). Resume reloads
them; fork copies them. Compaction does not attach images.

The adapter appends `/responses` to `OPENAI_BASE_URL`. DeepSeek's Responses API
therefore uses:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

All provider requests use `{OPENAI_BASE_URL}/responses`. DeepSeek image turns
use `function_call_output` `input_image.file_id`; mini-agent does not select a
second provider protocol or rewrite the endpoint based on the model name.

Run `mini-agent status` to inspect the effective non-secret configuration and
its source and detected world state. `status` never prints the credential and
succeeds even when the provider is unconfigured. Run `mini-agent doctor` to
validate provider configuration and the host shell without starting an agent
turn. `doctor` exits non-zero while `OPENAI_API_KEY` or `OPENAI_MODEL` is
missing. Both commands accept `--json`. `mini-agent demo` needs no credentials.

If the startup workspace contains `AGENTS.md`, mini-agent appends its UTF-8
contents once to the stable system prompt. The file has a 16 KiB hard limit.
Oversized files are not dropped: the host keeps a UTF-8-safe head and tail,
inserts an explicit `[truncated]` marker, and prints a warning. Invalid UTF-8
still fails startup. Nested instruction discovery is not part of the v0.1
contract.

World state is not configured through environment variables. Mini-agent
detects a fixed command catalog, root project markers, host shell, OS,
architecture, execution mode, and approval behavior. Use `/world` to inspect
the current snapshot and `/world refresh` after installing a command or
changing root project files. World-state items are bounded and contain no
environment values or command output.

## Durable sessions

Interactive, one-shot `ask`, and `auto` sessions always persist; there is no
persistence opt-out setting. List known IDs
with `mini-agent sessions`, and restore one with
`mini-agent resume SESSION_ID`.
Settled records live under `~/.mini-agent/sessions/<workspace>/<session-id>/`,
where `<workspace>` is the percent-encoded absolute project path.

Each modular session directory contains fast O(1) metadata index `summary.json`,
runtime telemetry `signals.json`, frozen environment snapshot `prompt_context.json`,
and the append-only `session.jsonl` log. Large tool results are recorded as
`result_stored` entries in that same log so handles survive resume.

## Plan Mode and Autonomous Goal Workspaces

Mini-Agent decouples task execution workflows from approval policies:

- **Plan Mode (`/plan` or `/plan <prompt>`)**: Locks codebase mutations to read-only while permitting edits exclusively to the session living plan (`~/.mini-agent/sessions/<workspace>/<id>/plan.md`). Relative path `plan.md` maps to that file. Tracks planning state in `plan_mode.json`.
- **Autonomous Goal Mode (`/goal <objective>`)**: Materializes a dedicated `goal/` workspace containing `state.json` (milestone progress, loop counts, verifier scores) and `plan.md` (acceptance criteria). Integrates with independent mentor verifiers (`goal/verifier_verdict.md`) to provide blind validation gates before advancing milestones.

Goal limits can be shortened in a workspace `.env` for deterministic local
fixtures. A timeout stops the current milestone cooperatively, persists a
failed Goal state, and leaves the REPL available for another command; it does
not forcibly interrupt synchronous tool effects.
- **Built-in Foundations & Personas**: Supports 3 core agent roles (`explore`, `plan`, `general`) and 7 specialized personas (`reviewer`, `implementer`, `security-auditor`, `test-writer`, `researcher`, `design-doc-writer`, `design-doc-reviewer`) with dual-mode file contracts (`review_file`, `summary_file`).

## Mentor insight and verification

Set `MENTOR_OPENAI_MODEL`, then analyze the latest settled checkpoint of a
durable session:

```sh
mini-agent mentor insight SESSION_ID
mini-agent mentor verify SESSION_ID -- "the requested tests passed and the diff is clean"
```

Both commands accept `--json`. The mentor inherits the
primary credential and endpoint unless the mentor-specific overrides are set.
It has a separate system role, exactly one model step, and an empty tool
catalog. Its output is appended as a derived item linked to the immutable source
checkpoint; it is not inserted into the primary thread's replay history.

## Project extensions

Installed skills, plugins, and MCP configs stay inside the startup workspace.
`read_file`, `edit_file`, and `write_file` remain workspace-scoped.

### Skills

Install one standards-strict Agent Skill at
`.agents/skills/<skill>/SKILL.md`. Only workspace-local entries are discovered,
and the directory name must match the bounded YAML `name` field.

### Plugins

Put one installed plugin in `.agents/plugins/<plugin>`. Supported package
shapes are:

| Ecosystem | Manifest | Skills | MCP | Additional instructions |
| --- | --- | --- | --- | --- |
| Agent Plugins v1 | `plugin.json` | `skills/` | `mcp.json` | none |
| Claude | `.claude-plugin/plugin.json` | `skills/` | `.mcp.json` | `agents/*.md` |
| Grok | `.grok-plugin/plugin.json` | `skills/` | `.mcp.json` | `agents/*.md` |

Claude/Grok commands, hooks, LSP, UI metadata, model selection, and subagent
isolation remain client-specific and are not executed by mini-agent.

### Standalone MCP

An aggregate `.agents/mcp.json` accepts either `{"mcpServers": {...}}` or the
legacy direct server map. One-server files under `.agents/mcp/*.json` use this
native shape:

```json
{
  "name": "context7",
  "transport": "http",
  "enabled": true,
  "url": "https://mcp.context7.com/mcp",
  "headers": {
    "Authorization": "${CONTEXT7_API_KEY:-}"
  },
  "connect_timeout_ms": 60000
}
```

Supported transports are `stdio`, `http`, and `streamable-http`. The `http`
name is the Claude/Grok alias for streamable HTTP. `connect_timeout_ms`
defaults to 20 seconds and is capped at 120 seconds. Header placeholders use
`${NAME}`, `${env:NAME}`, or `${NAME:-default}`; missing variables without a
default fail before connecting. Mini-agent does not run an interactive OAuth
browser flow; choose an anonymous endpoint or configure an existing credential.

Stdio configurations accept `command`, `args`, `env`, and `cwd`. Portable
`${PLUGIN_ROOT}` / `${PLUGIN_DATA}` and legacy `${CLAUDE_PLUGIN_ROOT}` package
placeholders are supported without invoking a shell.

Copyable skill, plugin, HTTP MCP, and stdio MCP examples live in
[`examples/extensions`](../examples/extensions/README.md).
