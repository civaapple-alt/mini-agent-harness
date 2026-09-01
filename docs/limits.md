# Harness limits

Every value sent to or accepted from a model has a direct hard bound. The
defaults are part of the harness rather than terminal flags.

| Boundary | Default | Behavior at limit |
| --- | ---: | --- |
| one host context item | 8 KiB | reject before retaining the item |
| user input | 32 KiB | reject before retaining or emitting the text |
| model response | 64 KiB | reject before retaining text or tool calls |
| tool calls in one model step | 8 | reject the whole proposal before effects |
| one tool result | 16 KiB | retain UTF-8-safe head and tail |
| model request context | 1 MiB | reject before the provider request |
| model steps in one run | 8, or `0` for no cap | return `step_limit` when a positive cap is reached |

Context size is the byte length of the system prompt plus JSON-serialized
messages and tool specifications. It is a provider-neutral safety ceiling, not
a prediction of provider tokenization. DeepSeek V4 advertises a 1M-token
context and 384K-token output; the harness still caps a request at 1 MiB and a
model step at 64 KiB so one turn cannot fill that window. Provider-reported
token counts remain available separately in model-response events.

Reasoning and assistant text deltas share the model-response ceiling. They stop
reaching observers once the combined response crosses it, and the completed
response is then rejected. The two streams remain distinct in the live event
stream and terminal output. Settled reasoning is replayed as a Responses API reasoning
item before the same assistant turn's text and tool calls. Tool output is
different: it comes from an already-performed external effect, so the harness
retains a bounded result and marks `truncated` explicitly instead of discarding
the outcome.

Every runtime limit failure emits `run_failed` with a structured
`limit_exceeded` reason. The default behavior does not compact or delete
history behind the user's back; in the interactive terminal, `/new` is the
explicit way to clear a conversation that has reached its context ceiling.

The default interactive session skips per-step approval and keeps the 8-step
reject-at-ceiling loop. The explicit copilot `auto` mode changes two loop
defaults: it uses `max_steps = 0` (no step cap, like fx `max_agent_steps`) and
selects `ContextLimitBehavior::Compact`. Set `MINI_AGENT_MAX_STEPS` in the
process environment, workspace `.env`, or `~/.mini-agent/.env` to impose a
positive cap; `0` remains unlimited. It can be selected by starting `auto`
with or without a prompt or by entering `/auto` in an interactive session;
`/auto off` restores per-action approval and the 8-step defaults. Before a
normal sampling request, settled history at or above half of the 1 MiB ceiling
is compacted. The newest context item and a bounded recent tail stay verbatim:
the last two model-step groups (each an assistant message plus its following
tool results, or a final tool-less assistant), capped at 128 KiB serialized.
Only the older prefix is sent to the same model with the unchanged system
prompt and an empty tool catalog (so compact cannot call tools or attach
images), followed by one appended compaction user message. If that compaction
request would exceed 1 MiB, the oldest prefix messages are dropped until it
fits. The 1 MiB JSON ceiling does not count host-projected image bytes; image
data URLs are a host wire payload, not core history. The returned summary must be non-empty, contain no tool
calls, reduce context size, and fit the existing response and request ceilings.
If it does not, the harness drops oldest prefix messages until the request is
under the compact threshold, instead of aborting the run. Compaction emits live
lifecycle events and does not consume an agent step. A pathological single step
can still exceed the hard context ceiling and fail rather than sending an
oversized request.

Skill catalog text, root `AGENTS.md`, MCP tool schemas, and the latest
world-state context item sit in the stable request prefix on normal turns.
Compaction omits the tool catalog from its auxiliary request. Opening more MCP
tools therefore makes long auto runs worse, not better.

The host currently uses context items for full world-state snapshots. The
latest snapshot is retained across compaction and restored after `/new`.
Changing execution mode appends a new context item while leaving the system
prompt byte-stable.

Host tools add their own effect-side bounds before results reach core:

| Host boundary | Default |
| --- | ---: |
| file read | 128 KiB |
| `read_image` file | 4 MiB; JPEG/PNG/GIF/WebP by magic; 4 images / request; Files API 60s, 7-day expiry; session `attachments/` reloaded on resume and copied on fork |
| `web_fetch` body / extracted text | 128 KiB / 50k characters; 15s; 5 same-class redirects |
| new file or edited file | 1 MiB |
| shell command text | 16 KiB |
| shell runtime | 120 seconds |
| captured foreground stdout and stderr | 8 MiB combined |
| inline foreground result threshold | 16 KiB |
| retained result artifact | 8 MiB in memory; session-backed records retain at most 64 KiB each, 8 entries, 16 MiB total |
| queued REPL operations | 16 |
| root `AGENTS.md` | 16 KiB; UTF-8-safe head and tail if larger; reject if invalid UTF-8 |
| rendered world-state snapshot | 8 KiB; fixed command catalog and capped path |
| durable session file / JSONL record | 32 MiB / 512 KiB |
| listed durable sessions | 128 per workspace under `~/.mini-agent/sessions/` |
| Goal verifier criteria | 32 KiB |
| Goal verifier execution | 1 model step, 0 tool calls |
| discovered skill or compatible plugin instructions | 64; 16 KiB combined metadata catalog |
| skill, plugin, or MCP metadata file | 64 KiB |
| MCP servers | 8 configured stdio or streamable HTTP servers |
| MCP tools | 32 total; 16 KiB input schema per tool |
| MCP connection / tool call | 20 seconds default (120 seconds max) / 120 seconds |
| serialized MCP result | 64 KiB before the core 16 KiB projection |
| HTTP MCP circuit breaker | 3 consecutive failures | 30s cooldown before probe |
| cached session approvals (`ApprovalStore`) | 1024 entries | FIFO/clear on capacity limit |
| repetitive tool-call loop threshold | 2 consecutive identical batches | injects advisory guidance warning |

Shell streams are drained concurrently with a hard capture limit, so a noisy
process cannot accumulate unbounded captured output or deadlock on a full pipe.
Large completed results are retained in the process-local result store and
projected to the model as a bounded preview. On foreground timeout the host
terminates the shell process tree. Shell execution is still not an isolation
boundary. Non-interactive `ask` requires
explicit approval for sensitive tools; the explicitly selected `auto` mode does
not.

Project extension discovery scans only immediate children at fixed locations
and at most 128 directory entries per location. Installed skills, plugins, and
MCP stay inside the workspace. Stdio MCP servers run as local processes with a
small ambient environment allowlist; they are not sandboxed. HTTP MCP connects only to its configured
absolute URL and applies bounded SSE events and tool results.

The OpenAI adapter applies a 10-second connection timeout and a 120-second
deadline to the complete streaming request. It enforces the harness response
byte limit while accumulating text and tool calls, before returning them to
core, and retains at most 4 KiB from an HTTP error body.

Durable sessions are append-only. Interactive, one-shot `ask`, and `auto`
sessions are stored per workspace under `~/.mini-agent/sessions/`, not in the
project tree. The session log contains turns, context items, and stored tool
results. Resume validates strictly
increasing sequence numbers and restores only the newest complete checkpoint.
An incomplete final JSONL line is treated as a torn write and truncated before
new records are appended. One lock file prevents concurrent writers; a stale
lock is never ignored automatically.

Goal verifier analysis restores only the newest settled checkpoint under the same
session lock. It uses the normal 1 MiB context and 64 KiB response ceilings,
rejects any proposed tool call, and appends its result under the existing
512 KiB record and 32 MiB session limits. Its deterministic FNV-1a source
fingerprint is a change-detection aid, not a cryptographic integrity proof; the
monotonic checkpoint sequence is the authoritative source reference.
