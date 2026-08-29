# Extension configuration examples

Mini Agent Harness reads extensions only from the startup workspace. The
`project/` directory is the canonical example and mirrors the supported
`.agents/` layout:

```text
project/
└── .agents/
    ├── skills/repository-review/
    ├── plugins/context7/
    └── mcp/context7-stdio.json
```

The sibling `skill/`, `plugin/`, and `mcp/` directories contain component
snippets. Copy them into the matching `.agents/` directory; do not copy both
MCP transport alternatives unless the workspace needs both.

## Skills

Copy `skill/repository-review` to `.agents/skills/repository-review`. Skills
must contain a standards-strict `SKILL.md`; only immediate workspace-local
entries are discovered.

## Installed plugins

Copy `plugin/context7` to `.agents/plugins/context7`. It is a portable Agent
Plugins v1 package containing one skill and one remote HTTP MCP server.

Only the portable Agent Plugins v1 layout is accepted. Client-specific
commands, hooks, LSP, and UI metadata are not executed by mini-agent.

## Standalone MCP

Copy one file from `mcp/` to `.agents/mcp/<server>.json`. The HTTP example uses
an optional `CONTEXT7_API_KEY`; the stdio example launches the configured
process. Connection and tool-call approval follow the active runtime profile.

Use `/status` in an interactive session after changing extensions to inspect the
non-secret runtime and MCP summary.
