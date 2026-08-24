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

Assistant text deltas stop reaching observers once the response ceiling is
crossed; the completed response is then rejected. Tool output is different:
it comes from an already-performed external effect, so the harness retains a
bounded result and marks `truncated` explicitly instead of discarding the
outcome.

Every runtime limit failure emits `run_failed` with a structured
`limit_exceeded` reason. No automatic compaction, history deletion, or policy
callback runs behind the user's back. In the terminal, `/new` is the explicit
way to clear a conversation that has reached its context ceiling.

Host tools add their own effect-side bounds before results reach core:

| Host boundary | Default |
| --- | ---: |
| file read | 64 KiB |
| new file or edited file | 1 MiB |
| shell command text | 16 KiB |
| shell runtime | 120 seconds |
| retained shell stdout and stderr | 64 KiB combined |

Shell streams are drained concurrently while retaining bounded head and tail,
so a noisy process cannot accumulate unbounded captured output or deadlock on
a full pipe. On timeout the host terminates the shell process tree. Shell is
still not an isolation boundary; user approval remains mandatory.

The OpenAI adapter applies a 10-second connection timeout and a 120-second
deadline to the complete streaming request. It enforces the harness response
byte limit while accumulating text and tool calls, before returning them to
core, and retains at most 4 KiB from an HTTP error body.
