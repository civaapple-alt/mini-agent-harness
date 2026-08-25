# Troubleshooting

Start with:

```sh
mini-agent doctor
mini-agent status
```

`doctor --json` and `status --json` provide the same information for scripts.

## Missing provider configuration

`mini-agent --version`, `status`, and `demo` work without credentials. `doctor`
prints a structured report and exits non-zero until `OPENAI_API_KEY` and
`OPENAI_MODEL` are set. For a downloaded binary, create
`~/.mini-agent/.env` (`%USERPROFILE%\.mini-agent\.env` on Windows), copy
`.env.demo` into it, and fill `OPENAI_API_KEY`. A workspace `.env` overrides
the user file; process environment values override both. `status` shows the
source without printing the secret.

## AGENTS.md is too large

`ask` and the interactive terminal still start. Mini-agent keeps a 64 KiB head
and tail of root `AGENTS.md`, marks the gap with `[truncated]`, and prints a
warning. `doctor` reports the oversize check as an error. Trim the file if the
omitted middle contains rules the model must see. Invalid UTF-8 still prevents
startup.

## PowerShell commands fail on Windows

mini-agent intentionally uses `pwsh`, not Windows PowerShell. Install
PowerShell 7 and confirm `pwsh` is on `PATH`; `mini-agent doctor` checks this.

## A noninteractive tool call is denied

`ask` cannot stop a script to obtain approval when stdin is not a terminal.
Use `ask --auto` only inside a workspace and execution environment you trust.
The interactive REPL does not prompt by default; `/auto off` turns prompts on.

## A command produces too much output

Foreground output is captured with a hard limit. Large completed results return
a preview and a process-local handle; the model can call `read_tool_result` to
inspect a byte range or literal match. Use managed-process tools for servers and
watchers instead of a foreground shell command.

## A trace path already exists

Trace creation refuses to overwrite files. Choose a new path or explicitly
move the existing trace first.

## A durable session is locked

Only one process may write a session. Exit the other process before resuming.
After confirming no mini-agent process is using that session, a lock left by a
crash can be removed from `.agents/sessions/<SESSION_ID>.lock`; the JSONL data
file remains untouched. Mini-agent never removes a stale lock automatically.

Mentor commands acquire this same lock so they cannot derive from a checkpoint
while another process mutates the session. Exit the interactive owner before
running `mentor insight` or `mentor verify`.

## A mentor command reports missing configuration

Set `MENTOR_OPENAI_MODEL`. The mentor uses `OPENAI_API_KEY` and
`OPENAI_BASE_URL` unless `MENTOR_OPENAI_API_KEY` or
`MENTOR_OPENAI_BASE_URL` overrides them. `mini-agent status` reports whether
the mentor is enabled without printing credentials; `doctor` validates the
effective mentor endpoint when the model is configured.

## A cloned marketplace skill or plugin is missing

Cloning a marketplace does not enable every entry. Put the clone under
`.agents/marketplaces/<directory>`, add the desired immediate skill directory or
plugin name to `.agents/marketplaces.json`, then run `mini-agent doctor`. Only
local marketplace sources are loaded. Remote `url` and `git-subdir` entries must
be installed or cloned locally first.

## An MCP server is not discovered

Use plugin-root `mcp.json` for Agent Plugins v1, plugin-root `.mcp.json` for a
Claude/Grok plugin, or `.agents/mcp.json` / `.agents/mcp/<server>.json` for a
standalone server. `status --json` separates `mcp_stdio_servers` and
`mcp_http_servers`. Legacy SSE is unsupported; use streamable HTTP or stdio.

An HTTP server can also fail because a referenced header environment variable
is missing. Use `${NAME:-}` only when an empty value is valid for that server.
If a connection was denied or startup failed transiently, enter `/mcp` in the
same interactive session to retry. Existing conversation history is preserved.
