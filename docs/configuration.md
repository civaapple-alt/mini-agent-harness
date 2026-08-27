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
| `OPENAI_MODEL` | for primary commands | Provider model identifier. DeepSeek flash/pro image-bearing requests are sent as `deepseek-v4-flash-vision-exp` |
| `OPENAI_BASE_URL` | no | Responses API root; defaults to `https://api.openai.com/v1`. Files API is `{base}/files` |
| `OPENAI_CHAT_BASE_URL` | for GLM image turns | Chat Completions API root. The adapter appends `/chat/completions` unless the value already ends with that path. Not inferred from `OPENAI_BASE_URL`. |
| `MENTOR_OPENAI_MODEL` | for mentor commands | Independent mentor model identifier |
| `MENTOR_OPENAI_API_KEY` | no | Mentor credential override; otherwise inherits `OPENAI_API_KEY` |
| `MENTOR_OPENAI_BASE_URL` | no | Mentor API root override; otherwise inherits `OPENAI_BASE_URL` |
| `MINI_AGENT_MAX_STEPS` | no | Copilot/auto model-step cap; `0` means unlimited (the default) |

`read_image` bytes stay in the session `attachments/` directory (not `session.jsonl`). Resume reloads
them; fork copies them. Compaction does not attach images.

The adapter appends `/responses` to `OPENAI_BASE_URL`. DeepSeek's Responses API
therefore uses:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

GLM Coding Plan (Responses, not Chat Completions) uses a **coding-plan key** from
[个人编程套餐概览](https://bigmodel.cn/coding-plan/personal/overview), not a general platform key:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=glm-5.3
OPENAI_BASE_URL=https://open.bigmodel.cn/api/v1
OPENAI_CHAT_BASE_URL=https://open.bigmodel.cn/api/coding/paas/v4
```

`glm-5.3` is text-only. Image turns (`read_image`) are sent as `glm-5.3-flash` for that request
over Chat Completions, with a user `image_url.url` data URL.

Text and tool turns stay on `{OPENAI_BASE_URL}/responses`. DeepSeek image turns still use
`function_call_output` `input_image.file_id`. Mini-agent does not rewrite the Responses URL into
a Chat Completions URL. Set `OPENAI_MODEL=glm-5.3-flash` if you want native vision on every turn
(Coding Plan quota is 3× vs 5.3). Built-in Responses `web_search` stays off for BigModel. Known URLs use
host `web_fetch`. Optional Coding Plan search is Remote MCP `webSearchPrime`
(`https://open.bigmodel.cn/api/mcp/web_search_prime/mcp`).

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

Interactive and one-shot `ask` sessions are in-memory by default. Use
`--persist` to save a session; use `--ephemeral` (or `--no-persist`) to make
the choice explicit. `auto` sessions persist by default and accept
`--ephemeral`. List known IDs with `mini-agent sessions`, and restore one with
`mini-agent resume SESSION_ID`.
Settled records live under `~/.mini-agent/sessions/<workspace>/<session-id>/`,
where `<workspace>` is the percent-encoded absolute project path. No provider
setting enables persistence implicitly.

Each modular session directory contains fast O(1) metadata index `summary.json`,
runtime telemetry `signals.json`, frozen environment snapshot `prompt_context.json`,
and the append-only `session.jsonl` log.

## Plan Mode and Autonomous Goal Workspaces

Mini-Agent decouples task execution workflows from approval policies:

- **Plan Mode (`/plan` or `/plan <prompt>`)**: Locks codebase mutations to read-only while permitting edits exclusively to the session living plan (`~/.mini-agent/sessions/<workspace>/<id>/plan.md`). Relative path `plan.md` maps to that file. Tracks planning state in `plan_mode.json`.
- **Autonomous Goal Mode (`/goal <objective>`)**: Materializes a dedicated `goal/` workspace containing `state.json` (milestone progress, loop counts, verifier scores) and `plan.md` (acceptance criteria). Integrates with independent mentor verifiers (`goal/verifier_verdict.md`) to provide blind validation gates before advancing milestones.
- **Built-in Foundations & Personas**: Supports 3 core agent roles (`explore`, `plan`, `general`) and 7 specialized personas (`reviewer`, `implementer`, `security-auditor`, `test-writer`, `researcher`, `design-doc-writer`, `design-doc-reviewer`) with dual-mode file contracts (`review_file`, `summary_file`).

## Mentor insight and verification

Set `MENTOR_OPENAI_MODEL`, then analyze the latest settled checkpoint of a
durable session:

```sh
mini-agent mentor insight SESSION_ID
mini-agent mentor verify SESSION_ID -- "the requested tests passed and the diff is clean"
```

Both commands accept `--json` and `--trace PATH`. The mentor inherits the
primary credential and endpoint unless the mentor-specific overrides are set.
It has a separate system role, exactly one model step, and an empty tool
catalog. Its output is appended as a derived item linked to the immutable source
checkpoint; it is not inserted into the primary thread's replay history.

## Project extensions

Installed skills, plugins, and MCP configs stay inside the startup workspace.
Marketplace and skillset `path` values may point at an existing local clone
outside the workspace; `read_file` can open files inside those configured
roots, while `edit_file` and `write_file` remain workspace-only.

### Skills

Install one standards-strict Agent Skill at
`.agents/skills/<skill>/SKILL.md`. A cloned collection can live at
`.agents/skillsets/<collection>` or be referenced from `.agents/skillsets.json`.
Without `skillsets.json`, every immediate child of `.agents/skillsets/` loads
its root `SKILL.md` and immediate `skills/*/SKILL.md`. With `skillsets.json`,
only named skillsets and listed skill names (directory or YAML `name`) are
enabled:

```json
{
  "skillsets": {
    "anthropics-skills": {
      "path": "../shared-skills/anthropics-skills",
      "skills": ["frontend-design", "skill-creator"]
    }
  }
}
```

`path` is optional and defaults to `.agents/skillsets/<key>`. A string array is
shorthand for that default path plus an explicit skill list. Collection
compatibility mode accepts the install-name/folder-name differences used by
repositories such as `taste-skill` and `vercel-labs/agent-skills`.

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

### Plugin marketplaces

Name local marketplace clones in `.agents/marketplaces.json`. Prefer an object
that separates skills from plugins. `path` is optional and may be an existing
local directory; omit it to use `.agents/marketplaces/<key>`:

```json
{
  "marketplaces": {
    "taste-skill": { "skills": ["minimalist-skill"] },
    "anthropics-skills": {
      "path": "../shared-marketplaces/anthropics-skills",
      "skills": ["skill-creator"]
    },
    "claude-plugins-official": { "plugins": ["code-simplifier"] },
    "xai-org-plugin-marketplace": { "plugins": ["neon"] },
    "cursor-plugins": {
      "path": "../shared-marketplaces/cursor-plugins",
      "skills": ["thermo-nuclear-code-quality-review"]
    }
  }
}
```

The object key is a local name. `skills` selects `SKILL.md` directories by
directory name or YAML `name`: an immediate `skills/<name>/SKILL.md` first,
otherwise a bounded walk of at most five levels inside the clone. A Claude or Grok
marketplace manifest is not required for skill-only selection. `plugins`
selects a marketplace plugin entry and still requires `.claude-plugin` or
`.grok-plugin` `marketplace.json`. A legacy string array remains accepted,
uses `.agents/marketplaces/<key>`, and still means "immediate skill, else
plugin". Direct skill selection in that legacy form wins when a skill
directory and plugin entry share a name. Claude string sources and Grok local
source objects are resolved inside the clone. An enabled remote source is
diagnosed but never downloaded; install it under `.agents/plugins` or set
`path` to a local clone.

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

Copyable skill, plugin, marketplace, HTTP MCP, and stdio MCP examples live in
[`examples/extensions`](../examples/extensions/README.md).
