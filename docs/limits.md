# Harness limits

Every value sent to or accepted from a model has a direct hard bound. The
defaults are part of the harness rather than terminal flags.

| Boundary | Default | Behavior at limit |
| --- | ---: | --- |
| user input | 32 KiB | reject before retaining or tracing the text |
| model response | 64 KiB | reject before retaining text or tool calls |
| tool calls in one model step | 8 | reject the whole proposal before effects |
| one tool result | 16 KiB | retain UTF-8-safe head and tail |
| model request context | 256 KiB | reject before the provider request |
| model steps in one run | 8 | return `step_limit` outcome |

Context size is the byte length of the system prompt plus JSON-serialized
messages and tool specifications. It is a provider-neutral safety ceiling, not
a prediction of provider tokenization. Provider-reported token counts remain
available separately in model-response events.

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
normal sampling request, settled history at or above half of the 256 KiB ceiling is sent to the
same model with the unchanged system prompt and tool catalog, followed by one
appended compaction user message. This preserves the previous request prefix for
provider KV-cache reuse. The returned summary must be non-empty, contain no tool
calls, reduce context size, and fit the existing response and request ceilings.
Compaction has its own trace events and does not consume an agent step. A
pathological single step can still exceed the hard context ceiling and fail
rather than sending an oversized request.

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
| root `AGENTS.md` | 16 KiB; reject if larger or invalid UTF-8 |

Shell streams are drained concurrently with a hard capture limit, so a noisy
process cannot accumulate unbounded captured output or deadlock on a full pipe.
Large completed results are retained in the process-local result store and
projected to the model as a bounded preview plus a handle. Managed process logs
retain bounded head and tail while their pipes continue to drain; at most eight
process records exist, and dropping the host stops any remaining process trees.
On foreground timeout the host terminates the shell process tree. Process
execution is still not an isolation boundary. Interactive and `run` modes
require approval; the explicitly selected `auto` mode does not.

The OpenAI adapter applies a 10-second connection timeout and a 120-second
deadline to the complete streaming request. It enforces the harness response
byte limit while accumulating text and tool calls, before returning them to
core, and retains at most 4 KiB from an HTTP error body.
