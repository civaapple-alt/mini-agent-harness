# Harness limits

Every value sent to or accepted from a model has a direct hard bound. The
defaults are part of the harness rather than terminal flags.

The Rust line-budget report from `python3 scripts/line_budget.py` uses effective
code lines: blank lines and comment-only lines, including documentation and
multi-line block comments, are excluded. A line containing code and a trailing
comment still counts once. Production, unit-test, and integration-test totals
use the same effective-line rule.

## Rust source budget

预算按互斥文件分类统计；同一个文件只能进入一个 category。当前硬门禁是：

| 指标 | 统计范围 | 硬上限 |
| --- | --- | ---: |
| Core + Protocol | `mini-agent-core` + `mini-agent-protocol` | 7,000 |
| Control Plane | Host/App Server control slice + Capabilities control slice | 45,000 |
| Release Rust source | 支持的运行时 crate 与测试，排除实验性 CLI/REPL | 65,000 |

Runtime 聚合值仍可在 JSON 中用于诊断，但不再设置 `25,000` 行硬门禁。每个 PR 的
Release Rust 净增量 `1,000` 行是审查参考值，不是硬门禁。Core + Protocol、Control
Plane 和 Release Rust 执行绝对硬上限；增量检查使用 `--check-delta` 报告变化。

当前 revision 的有效行数为：

```text
core+protocol    6519/7000
control-plane   40729/45000
release         58732/65000
```

运行 `python3 scripts/line_budget.py` 可重新计算这些数字。使用
`--base <merge-base> --check-delta` 可查看三项增量。超过建议值只提示，不失败。
使用 `--verbose` 查看 crate 和 production、unit、integration 拆分。使用 `--json`
获取完整的 categories、layers、limits、status 和 violations。详细统计用于诊断，不能
绕过硬门禁或结构性验收。

行数门禁之外，交付门禁还要求受影响 Rust 包测试、Clippy、fmt、Cargo boundary 检查
通过；如果变更影响 prompt、tool schema、loop-control、context、event 或 persistence，
还必须提供 bounded Harness Scenario/Eval。这样可以避免用减少测试、压缩边界类型或把
Control Plane 责任塞进 Thin Loop 的方式“通过”预算。

| Boundary | Default | Behavior at limit |
| --- | ---: | --- |
| one ordinary host context item | 8 KiB by default | reject before retaining the item |
| one metadata-backed Host context injection | 64 KiB | reject before retaining the source |
| Skill inventory context | 16.5 KiB | reject before retaining the catalog snapshot |
| activated Skills context | 33.5 KiB | reject before retaining the turn's selected Skill bodies |
| user input | 32 KiB | reject before retaining or emitting the text |
| queued steering/follow-up input | 16 items, 64 KiB per item, 512 KiB total | reject before retaining the item |
| JSON-RPC input line | 2 MiB including the line ending | close the stream before deserialization |
| model response | 16 MiB | reject before retaining text or tool calls |
| tool calls in one model step | 8 | reject the whole proposal before effects |
| one tool result inline in model context | 16 KiB | retain UTF-8-safe head and tail; larger Host results include a Session artifact handle |
| model request context | 64 MiB | reject before the provider request |
| one Session journal record | 128 MiB | reject before appending the record |
| one Session journal file | 256 MiB | reject before appending the record |
| persisted ThreadItem text projection | 16 KiB | truncate the client-facing text |
| model steps in one run | internal safety guard | report a runtime-protection diagnostic; never treat it as a user task setting |
| Goal milestone model steps | 200 by default | Goal becomes `usageLimited` after settling evidence |
| Goal milestone wall-clock time | 1,800 seconds by default | request cooperative cancellation, then `usageLimited` |
| Goal continuation loops | 100 by default | Goal stops scheduling further continuation |
| Goal token budget | unset by default | accumulated provider input/output usage becomes `budgetLimited` |

Context size is the byte length of the system prompt plus JSON-serialized
messages and tool specifications. It is a provider-neutral safety ceiling, not
a prediction of provider tokenization. A model profile's `contextWindow` is
token metadata for Studio's usage gauge; the selected model and provider decide
the actual token window. The Host can send at most 64 MiB of serialized context.
The profile's `maxOutputTokens` is sent as `max_output_tokens`; the Core accepts
at most 16 MiB of response data. Provider limits still apply. Provider-reported
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
history behind the user's back. The current interactive terminal has no `/new`
command: exit it and start `mini-agent` without `--session-id` to create a new
conversation, or use `resume` to continue a settled Session.

Core keeps an internal runaway-loop guard and may compact context when the
runtime composition allows it. `max_steps` and `step_limit` are not direct
Goal progress semantics. Web Studio exposes them through the explicit Thread
`continuationMode`: `manual` keeps the default 8-step bound, while `continuous`
sets `max_steps=0` for ordinary Chat. Goal's long-running behavior is owned by
the Goal Runtime and temporarily uses its own milestone budget; Auto Copilot is
the explicit `trusted + continuous` Web Studio preset, while each grant is still
bounded by its action key and selected scope. Before a normal sampling request,
settled history at or above half of the 64 MiB ceiling
is compacted. The newest context item and a bounded recent tail stay verbatim:
the last two model-step groups (each an assistant message plus its following
tool results, or a final tool-less assistant), capped at 128 KiB serialized.
Only the older prefix is sent to the same model with the unchanged system
prompt and an empty tool catalog (so compact cannot call tools or attach
images), followed by one appended compaction user message. If that compaction
request would exceed 64 MiB, the oldest prefix messages are dropped until it
fits. The 64 MiB JSON ceiling does not count host-projected image bytes; image
data URLs are a host wire payload, not core history. The returned summary must be non-empty, contain no tool
calls, reduce context size, and fit the existing response and request ceilings.
If it does not, the harness drops oldest prefix messages until the request is
under the compact threshold, instead of aborting the run. Compaction emits live
lifecycle events and does not consume an agent step. Each live compaction
start/finish pair carries a bounded item identity; Studio may group adjacent
completed entries as “上下文压缩 ×N” while retaining turn/item detail. A pathological single step
can still exceed the hard context ceiling and fail rather than sending an
oversized request.

The stable system prompt and tool definitions remain unchanged when dynamic
context changes. Project instructions, Skill catalog and bodies, workspace
state, and other dynamic context are appended to Session history in occurrence
order. An unchanged source fingerprint is not appended again; a changed source
is appended with the fingerprint it supersedes, leaving earlier messages
intact. Compaction keeps the latest effective context for each source slot.

This preserves the existing request prefix when context is added later, which
can help provider prompt caching. A changed message can still affect the
provider's cache boundary, and tokenization and cache policy vary by provider;
the App Server reports actual input and cached-input usage when the provider
returns it. Web Studio estimates category token counts by byte share and labels
them as estimates. Cached tokens are shown only as a provider-reported total.
The model context window and provider usage are shown as unknown when their
metadata is unavailable. Compaction omits the tool catalog from its auxiliary
request. Opening more MCP tools therefore makes long Goal runs worse, not better.

The Host stores world-state snapshots as append-only context messages. A changed
snapshot supersedes the previous version; compaction retains the latest
effective snapshot. A resumed Session rebuilds the source inventory from its
checkpoint. The stable system prompt does not change.

Host tools add their own effect-side bounds before results reach core:

| Host boundary | Default |
| --- | ---: |
| file source read | 8 MiB; UTF-8 text only |
| `read_file` page | 200 lines by default, 2,000 maximum, 15 KiB rendered page |
| `read_image` file | 4 MiB; JPEG/PNG/GIF/WebP by magic; 4 images / request; Files API 60s, 7-day expiry; session `attachments/` reloaded on resume and copied on fork |
| `web_fetch` response / extracted text | 8 MiB; 15s; at most 5 same-host, same-class redirects |
| `web_fetch` inline result page | 8 KiB; longer results continue from the Session cache by handle and cursor |
| Session-backed result cache | 8 MiB per result; at most 8 entries; 16 MiB total Session result data |
| `web_search` request and results | query 2,000 bytes; up to 10 results; URL 2,000 bytes; title 256 chars; snippet 640 chars; provider response 1 MiB / 35s |
| new file or edited file | 1 MiB |
| shell command text | 16 KiB |
| shell runtime | 120 seconds |
| captured foreground stdout and stderr | 8 MiB combined |
| inline foreground result threshold | 16 KiB |
| retained result artifact | 8 MiB per result; 8 entries, 16 MiB total; Session content is stored in sidecars |
| queued REPL operations | 16 |
| `AGENTS.md` source | 16 KiB per file; at most 16 workspace roots and 16 applicable files; 256 KiB aggregate; UTF-8-safe head and tail if larger; reject if invalid UTF-8 |
| rendered world-state snapshot | 8 KiB; fixed command catalog and capped path |
| durable session file / JSONL record | 256 MiB / 128 MiB |
| listed durable sessions | 128 per workspace under `~/.mini-agent/sessions/` |
| Goal verifier criteria | 32 KiB |
| Goal verifier execution | 1 model step, 0 tool calls |
| discovered skill or compatible plugin instructions | 64; 16 KiB combined metadata catalog |
| explicitly activated Skill bodies | 8 Skills; 32 KiB combined per Turn |
| Skill directory reads | 64 KiB rendered `read_file` output per Turn; existing page limits still apply |
| skill, plugin, or MCP metadata file | 64 KiB |
| MCP servers | 8 configured stdio or streamable HTTP servers |
| MCP tools | 32 total; 16 KiB input schema per tool |
| MCP connection / tool call | 20 seconds default (120 seconds max) / 120 seconds |
| serialized MCP result | 64 KiB before the core 16 KiB projection |
| HTTP MCP circuit breaker | 3 consecutive failures | 30s cooldown before probe |
| cached action grants (`ApprovalStore`) | 1024 entries | bounded by scope/owner/action key/revision; overflow requires a new approval |
| repetitive tool-call loop threshold | 2 consecutive identical batches | injects advisory guidance warning |

Shell streams are drained concurrently with a hard capture limit, so a noisy
process cannot accumulate unbounded captured output or deadlock on a full pipe.
Large completed results are retained in the process-local result store and
projected to the model as a bounded preview. On foreground timeout the host
terminates the shell process tree. Shell execution is still not an isolation
boundary. Under Automatic policy, explicitly read-only shell inspection is
admitted without approval only when referenced paths stay inside the workspace
or configured read roots; dynamic paths, writes, high-risk commands, and
outside paths retain the typed approval path. A runtime access scope never
becomes a global allow-all switch. `FullMachine` widens file path scope but does
not override hard Deny, Plan-mode source-file mutation locks, unavailable tools,
or high-risk shell confirmation. Shell itself remains governed by the selected
approval policy in Plan mode; Plan does not add a separate read-only Shell
restriction.

## Shell execution contract

Foreground Shell execution uses one bounded local-development contract for
Native and Docker backends:

- The process starts with the configured workspace as its working directory.
  Docker mounts that workspace at `/workspace`; the container's writable layer
  is discarded when the command exits.
- A foreground command has a 120-second deadline and at most 8 MiB of combined
  captured stdout and stderr. Larger output is truncated to a bounded head and
  tail and may be read through the bounded result artifact.
- Cancellation and timeout terminate the complete Native process tree. Docker
  runs use a unique container name; the App Server stops the Docker client and
  then kills and verifies the named container. If cleanup cannot be confirmed,
  the Shell call returns an error.
- Docker is selected explicitly. If its daemon is unavailable or the container
  cannot start, the command fails without falling back to Native.

This is not an adversarial-code sandbox. Native Shell runs with the user's OS
permissions and can access paths available to that user; configured file-tool
workspace roots remain enforced at their own Host boundary. Docker is intended
for local development isolation only and does not promise resistance to code
that can attack the Docker daemon or kernel. Docker unavailability must not be
treated as permission to execute the same command natively.

Project extension discovery scans only immediate children at fixed locations
and at most 128 directory entries per location. Installed skills, plugins, and
MCP stay inside the workspace. Stdio MCP servers run as local processes with a
small ambient environment allowlist; they are not sandboxed. HTTP MCP connects only to its configured
absolute URL and applies bounded SSE events and tool results.

The OpenAI adapter applies a 10-second connection timeout and a 120-second
deadline to the complete streaming request. It enforces the harness response
byte limit while accumulating text and tool calls, before returning them to
core, and retains at most 4 KiB from an HTTP error body.

Durable App Server sessions are append-only. Sessions are stored per workspace
under `~/.mini-agent/sessions/`, not in the
project tree. The session log contains turns, context items, and stored tool
results. Resume validates strictly
increasing sequence numbers and restores only the newest complete checkpoint.
An incomplete final JSONL line is treated as a torn write and truncated before
new records are appended. One lock file prevents concurrent writers; a stale
lock is never ignored automatically.

Goal verifier analysis restores only the newest settled checkpoint under the same
session lock. It uses the normal 64 MiB context and 16 MiB response ceilings,
rejects any proposed tool call, and stores a bounded 32 KiB result in the Goal
workspace without appending verifier output to the primary session history.
The monotonic checkpoint sequence is the authoritative source reference.
