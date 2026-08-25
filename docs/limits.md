# Harness limits

Every value sent to or accepted from a model has a direct hard bound. The
defaults are part of the harness rather than terminal flags.

| Boundary | Default | Behavior at limit |
| --- | ---: | --- |
| one host context item | 8 KiB | reject before retaining the item |
| user input | 32 KiB | reject before retaining or tracing the text |
| model response | 384 KiB | reject before retaining text or tool calls |
| tool calls in one model step | 8 | reject the whole proposal before effects |
| one tool result | 16 KiB | retain UTF-8-safe head and tail |
| model request context | 1 MiB | reject before the provider request |
| model steps in one run | 8 | return `step_limit` outcome |

Context size is the byte length of the system prompt plus JSON-serialized
messages and tool specifications. It is a provider-neutral safety ceiling, not
a prediction of provider tokenization. DeepSeek V4 advertises a 1M-token
context and 384K-token output; the harness still caps a request at 1 MiB and a
model step at 384 KiB so one turn cannot fill that window. Provider-reported
token counts remain available separately in model-response events.

Reasoning and assistant text deltas share the model-response ceiling. They stop
reaching observers once the combined response crosses it, and the completed
response is then rejected. The two streams remain distinct in trace events and
terminal output. Settled reasoning is replayed as a Responses API reasoning
item before the same assistant turn's text and tool calls. Tool output is
different: it comes from an already-performed external effect, so the harness
retains a bounded result and marks `truncated` explicitly instead of discarding
the outcome.

Every runtime limit failure emits `run_failed` with a structured
`limit_exceeded` reason. The default behavior does not compact or delete
history behind the user's back; in the interactive terminal, `/new` is the
explicit way to clear a conversation that has reached its context ceiling.

The explicit `auto` mode changes two bounded defaults: it permits 128 model
steps and selects `ContextLimitBehavior::Compact`. It can be selected by
starting `auto` with or without a prompt or by entering `/auto` in an
interactive session; `/auto off` restores the interactive defaults. Before a
normal sampling request, settled history at or above half of the 1 MiB ceiling is sent to the
same model with the unchanged system prompt and tool catalog, followed by one
appended compaction user message. This preserves the previous request prefix for
provider KV-cache reuse. The returned summary must be non-empty, contain no tool
calls, reduce context size, and fit the existing response and request ceilings.
Compaction has its own trace events and does not consume an agent step. A
pathological single step can still exceed the hard context ceiling and fail
rather than sending an oversized request.

The host currently uses context items for full world-state snapshots. The
latest snapshot is retained across compaction and restored after `/new`.
Changing execution mode appends a new context item while leaving the system
prompt byte-stable.

Host tools add their own effect-side bounds before results reach core:

| Host boundary | Default |
| --- | ---: |
| file read | 64 KiB |
| new file or edited file | 1 MiB |
| shell command text | 16 KiB |
| shell runtime | 120 seconds |
| captured foreground stdout and stderr | 8 MiB combined |
| inline foreground result threshold | 16 KiB |
| stored result | 8 MiB each, 8 entries, 16 MiB total |
| `read_tool_result` response | 16 KiB |
| managed processes | 8 |
| managed-process log | 256 KiB per stream |
| queued REPL operations | 16 |
| root `AGENTS.md` | 64 KiB; UTF-8-safe head and tail if larger; reject if invalid UTF-8 |
| rendered world-state snapshot | 8 KiB; fixed command catalog and capped path |
| durable session file / JSONL record | 32 MiB / 512 KiB |
| listed durable sessions | 128 project-local files |
| mentor verification criteria | 32 KiB |
| mentor execution | 1 model step, 0 tool calls |
| discovered skill or compatible plugin instructions | 64; 16 KiB combined metadata catalog |
| skill, plugin, or MCP metadata file | 64 KiB |
| marketplace manifest | 256 KiB |
| marketplaces / enabled selectors per marketplace | 16 / 32 |
| MCP servers | 8 configured stdio or streamable HTTP servers |
| MCP tools | 32 total; 16 KiB input schema per tool |
| MCP connection / tool call | 20 seconds default (120 seconds max) / 120 seconds |
| serialized MCP result | 64 KiB before the core 16 KiB projection |

Shell streams are drained concurrently with a hard capture limit, so a noisy
process cannot accumulate unbounded captured output or deadlock on a full pipe.
Large completed results are retained in the process-local result store and
projected to the model as a bounded preview plus a handle. Managed process logs
retain bounded head and tail while their pipes continue to drain; at most eight
process records exist, and dropping the host stops any remaining process trees.
On foreground timeout the host terminates the shell process tree. Process
execution is still not an isolation boundary. Interactive and `run` modes
require approval; the explicitly selected `auto` mode does not.

Project extension discovery scans only immediate children at fixed locations
and at most 128 directory entries per location. Filesystem-resolved skill,
plugin, and marketplace paths must remain inside their package or workspace
root. Stdio MCP servers run as local processes with a small ambient environment
allowlist; they are not sandboxed. HTTP MCP connects only to its configured
absolute URL and applies bounded SSE events and tool results.

The OpenAI adapter applies a 10-second connection timeout and a 120-second
deadline to the complete streaming request. It enforces the harness response
byte limit while accumulating text and tool calls, before returning them to
core, and retains at most 4 KiB from an HTTP error body.

Durable sessions are opt-in and append-only. Resume validates strictly
increasing sequence numbers and restores only the newest complete checkpoint.
An incomplete final JSONL line is treated as a torn write and truncated before
new records are appended. One lock file prevents concurrent writers; a stale
lock is never ignored automatically.

Mentor analysis restores only the newest settled checkpoint under the same
session lock. It uses the normal 1 MiB context and 384 KiB response ceilings,
rejects any proposed tool call, and appends its result under the existing
512 KiB record and 32 MiB session limits. Its deterministic FNV-1a source
fingerprint is a change-detection aid, not a cryptographic integrity proof; the
monotonic checkpoint sequence is the authoritative source reference.
