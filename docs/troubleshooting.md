# Troubleshooting

Start with `mini-agent --version`, then configure a provider and global default
in Web Studio before running `run` or the REPL. Inspect effective runtime
state through the App Server `initialize`, `world/state`, and `mcp/status`
interfaces; the REPL does not duplicate that management dashboard.

For Web Studio startup, Project, Session attach, WebSocket, and browser blank
page failures, follow [`studio-integration.md`](studio-integration.md) first;
this document focuses on the CLI/Host/App Server symptoms shared by both
frontends.

## Missing provider configuration

`mini-agent --version` works without credentials. For provider-backed turns,
open Web Studio and use **Settings → Agent capabilities → Model settings** to
add a provider, configure its API Key and model, and save a global default.
Project defaults are set in Project settings. The Host stores the catalog and
credential under `~/.mini-agent`; legacy model variables in `.env` are ignored.
Use **Test connection** to check the selected model when needed; this sends one
small provider request and may incur a charge.

## AGENTS.md is too large

`run` and the interactive terminal still start. Mini-agent keeps a 16 KiB head
and tail of root `AGENTS.md`, marks the gap with `[truncated]`, and prints a
warning. Trim the file if the omitted middle contains rules the model must see.
Invalid UTF-8 still prevents
startup.

## PowerShell commands fail on Windows

mini-agent intentionally uses `pwsh`, not Windows PowerShell. Install
PowerShell 7 and confirm `pwsh` is on `PATH` before using shell tools.

## A long shell command makes steer or stop appear stuck

The shell tool runs in the App Server's blocking tool boundary. Current
Windows native runs attach the PowerShell process tree to a Job Object. A
`turn/interrupt` request publishes a host-local cancellation token; Core races
it against the active model future and drops that local future without waiting
for a long reasoning response. The provider may continue remote computation
after the local request is cancelled. A running cancellable tool receives the
same signal. Before each next call in a tool batch, Core checks cancellation,
records calls that never started as `cancelled`, and keeps the real result for
calls that did start. This closes every model `callId` without claiming that
an uncertain side effect did not happen.

Steering an active Turn is admitted directly into the bounded control queue and
may be acknowledged before the current tool checkpoint returns. Stop takes
precedence over a steer that has not reached a later model context. For
macOS/Linux Ctrl+C, the Gateway first rejects new Turns and issues interrupts,
then uses one shared 10-second window to collect terminal events and
authoritative Turn/checkpoint reads. An unresolved Turn remains recoverable;
the Gateway does not replay it. SDK shutdown then closes the isolated App
Server process group, with up to 5 seconds for graceful and forced cleanup.
If a manual Stop remains unresolved, inspect runtime status and App Server
stderr, then reconnect and let recovery read the persisted Turn rather than
resending the prompt or a tool call.

## A noninteractive tool call is denied

`run` cannot stop a script to obtain approval when stdin is not a terminal.
Use `run --auto-approve` (or `-y`) only inside a workspace and execution environment you trust.
The REPL uses its local approval adapter.

## A command produces too much output

Foreground output is captured with a hard limit. Large completed results return
a bounded preview and are retained as a session-side artifact; result
continuation is not exposed by the Builtin catalog. Keep shell output narrow
when the omitted middle is important.

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

Choose a separate Goal Verifier default in Web Studio's Model settings. The
verifier uses that model's configured provider credentials, runs with one model
step and no tools, and stores only its bounded verdict in the Goal workspace.
It is optional for ordinary chat.

## A workspace skill or plugin is missing

The runtime discovers project `.agents/skills/<skill>/SKILL.md`, user
`%USERPROFILE%/.agents/skills/<skill>/SKILL.md`, user
`%USERPROFILE%/.mini-agent/skills/<skill>/SKILL.md`, synchronized builtin groups,
and installed `.agents/plugins/<plugin>` packages. Check the bounded YAML name,
the expected direct-child layout, plugin manifest, and project path. If a Skill
is shown in metadata but its `SKILL.md` cannot be read, check that its enabled
root is present in the runtime's Skill read roots.

## An MCP server is not discovered

Use plugin-root `mcp.json` for Agent Plugins v1, or
`.agents/mcp/<server>.json` for a standalone server. App Server `mcp/status`
reports the currently loaded MCP summary. Legacy SSE is unsupported; use
streamable HTTP or stdio.

An HTTP server can also fail because a referenced header environment variable
is missing. Use `${NAME:-}` only when an empty value is valid for that server.
If a connection was denied or startup failed transiently, call App Server
`mcp/retry` from Studio or an SDK client. Existing conversation history is
preserved.

## A model request fails with a transport error

The Responses adapter retries connection-establishment failures up to two times
with short delays before ending the Turn. HTTP error responses and failures after
the streaming response begins are not retried, because the provider may already
have processed the request or emitted part of the answer. The error includes the
bounded underlying connection cause when the transport exposes one. If retries
are exhausted, the checkpoint is preserved; check the configured endpoint,
network route, proxy, and TLS connectivity before continuing the Thread.

On Windows, `os error 10053` with `error writing a body to connection` means the
local host aborted a socket while the request was being sent. That message alone
does not identify whether a proxy, VPN, security product, network path, or remote
peer closed it, and it does not prove the request body was too large. The provider
may have received some or all of the request, so the adapter does not replay this
ambiguous send failure automatically. Continue in the same Thread to use its
preserved checkpoint; if it repeats, compare the configured endpoint and network
route and inspect local proxy/VPN/security logs around the failure time.

## File tools and workspace paths

`read_file` and `read_image` accept paths located inside the active Project
workspace, including configured associated reference roots. `Full access`
explicitly expands the current Runtime/Session path scope to the machine, but
paths still cannot point to `.git` and all hard Deny rules remain active.
`apply_patch` is the only Builtin file mutation path. Its Codex-style patch paths
must be relative to the workspace, and it validates every affected file before
writing. The removed `write_file` and `edit_file` names are not accepted.

## Real-time web search and network data

The shared `web_search` tool is configured independently from the Responses
model provider. Select and configure one provider in Web Studio's “联网搜索”
settings; without a selected provider and its key, the tool is not exposed. The
CLI `--no-web-search` (or `--no-search`) option can hide search for one run.
See [web search](web-search.md) for provider setup and the search-to-fetch flow.

`web_search` is for discovery. To read a result URL, a known public URL, or a local Vite/Next/Vue/React
dev server, use `web_fetch` instead of `curl` or PowerShell download cmdlets. `web_fetch` admits
public `http`/`https` URLs and loopback (`localhost`, `127.0.0.1`, `[::1]`). It still rejects
credentials, LAN/private IPs, cloud metadata (`169.254.169.254`), and `file:` paths, and it
does not run JavaScript. A public page cannot redirect onto loopback. Client-only SPAs may
come back as a thin shell with a warning; SSR HTML is returned as markdown. A public `web_fetch`
request requires Host approval in Interactive policy. Automatic and Trusted policies admit it
after URL validation; loopback remains an explicit allowed target. Network
timeouts and transient transport failures are reported as retryable tool outcomes. `read_file`
returns bounded, line-numbered pages; workspace and configured extension-root paths are allowed,
while an existing file outside those roots requires Host admission. Pass its `next_offset` back as
`offset` to continue. `apply_patch`
is the preferred multi-file mutation path and validates all affected files before writing. There is no screenshot, vision,
or headless-browser tool.

`web_fetch` uses the App Server process environment for `HTTP_PROXY`, `HTTPS_PROXY`, or
`ALL_PROXY`, including lowercase names and SOCKS proxy URLs. It honors `NO_PROXY` and always sends
loopback URLs directly. If Clash resolves a public hostname to a `198.18.0.0/15` fake-IP, Host uses
the configured proxy to resolve that hostname. It still rejects private, loopback, mixed-class, or
metadata DNS results. Start or restart Gateway from a shell that has the proxy variables set so
its App Server process inherits them.

## Image understanding

`read_image` is for existing PNG/JPEG/GIF/WebP files (screenshots, diagrams, UI captures). Pass a
workspace-relative path, or an absolute path on this machine such as a file under Pictures. Outside
the Project workspace, `Full access` may admit a path only under its remaining
security and approval rules. Do not copy files into the Project just to make an
out-of-scope path appear allowed.

It uploads the file once through DeepSeek Files API (`POST /files`, `purpose=user_data`) and later
turns reuse the returned `file_id`. Inline base64 is only a fallback if that upload fails.

DeepSeek text models ignore `input_image` (they replace it with a placeholder). When a selected
DeepSeek text model is `deepseek-flash`, `deepseek-v4-flash`, or `deepseek-v4-pro` and the request actually contains images,
that one request is sent as `deepseek-v4-flash-vision-exp`. All requests use the Responses endpoint;
DeepSeek keeps using `file_id` from the envelope when present. Resume and fork reload session
`attachments/` so image turns can be retried without losing the local bytes.

`read_file` still refuses binary images; use `read_image`. There is no screenshot or browser-capture
tool.
