# Troubleshooting

Start with `mini-agent --version`, then configure the provider environment before
running `ask`, `auto`, or the interactive session. Inside the interactive
session, `/status` shows the effective runtime state.

## Missing provider configuration

`mini-agent --version` works without credentials. Provider-backed turns require
`OPENAI_API_KEY` and `OPENAI_MODEL`. For a downloaded binary, create
`~/.mini-agent/.env` (`%USERPROFILE%\.mini-agent\.env` on Windows), copy
the provider settings into it, and fill `OPENAI_API_KEY`. A workspace `.env`
overrides the user file; process environment values override both.

## AGENTS.md is too large

`ask` and the interactive terminal still start. Mini-agent keeps a 16 KiB head
and tail of root `AGENTS.md`, marks the gap with `[truncated]`, and prints a
warning. Trim the file if the omitted middle contains rules the model must see.
Invalid UTF-8 still prevents
startup.

## PowerShell commands fail on Windows

mini-agent intentionally uses `pwsh`, not Windows PowerShell. Install
PowerShell 7 and confirm `pwsh` is on `PATH` before using shell tools.

## A noninteractive tool call is denied

`ask` cannot stop a script to obtain approval when stdin is not a terminal.
Use `ask --auto-approve` (or `-y`) only inside a workspace and execution environment you trust.
The interactive REPL does not prompt by default; `/auto off` turns prompts on.

## A command produces too much output

Foreground output is captured with a hard limit. Large completed results return
a preview and a process-local handle; the model can call `read_tool_result` to
inspect a byte range or literal match. Use managed-process tools for servers and
watchers instead of a foreground shell command.

## A durable session is locked

Only one process may write a session. Exit the other process before resuming.
After confirming no mini-agent process is using that session, a lock left by a
crash can be removed from
`~/.mini-agent/sessions/<workspace>/<SESSION_ID>/session.lock`; the JSONL data
file remains untouched. Mini-agent never removes a stale lock automatically.

Goal verification uses the same session lock so it cannot inspect a checkpoint
while another process mutates the session. Exit the interactive owner before
starting a Goal verifier.

## Goal verification reports missing configuration

Set `VERIFIER_OPENAI_MODEL`. The verifier uses `OPENAI_API_KEY` and
`OPENAI_BASE_URL` unless `VERIFIER_OPENAI_API_KEY` or
`VERIFIER_OPENAI_BASE_URL` overrides them. The legacy `MENTOR_OPENAI_*` names
remain accepted as fallbacks. It runs with one model step and no tools, and
stores only its bounded verdict in the Goal workspace.

## A workspace skill or plugin is missing

Only workspace-local `.agents/skills/<skill>/SKILL.md` entries and installed
`.agents/plugins/<plugin>` packages are discovered. Check the bounded YAML name,
plugin manifest, and workspace path.

## An MCP server is not discovered

Use plugin-root `mcp.json` for Agent Plugins v1, or
`.agents/mcp/<server>.json` for a standalone server. `/status` reports the
currently loaded MCP summary. Legacy SSE is unsupported; use streamable HTTP
or stdio.

An HTTP server can also fail because a referenced header environment variable
is missing. Use `${NAME:-}` only when an empty value is valid for that server.
If a connection was denied or startup failed transiently, enter `/mcp` in the
same interactive session to retry. Existing conversation history is preserved.

## File tools and workspace paths

`read_file`, `read_image`, `edit_file`, and `write_file` accept paths located inside the active
workspace, whether given as relative paths (e.g. `src/main.rs`) or absolute paths
(e.g. `D:\workspace\src\main.rs`). Paths that escape the workspace (such as
`../secret` or external directories) or point to `.git` are strictly rejected.

## Real-time web search and network data

By default, mini-agent enables built-in Responses API `web_search` (`{"type": "web_search"}`)
so the model can query the internet without writing raw local shell/PowerShell scrape scripts.
To disable web search, pass `--no-web-search` (or `--no-search`) or set `MINI_AGENT_WEB_SEARCH=false`
in `.env`. Use host `web_fetch` for a known URL when the provider does not expose built-in search.

`web_search` is for discovery. To read a known public URL, or a local Vite/Next/Vue/React
dev server, use `web_fetch` instead of `curl` or PowerShell download cmdlets. `web_fetch` admits
public `http`/`https` URLs and loopback (`localhost`, `127.0.0.1`, `[::1]`). It still rejects
credentials, LAN/private IPs, cloud metadata (`169.254.169.254`), and `file:` paths, and it
does not run JavaScript. A public page cannot redirect onto loopback. Client-only SPAs may
come back as a thin shell with a warning; SSR HTML is returned as markdown. `read_file` is for
source. There is no screenshot, vision,
or headless-browser tool.

## Image understanding

`read_image` is for existing PNG/JPEG/GIF/WebP files (screenshots, diagrams, UI captures). Pass a
workspace-relative path, or an absolute path on this machine such as a file under Pictures. Outside
the workspace, `auto` and other automatic-approval sessions proceed after the same approval gate as
shell; interactive ask/N sessions prompt. Do not copy those files into the project.

It uploads the file once through DeepSeek Files API (`POST /files`, `purpose=user_data`) and later
turns reuse the returned `file_id`. Inline base64 is only a fallback if that upload fails.

DeepSeek text models ignore `input_image` (they replace it with a placeholder). When the current
`OPENAI_MODEL` is `deepseek-v4-flash` or `deepseek-v4-pro` and the request actually contains images,
that one request is sent as `deepseek-v4-flash-vision-exp`. All requests use the Responses endpoint;
DeepSeek keeps using `file_id` from the envelope when present. Resume and fork reload session
`attachments/` so image turns can be retried without losing the local bytes.

`read_file` still refuses binary images; use `read_image`. There is no screenshot or browser-capture
tool.
