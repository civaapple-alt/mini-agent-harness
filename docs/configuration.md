# Configuration

mini-agent resolves provider settings in this order:

1. process environment;
2. `.env` in the startup workspace;
3. `~/.mini-agent/.env` (`%USERPROFILE%\.mini-agent\.env` on Windows);
4. a built-in default, where one exists.

A PATH-installed binary should keep credentials in the user file so they are
not copied into every workspace. A workspace `.env` still overrides the user
file when a project needs a different key or model. The App Server initialize
response reports the bounded non-secret capability manifest.

| Variable | Required | Meaning |
| --- | --- | --- |
| `OPENAI_API_KEY` | for primary commands | Bearer credential for the Responses endpoint |
| `OPENAI_MODEL` | for primary commands | Provider model identifier. DeepSeek flash/pro image-bearing requests use `deepseek-v4-flash-vision-exp` |
| `OPENAI_BASE_URL` | no | Responses API root; defaults to `https://api.openai.com/v1`. Files API is `{base}/files` |
| `VERIFIER_OPENAI_MODEL` | for Goal verification | Goal verifier model identifier |
| `VERIFIER_OPENAI_API_KEY` | no | Goal verifier credential override; otherwise inherits `OPENAI_API_KEY` |
| `VERIFIER_OPENAI_BASE_URL` | no | Goal verifier API root override; otherwise inherits `OPENAI_BASE_URL` |
| `MINI_AGENT_MAX_STEPS` | internal only | Optional Core runaway-loop guard for local experiments; not a Web/Goal control |
| `MINI_AGENT_GOAL_MAX_LOOPS` | no | Maximum Goal continuation loops; defaults to `100` |
| `MINI_AGENT_GOAL_STEP_BUDGET` | no | Maximum Core model steps per Goal milestone; defaults to `200` |
| `MINI_AGENT_GOAL_TIMEOUT_SECS` | no | Wall-clock timeout for one Goal milestone; defaults to `1800` seconds |
| `MINI_AGENT_PROJECT_ID` | Web/Host binding | Trusted Project identity used for scoped approval ownership |
| `MINI_AGENT_EXTRA_READ_ROOTS` | Web/Host binding | Path-separated associated roots exposed as read-only references |
| `MINI_AGENT_EXTRA_WRITE_ROOTS` | Web/Host binding | Path-separated associated roots admitted for edits |

## Runtime composition and prompt/rule sources

The Host assembles a bounded runtime composition before the App Server starts.
The composition chooses the model/tool scope, extension load depth, foundational
agent, persona, and workflow policy, plus sandbox and security selections. These
are internal composition inputs, not a user-facing Profile or Session axis. The regular `general` agent still has explicit prompt and
rule configuration. Its stable context is assembled from the built-in prompt,
project `AGENTS.md`, selected extension instructions, and workflow riders in
that order; safety and host policy rules retain higher precedence.

For `general`, the built-in prompt is the regular-agent base contract, while
`promptSources` and `ruleSources` independently control the project,
extension, and workflow inputs. Output behavior and context limits remain
typed runtime policy, rather than arbitrary prompt text. Foundational
`explore` and `plan` agents add their own bounded contract and read-mostly rule;
personas add an overlay to either contract. No public startup request or Web
setting selects a Profile identity. Neither interface can carry arbitrary prompt bodies,
commands, paths, or credentials.

Local experiments may use `--no-tools` to resolve the
same composition into a model-only runtime. Tool and extension construction is
skipped, while sessions, App Server turns, events, and persistence remain the
same. Startup output and the App Server initialize response expose the bounded capability
manifest, including enabled/disabled groups and prompt/rule source names.
The host keeps prompt admission and rule admission as separate settings. Their
source names are visible independently in the manifest, along
with the effective typed policy for workspace writes, shell execution, and
workflow scope. The manifest also lists each rule source in precedence
order as active, shadowed, or disabled with a bounded reason. Explore and Plan
read-only workspace rules at the host boundary. The built-in
prompt and core safety rules are always retained.
Loaded prompt and rule sources may expose a deterministic 16-character
fingerprint in structured status for comparison; the source text itself is
never returned.
The manifest also reports the fixed prompt/rule precedence, the current rule
resolver phase, and the effective bounded context limits; it never includes
prompt bodies or secrets.

### Embedding an external capability provider

The internal runtime composition contains provider IDs only. An embedding
application registers the implementation in a local `CapabilityRegistry`, then
passes that registry to `HostRuntimeFactory`; untrusted workspace files cannot
load arbitrary code. The repository
includes a complete, runnable tool-provider example at
`crates/mini-agent-capabilities/examples/external_tool_provider.rs`:

```rust
let registry = CapabilityRegistry::builtin()
    .with_tool_provider(Arc::new(ExampleToolProvider));

let runtime = HostRuntimeFactory::new(&config, approval, harness_config)
    .with_registry(registry)
    .build(runtime_composition, results)?;
```

An in-process App Server embedder can pass the same registry through the
`RuntimeStartOptions.registry` field; the regular `start*` methods continue to
use the built-in registry.

`ExampleToolProvider` publishes the stable ID `example-echo` and constructs an
`external_echo` tool. The provider receives runtime-scoped workspace, sandbox,
approval, image, and result-store inputs through `ToolBuildRequest`; it does
not own the execution loop.

External model providers use a separate factory seam. The compile-checked
example at `crates/mini-agent-app-server/examples/external_model_provider.rs`
registers a stable model ID and starts `AppServerRuntime<EchoModel>` with:

```rust
AppServerRuntime::<EchoModel>::start_with_model_factory(
    RuntimeStartOptions {
        runtime_config,
        approval,
        harness_config,
        session_request: SessionRequest::Disabled,
        control: Arc::new(RunControl::new()),
        profile: runtime_composition,
        registry,
    },
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

Use the App Server `initialize` response to inspect the bounded non-secret
capability manifest. The REPL keeps this as a core turn surface and does not
duplicate the App Server runtime status dashboard. Neither output contains
credentials. Provider configuration is validated when a provider-backed turn
starts.

If the startup workspace contains `AGENTS.md`, mini-agent appends its UTF-8
contents once to the stable system prompt. The file has a 16 KiB hard limit.
Oversized files are not dropped: the host keeps a UTF-8-safe head and tail,
inserts an explicit `[truncated]` marker, and prints a warning. Invalid UTF-8
still fails startup. Nested instruction discovery is not part of the v0.1
contract.

World state is not configured through environment variables. Mini-agent
detects a fixed command catalog, root project markers, host shell, OS,
architecture, execution mode, and approval behavior. App Server clients can
inspect the current snapshot with `world/state` and refresh it with
`world/refresh` after installing a command or changing root project files. The
core REPL keeps the snapshot in the model context but does not duplicate this
management dashboard. World-state items are bounded and contain no environment
values or command output.

## Durable sessions

Interactive and one-shot `ask` sessions always persist; there is no
persistence opt-out setting. Restore a known session with
`mini-agent resume SESSION_ID`.
Settled records live under `~/.mini-agent/sessions/<workspace>/<session-id>/`,
where `<workspace>` is the percent-encoded absolute project path.

Each modular session directory contains fast O(1) metadata index `summary.json`,
runtime telemetry `signals.json`, frozen environment snapshot `prompt_context.json`,
and the append-only `session.jsonl` log. Large tool results are recorded as
`result_stored` entries in that same log so handles survive resume.

## App Server Plan Mode and Autonomous Goal Workspaces

Mini-Agent decouples task execution workflows from approval policies. These
workflows are owned by the App Server and are intended for Studio/SDK clients;
the core REPL remains focused on turn execution and run control:

- **Plan Mode (`thread/settings/update`)**: Set `collaborationMode.mode` to
  `"plan"` to make exploration read-mostly. Bounded scratch scripts and outputs
  may be created in the Session-owned plan area, and `plan.md` is retained;
  formal Project mutations remain locked. The setting is applied by the App
  Server Runtime Actor to the settled Thread, approval controller, and bounded
  Host-composed prompt; arbitrary raw system-prompt replacement is not accepted.
  Planning state is persisted in `plan_mode.json`.
- **Autonomous Goal Mode (`thread/goal/set|get|clear`)**: Materializes a
  dedicated `goal/` workspace containing `state.json` (milestone progress, loop
  counts, verifier scores) and `plan.md` (acceptance criteria). Each ordinary
  Goal turn is persisted as a settled checkpoint before the independent,
  tool-free verifier runs; approved, rejected, and verifier-error outcomes are
  applied by the serialized GoalRuntime. There is no client-submitted manual
  `workflow/goal/*` control path. `thread/settings/updated` reports settings
  mutations with the same Runtime revision as their responses. Goal objectives
  are capped at 8 KiB, and while a Goal is active the relative `goal/...` tool
  path is bound to this session-owned workspace rather than the project root.

Goal limits can be shortened in a workspace `.env` for deterministic local
fixtures. `MINI_AGENT_GOAL_STEP_BUDGET` and the Core loop guard are runtime
safety limits, not user task controls. `MINI_AGENT_GOAL_TIMEOUT_SECS` starts
at that turn's execution boundary and requests cooperative cancellation when it
expires; Core then settles the turn at a safe boundary, so a synchronous tool
effect is never killed halfway through. A step or timeout limit projects as
`usageLimited`. Provider-reported input plus output tokens are accumulated in
`tokensUsed`; reaching `tokenBudget` stops the Goal and projects as
`budgetLimited`. Missing provider usage metadata is not estimated or silently
converted into tokens.
- **Built-in Foundations & Personas**: Supports 3 core agent roles (`explore`, `plan`, `general`) and 3 bounded personas (`reviewer`, `implementer`, `researcher`).

## Goal verification

When Goal Mode has a verifier gate, set `VERIFIER_OPENAI_MODEL` to run a separate,
tool-free check against the latest settled checkpoint. On restart, an unsettled
Goal schedules a new ordinary turn; a settled Goal checkpoint is verified again
without replaying that turn. A clear operation invalidates pending verifier
results, so a late result cannot advance a cleared or replaced Goal:

The verifier inherits the primary credential and endpoint unless the
verifier-specific overrides are set. It has a separate system role, exactly one
model step, and an empty tool catalog. Its bounded verdict is stored in the
Goal workspace and is not replayed as primary conversation history.
If verifier preparation fails, or the verifier is not configured, the active
Goal is durably marked failed with a bounded reason. A verifier result is only
accepted when its `goalId`, `turnId`, and settled checkpoint sequence still
match the current Goal runtime state.

## Project extensions

Installed skills, plugins, and MCP configs stay inside the startup workspace.
`read_file`, `apply_patch`, `shell`, and `read_image` are the default workspace
tools. `web_fetch` and MCP tools are explicit extensions; `write_file` and
`edit_file` are not part of the current tool surface or compatibility selection.

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

Client-specific commands, hooks, LSP, UI metadata, model selection, and
subagent isolation are not executed by mini-agent.

### Standalone MCP

One-server files under `.agents/mcp/*.json` use this native shape:

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
name is accepted as an alias for streamable HTTP. `connect_timeout_ms`
defaults to 20 seconds and is capped at 120 seconds. Header placeholders use
`${NAME}`, `${env:NAME}`, or `${NAME:-default}`; missing variables without a
default fail before connecting. Mini-agent does not run an interactive OAuth
browser flow; choose an anonymous endpoint or configure an existing credential.

Stdio configurations accept `command`, `args`, `env`, and `cwd`. Portable
`${PLUGIN_ROOT}` / `${PLUGIN_DATA}` package placeholders are supported without
invoking a shell.

Copyable skill, plugin, HTTP MCP, and stdio MCP examples live in
[`examples/extensions`](../examples/extensions/README.md).
