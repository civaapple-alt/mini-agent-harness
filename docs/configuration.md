# Configuration

mini-agent resolves provider settings in this order:

1. process environment;
2. `.env` in the startup workspace;
3. a built-in default, where one exists.

| Variable | Required | Meaning |
| --- | --- | --- |
| `OPENAI_API_KEY` | yes | Bearer credential for the Responses endpoint |
| `OPENAI_MODEL` | yes | Provider model identifier |
| `OPENAI_BASE_URL` | no | API root; defaults to `https://api.openai.com/v1` |

The adapter appends `/responses` to `OPENAI_BASE_URL`. DeepSeek's Responses API
therefore uses:

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

Run `mini-agent status` to inspect the effective non-secret configuration and
its source and detected world state. `status` never prints the credential. Run
`mini-agent doctor` to validate provider configuration and the host shell
without starting an agent turn. Both commands accept `--json`.

If the startup workspace contains `AGENTS.md`, mini-agent appends its UTF-8
contents once to the stable system prompt. The file has a 16 KiB hard limit;
startup fails explicitly rather than silently omitting or truncating oversized
instructions. Nested instruction discovery is not part of the v0.1 contract.

World state is not configured through environment variables. Mini-agent
detects a fixed command catalog, root project markers, host shell, OS,
architecture, execution mode, and approval behavior. Use `/world` to inspect
the current snapshot and `/world refresh` after installing a command or
changing root project files. World-state items are bounded and contain no
environment values or command output.

## Project extensions

All extension paths are relative to the startup workspace and must remain
inside it after filesystem resolution.

### Skills

Install one standards-strict Agent Skill at
`.agents/skills/<skill>/SKILL.md`. A directly cloned collection belongs at
`.agents/skillsets/<collection>` and may contain either a root `SKILL.md` or an
immediate `skills/*/SKILL.md` collection. Collection compatibility mode accepts
the install-name/folder-name differences used by repositories such as
`taste-skill` and `vercel-labs/agent-skills`.

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

Clone each marketplace into `.agents/marketplaces/<directory>` and explicitly
enable local entries in `.agents/marketplaces.json`:

```json
{
  "marketplaces": {
    "taste-skill": ["minimalist-skill"],
    "anthropics-skills": ["skill-creator"],
    "claude-plugins-official": ["code-simplifier"],
    "xai-org-plugin-marketplace": ["neon"]
  }
}
```

The object key is the clone's directory name. Each array contains selectors. A
selector first enables an immediate `skills/<selector>/SKILL.md` directory when
present; otherwise it selects a marketplace plugin with that name. Direct skill
selection wins when a skill directory and plugin entry share a name. Claude
string sources and Grok local source objects are resolved inside the clone. An
enabled remote source is diagnosed but never downloaded; install it under
`.agents/plugins` or use a marketplace clone containing a local source.

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
