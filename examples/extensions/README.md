# Extension configuration examples

Mini Agent Harness reads extensions only from the startup workspace. The
`project/` directory is the canonical, complete example and mirrors a real
`.agents/` tree:

```text
project/
└── .agents/
    ├── skills/repository-review/
    ├── skillsets/example-collection/skills/project-summary/
    ├── plugins/context7/
    ├── marketplaces/example-marketplace/
    ├── marketplaces.json
    └── mcp/context7-stdio.json
```

The sibling `skill/`, `plugin/`, and `mcp/` directories are component snippets
and alternatives intended to be copied into the matching `.agents/` directory;
they are not complete workspace layouts. Do not copy every MCP alternative at
once. `http-project/` remains a focused runnable HTTP MCP example.

The local reference repositories map to mini-agent as follows:

| Local clone | Recommended project destination | Mode |
| --- | --- | --- |
| `taste-skill` | `.agents/marketplaces/taste-skill` | `{ "skills": ["minimalist-skill"] }`, or clone under `skillsets` to load the whole `skills/` collection |
| `context7` | `.agents/marketplaces/context7` | `{ "plugins": ["context7"] }` for its Claude marketplace entry; alternatively copy `plugins/agent-plugins/context7` into `.agents/plugins/context7` |
| `anthropics-skills` | `path` in `.agents/marketplaces.json` or `.agents/skillsets.json` | `{ "path": "../shared-skills/anthropics-skills", "skills": ["skill-creator"] }` |
| `agent-skills` | `path` in `.agents/skillsets.json` | `{ "path": "../shared-skills/agent-skills", "skills": ["react-best-practices"] }`; omit json to load a cloned `.agents/skillsets/agent-skills` in full |
| `claude-plugins-official` | `.agents/marketplaces/claude-plugins-official` | enable only selected local Claude plugins |
| `xai-org-plugin-marketplace` | `.agents/marketplaces/xai-org-plugin-marketplace` | enable selected local Grok plugins such as `neon` |

## Skill and skill collection

- Copy `skill/repository-review` to `.agents/skills/repository-review`.
- See `project/.agents/skillsets/example-collection` for a self-contained
  cloned-style skill collection example.
- Point `.agents/skillsets.json` at an existing local collection and list the
  skills to enable, or clone a repository whose root contains `skills/` into
  `.agents/skillsets/<name>` when you want the whole collection without json:

  ```json
  {
    "skillsets": {
      "agent-skills": {
        "path": "../shared-skills/agent-skills",
        "skills": ["react-best-practices"]
      }
    }
  }
  ```

Skill collections use compatibility naming because real skills.sh repositories
can intentionally use an install name that differs from the source folder.
Direct `.agents/skills` and portable Agent Plugins remain standards-strict.

## Installed plugin

Copy `plugin/context7` to `.agents/plugins/context7`. It is a portable Agent
Plugins v1 package containing one skill and one remote HTTP MCP server.

Claude and Grok plugins can also be copied to `.agents/plugins/<name>` without
conversion. Their `skills/`, `agents/*.md`, and `.mcp.json` components are
adapted; commands, hooks, LSP, and client UI remain client-specific.

## Plugin marketplace

The self-contained example lives at
`project/.agents/marketplaces/example-marketplace`, with its selector in
`project/.agents/marketplaces.json`.

Copy `marketplaces.json` to `.agents/marketplaces.json`. Use `skills` for a
`SKILL.md` directory (immediate or nested inside the clone) and `plugins` for a
Claude or Grok marketplace plugin. Set `path` to an existing local clone, or
clone into `.agents/marketplaces/<key>`. Remote marketplace entries are never
downloaded by mini-agent. Copy `skillsets.json` to `.agents/skillsets.json` to
enable named skills from a local collection without loading every skill.

```powershell
git clone https://github.com/example/taste-skill .agents\marketplaces\taste-skill
git clone https://github.com/example/anthropics-skills .agents\marketplaces\anthropics-skills
git clone https://github.com/example/claude-plugins-official .agents\marketplaces\claude-plugins-official
git clone https://github.com/example/xai-org-plugin-marketplace .agents\marketplaces\xai-org-plugin-marketplace
Copy-Item examples\extensions\marketplaces.json .agents\marketplaces.json
```

## Standalone MCP

Copy one file from `mcp/` to `.agents/mcp/<server>.json`. The HTTP example uses
an optional `CONTEXT7_API_KEY`; the stdio example launches the npm package.
Both server connection and each tool call remain approval-gated unless auto
mode is enabled.

Run `mini-agent doctor --json` after changing extensions. Counts for skills,
plugins, marketplaces, stdio MCP, and HTTP MCP are also available through
`mini-agent status --json`.

`http-project` is a runnable minimal workspace containing the HTTP Context7
configuration. From that directory, run the built `mini-agent` binary with
provider variables configured to exercise the complete remote MCP path.
