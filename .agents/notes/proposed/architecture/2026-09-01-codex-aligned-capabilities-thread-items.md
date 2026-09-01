# Codex-Aligned Skills, Plugins, Builtin Tools, and ThreadItems

Status: proposed

## Proposal

Align mini-agent-harness with the current Codex App Server semantic model:

```text
Thread
└── Turn[]
    └── ThreadItem[]
```

The alignment covers four related concerns:

1. `Skill` describes a repeatable workflow and its tool dependencies.
2. `Plugin` packages Skills, MCP servers, and optional resources or UI.
3. Builtin, Host, MCP, and Dynamic tools expose concrete `ToolSpec` entries and
   enter the same Router → Handler → Orchestrator → Runtime execution path.
4. `ThreadItem` is the durable/public projection of a user-visible turn unit;
   large outputs remain sidecar artifacts referenced by an Item.

This remains a design proposal for the full Thread/Turn/ThreadItem alignment.
Its initial exposure policy is implemented as two small Stage 1 batches: the
default Builtin catalog is limited to six tools, and the active Thread can
reversibly select a subset through `thread/settings/update`. The first bounded
Stage 2 Skill slice now parses typed `builtin`/`mcp` dependencies and returns
explicit local activation metadata; App Server Turn activation and Host
allowlist resolution remain deferred until their own evidence is accepted.

## Why this direction

The current mini-codex implementation already has useful pieces, but its public
concepts are not yet aligned:

| Concern | Current mini-codex | Proposed direction |
|---|---|---|
| Skill | Bounded discovery and metadata prompt; model reads `SKILL.md` with `read_file` | Explicit Skill metadata, dependency declaration, and turn-level activation |
| Plugin | Discovered package name and MCP configuration | Package metadata plus selected Provider inputs; no direct arbitrary code execution |
| Builtin tools | One `BuiltinToolProvider` assembles the workspace/web/image tools | One catalog of first-party ToolSpecs with per-tool exposure and origin; only six are visible by default |
| Tool execution | `ToolRouter` delegates to Host `ToolOrchestrator`; typed admission is in place for sensitive tools | Preserve this ownership and make every provider use it |
| History | Core `Event` and `messages`; App Server `turn/event` projection | Add a Codex-shaped `Turn.items` projection without moving Session authority |
| Large results | `ResultStore` handles and ImageStore attachments | Keep existing stores, then expose bounded `ArtifactRef` from ThreadItems |

The important replacement is semantic, not cosmetic: Skill and Plugin choose or
describe capabilities; they do not become a second execution framework.

## Reference findings from current Codex

The current Codex source and App Server documentation establish the following
shape:

- App Server exposes `Thread` objects containing `Turn` objects, and a `Turn`
  contains `ThreadItem` values.
- `ThreadItem` has specialized variants such as user/agent messages,
  reasoning, command execution, file changes, MCP calls, dynamic tool calls,
  image view, and context compaction.
- `item/started` and `item/completed` carry the same Item through its lifecycle;
  specialized delta notifications update a running Item.
- `thread/items/list` reads persisted Items independently from a loaded runtime.
- `turn/start` can explicitly activate a Skill through a Skill input item.
- `thread/start` can accept experimental Dynamic Tools and persist them with the
  Thread for resume.
- A Plugin is a package that can contain Skills, an MCP server, and optional UI;
  the Plugin package is not itself a generic executable Tool.

Primary references:

- Codex App Server: <https://developers.openai.com/codex/app-server>
- Plugin architecture: <https://developers.openai.com/plugins/concepts/plugin-architecture>
- Skills: <https://developers.openai.com/plugins/concepts/skills>
- mini-codex Skill discovery: `crates/mini-agent-capabilities/src/skills.rs`
- mini-codex Provider registry: `crates/mini-agent-capabilities/src/registry.rs`
- mini-codex Tool contract: `crates/mini-agent-protocol/src/tool.rs`

The proposal intentionally does not copy every Codex variant. It adopts the
semantic boundaries and only adds an Item variant when mini-codex has a real
capability and a client use case for it.

## Target ownership model

### Thread

`Thread` owns the durable conversation identity and thread-level capability
selection:

- Thread id, session identity, status, and settings;
- selected Skills and Plugins;
- selected Builtin/Host/MCP/Dynamic capability references;
- ordered Turns and their persisted Item projections.

Thread settings select capabilities, but do not contain secrets, live process
handles, approval callbacks, or unbounded Skill bodies.

### Turn

`Turn` represents one execution cycle:

- user submission, follow-up, steer, or automatic continuation;
- model steps and stop status;
- ordered ThreadItems;
- bounded error and usage information.

Goal Runtime, when implemented, schedules Turns. It must not introduce a second
conversation loop or a second Item history.

### ThreadItem

`ThreadItem` is the stable, user/client-visible semantic record. An Item has a
stable id and is updated from in-progress to a terminal state rather than being
replaced by an unrelated result record.

The initial mini-codex subset should be:

```text
UserMessage
AgentMessage
Reasoning
CommandExecution
FileChange
McpToolCall
DynamicToolCall
ImageView
ContextCompaction
```

Do not add collaboration-agent, review-mode, sleep, or other Codex variants
until mini-codex has the corresponding capability and public evidence.

Suggested mapping:

| Item | Primary fields |
|---|---|
| `UserMessage` | `id`, bounded content |
| `AgentMessage` | `id`, bounded text, optional phase |
| `Reasoning` | `id`, bounded summary/content according to exposure policy |
| `CommandExecution` | `id`, command, cwd, source, status, bounded output, exit code, duration |
| `FileChange` | `id`, bounded path/diff records, status |
| `McpToolCall` | `id`, server, tool, bounded arguments, status, bounded result/error |
| `DynamicToolCall` | `id`, namespace, tool, bounded arguments, status, bounded content |
| `ImageView` | `id`, attachment or artifact reference |
| `ContextCompaction` | `id`, lifecycle marker only |

For tool Items, `item.id` should initially equal the model `callId`. This keeps
the existing `callId` correlation and avoids introducing a second opaque identity
without a demonstrated need.

### Event projection

Core events remain the execution source and Session remains the durable authority.
The App Server maps them to the Codex-shaped Item stream:

```text
Core/Host event
  → App Server item/started
  → optional item-specific deltas
  → App Server item/completed
  → Session checkpoint
```

The completed Item is authoritative. A delta is progress, not a replacement
history record. `TurnRead` and `ThreadRead` may first expose an Item projection
alongside existing fields; any breaking replacement belongs in a separately
approved protocol batch.

## Skill contract

### Responsibilities

A Skill may provide:

- bounded metadata (`name`, `description`, scope, location);
- an instruction document and package-relative resources;
- optional interface/default prompt metadata;
- declared tool dependencies.

A Skill must not:

- execute a process while being discovered or selected;
- own approval, sandbox, Session, Thread, or Turn state;
- inject its entire body or resources into every model request;
- silently enable a provider that was not selected by the Thread policy.

The model-visible form should remain progressive disclosure:

```text
Thread settings select Skill
  → bounded metadata enters the prompt
  → Skill is explicitly activated for a Turn
  → Host resolves declared dependencies
  → only selected ToolSpecs enter the model request
```

The current `SKILL.md` frontmatter discovery can remain the first format. A
future optional metadata block may add a Codex-shaped dependency section:

```yaml
dependencies:
  tools:
    - type: builtin
      value: read_file
    - type: mcp
      value: github
```

Dependency types are declarative references, not permission grants. The Host
must resolve each reference against an allowlisted registry and policy.

## Plugin contract

### Package shape

A project Plugin may contain:

```text
.agents/plugins/<plugin>/
├── plugin.json
├── skills/
├── .mcp.json
├── resources/
└── optional UI metadata
```

The exact manifest format remains subject to the existing extension schema. The
important contract is that discovery returns bounded metadata and provider
inputs; it does not load arbitrary executable code into Core.

### Provider mapping

Plugin-provided capabilities use existing execution categories:

| Plugin content | Runtime category | Item category |
|---|---|---|
| trusted local command integration | Host/Builtin provider | `CommandExecution` |
| MCP server tool | MCP provider | `McpToolCall` |
| App Server supplied callback tool | Dynamic provider | `DynamicToolCall` |
| package image/resource | Attachment/Artifact store | `ImageView` or referenced result |
| Skill instructions | Skill catalog | no Tool Item by itself |

There must not be a Plugin-specific hidden execution path. A Plugin tool goes
through the same Handler admission, Host approval, sandbox selection, Runtime
effect, Item completion, and Session writeback path as a Builtin tool.

## Builtin and external Tool model

### Tool origin and exposure

The implementation should distinguish three independent properties:

```text
Tool origin:   Builtin | Host | Mcp | Dynamic
Tool exposure: Visible | Hidden | Disabled
Tool admission: ReadOnly | ApprovalRequired | Forbidden
```

Origin identifies ownership; exposure controls the model-visible manifest;
admission controls execution policy. They must not be collapsed into one
`ToolScope::All | None` value.

The current `CapabilityRegistry::ToolProvider` remains a useful construction
seam. The next design should add a bounded catalog or equivalent Host-owned
selection layer so one Builtin Provider does not imply that every Builtin Tool is
visible in every Thread.

### Initial exposure policy

The intended default is intentionally small. The first model-visible Builtin
catalog contains exactly these six tools:

```text
Builtin default:
  read_file
  edit_file
  write_file
  shell
  web_fetch
  read_image
```

The following are deliberately not in the default model-visible catalog:

```text
Removed Builtin tools:
  process_start/read/write/stop/list
  read_tool_result

Explicit extension capabilities:
  MCP tools
  Dynamic Tools
  Plugin-provided command integrations
```

This selection policy is now the default provider behavior and the removed tools
have no remaining Builtin implementation. `ResultStore` remains only as an
internal bounded sidecar for Shell/Web results; large result continuation is a
future Artifact-sidecar concern. Sensitive Builtin tools remain approval-gated
even when auto-approved; auto-approval is a decision record, not absence of the
admission lifecycle.

### Unified call lifecycle

```text
model samples ToolCall
  → append/start ThreadItem with itemId = callId
  → ToolRouter.resolve(name)
  → ToolHandler.parse + describe admission
  → ToolOrchestrator.approval + sandbox + lifecycle
  → ToolRuntime.execute
  → append/complete same ThreadItem
  → Core writes bounded result to conversation history
  → Session persists settled checkpoint
```

The ownership rules remain:

- Router resolves a name and does not approve;
- Handler parses arguments and describes tool-specific admission;
- Orchestrator owns generic approval, sandbox choice, lifecycle, and outcome;
- Runtime owns the side effect;
- Core owns Turn control, events, and model-history writeback;
- App Server owns public requests, notifications, and client projection.

## Approval and correlation

An approval request is a lifecycle request, not a new `ThreadItem` variant.
The public correlation is:

```text
threadId → turnId → itemId/callId → requestId
```

For every approval-gated call:

1. `item/started` is observable before waiting when the operation has begun;
2. the approval request identifies `threadId`, `turnId`, `itemId/callId`, and
   `requestId`;
3. the response is recorded as approved, auto-approved, denied, timed out, or
   cancelled according to the bounded protocol vocabulary;
4. the Runtime is never entered after denial;
5. `item/completed` contains a non-empty structured failure/rejection reason;
6. Session checkpoint and the next model input retain the settled result.

There should be no second approval authority in Plugin, MCP, or a Skill. MCP
server startup approval and MCP tool-call approval may remain distinct admission
events, but both are owned by Host policy and are correlated to the relevant
Thread/Turn where available.

## Stage 1 admission record: default Builtin catalog

This batch applies the six-question gate to the first exposure-policy change:

1. **Layer:** Capabilities owns the Builtin catalog assembly and Host owns the
   default profile manifest. No Core, App Server protocol, SDK, or client schema
   change is required.
2. **Duplicate responsibility:** Reuse `CapabilityRegistry`, the existing
   workspace tool factory, `ToolRouter`, `ToolOrchestrator`, `ResultStore`, and
   `ImageStore`. No second router, executor, approval authority, or result store
   is introduced.
3. **Replace vs. add:** Delete the managed-process implementation and the
   `ReadToolResult` wrapper instead of retaining dead tools. Reuse `ResultStore`
   as an internal Shell/Web sidecar; no alternate provider is added.
4. **Net line delta:** The measured post-change baseline is runtime
   `16,939/20,000` and all Rust `28,907/30,000`. Direct removal releases 869
   all-Rust lines and leaves 1,093 lines; future work still needs a measured
   offset before code.
5. **Visible surface:** The default manifest now exposes exactly
   `read_file`, `edit_file`, `write_file`, `shell`, `web_fetch`, and `read_image`.
   Process tools and `read_tool_result` are removed. The App Server manifest no
   longer advertises `rulePolicy.processExecution`; there is no new Item or
   approval correlation.
6. **Boundary evidence:** `mini-agent-capabilities` (61 tests),
   `mini-agent-host` (43 tests), and `mini-agent-app-server` (32 tests) pass.
   The capabilities suite asserts the exact six-tool catalog; Host and App Server
   suites cover profile exposure, approval, JSON-RPC, and lifecycle behavior.

### Stage 1 admission record: Host-owned Tool Catalog slice

1. **Layer:** Host owns the catalog metadata and applies it only to the selected
   Builtin provider; Capabilities still constructs concrete tools. No Core or
   public App Server protocol change is needed.
2. **Duplicate responsibility:** Reuse the existing provider construction,
   `ToolRouter`, `ToolOrchestrator`, and Handler admission. `ToolCatalog` does
   not resolve names for execution or introduce another lifecycle.
3. **Replace vs. add:** Replace implicit Builtin visibility with an explicit
   six-entry Host catalog. Explicitly registered non-Builtin providers pass
   through unchanged until Thread-level selection is implemented.
4. **Net line delta:** Before this slice: runtime `16,939/20,000`, all Rust
   `28,907/30,000`; after: runtime `17,149/20,000`, all Rust `29,117/30,000`.
   The slice adds 210 Rust lines and leaves 883 lines of whole-workspace margin.
5. **Visible surface:** The six default Builtin names remain unchanged; their
   origin, provider, exposure, and coarse admission metadata are now explicit.
   No Item, persistence, approval-correlation, or JSON-RPC field is added.
6. **Boundary evidence:** Host tests cover the typed six-entry catalog and
   removal of unlisted names; Capabilities, Host, App Server, and CLI suites
   remain green. Thread-level hidden/disabled selection is covered by the
   subsequent bounded settings batch.

### Stage 1 admission record: Thread-level Builtin selection

1. **Layer:** App Server owns the `thread/settings/update` request and runtime
   action; Host owns Builtin selection validation and hidden-name calculation;
   Core only applies the visibility filter inside its existing `ToolRouter`.
2. **Duplicate responsibility:** Reuse the existing Thread, Harness, Router,
   Host Catalog, Runtime Actor, and `collaborationMode` action. No Skill,
   Plugin, ThreadItem, Artifact, or second execution path is introduced.
3. **Replace vs. add:** Replace the Router's implicit all-visible behavior with
   a reversible hidden-name filter. Tool implementations stay resident so a
   later settings update can widen the selection; external/MCP names remain
   outside the Builtin filter.
4. **Net line delta:** Before this slice: runtime `17,149/20,000`, all Rust
   `29,117/30,000`; after: runtime `17,343/20,000`, all Rust `29,311/30,000`.
   The measured delta is `+194` lines, leaving `689` whole-Rust lines.
5. **Visible surface:** v2 `thread/settings/update` accepts optional
   `builtinTools`. Omission keeps the current selection; an empty array hides
   all six Builtin tools; invalid, duplicate, and over-limit names are rejected.
   The result returns the effective selection. No arbitrary tool execution,
   approval bypass, or public system-prompt replacement is added.
6. **Boundary evidence:** Core verifies hidden tools are absent from the model
   spec and return an unknown-tool result, then become visible again. Host
   verifies bounded selection and hidden-name calculation. The App Server
   public workflow/settings scenario verifies the JSON-RPC field and returned
   selection; affected tests and Clippy pass.

This is intentionally a live runtime slice, not the final persistence contract:
the active Thread retains its filter across ordinary turns and the existing
same-object resume path, while factory-created/forked Threads start from their
Host default until their own settings are applied. Persisting selected
capability references as part of a Thread checkpoint belongs with the later
Thread/ThreadItem persistence batch.

## Artifact and result contract

Codex's current public App Server model does not require a generic Artifact
ThreadItem. mini-codex should therefore treat an Artifact as a sidecar payload
referenced by a ThreadItem:

```text
ThreadItem
└── bounded inline result
    └── optional resultHandle / attachmentRef
```

The internal reference may contain:

```text
artifactId
kind
name
mediaType
size
digest
preview
sourceItemId
```

Rules:

- `ResultStore` remains the first implementation for large text results;
- `ImageStore` and existing attachments remain the first implementation for
  images;
- a future `ArtifactStore` may unify these stores behind the reference shape;
- raw bytes do not enter Thread history by default;
- previews and model-visible fragments remain bounded;
- artifact ownership, retention, redaction, overwrite behavior, and fork/resume
  semantics must be specified before public Artifact APIs are added.

An Artifact reference is not permission to read or execute its contents. Reading
an artifact remains a separate bounded capability and must honor the Thread's
workspace and approval policy.

## App Server protocol shape

### Existing methods to preserve and extend

The design uses the existing App Server lifecycle rather than adding a separate
tool session protocol:

```text
thread/start
thread/resume
thread/read
thread/settings/update
turn/start
turn/steer
turn/interrupt
```

Candidate additions or extensions for v2 are:

- `Thread` settings containing selected Skill, Plugin, and provider IDs;
- `Turn` containing `items` and an explicit `itemsView`;
- `item/started` notification;
- `item/completed` notification;
- item-specific bounded delta notifications;
- experimental `thread/items/list` with cursor, limit, and optional `turnId`;
- an eventual bounded Artifact read/list method only after storage semantics are
  proven.

No client-facing `tool/execute` method should be added for model tool calls.
That would create a second execution path beside `turn/start`.

No public arbitrary `systemPrompt` replacement is implied. Skill instructions,
Plugin metadata, and selected ToolSpecs are composed through the existing
allowlisted Thread/Host configuration seams.

### Persistence

The existing Session JSONL remains the source of durable settled history. A
ThreadItem projection may be derived from or stored with the same checkpoint, but
it must not introduce a second authoritative conversation log.

The persistence rules are:

- append-only settled records;
- no history rewrite during compaction;
- no replay of un-settled side effects;
- bounded Item fields and bounded error text;
- fork copies only the settled history and explicitly owned attachments;
- resumed Threads restore selected capabilities only after policy validation.

## Migration plan

Each stage is an independent small batch. Exact line deltas are measured before
and after; no estimate authorizes a budget breach.

### Stage 0: proposal and budget gate

- Land this proposal and update notes/index/changelog.
- Release whole-Rust budget through the current P1 cleanup path.
- Do not add public DTOs, Item enums, or generic Artifact storage before the
  whole-Rust margin is materially larger than the planned batch.

### Stage 1: Tool Catalog and exposure

- Landed the first Host-owned bounded catalog over existing ToolSpecs.
- Record origin, exposure, admission class, provider, and stable name for the
  six default Builtin entries.
- Filter the default Builtin provider through that catalog.
- Apply a bounded Builtin subset to the active Thread through
  `thread/settings/update`; keep the selection reversible and preserve
  explicitly registered external providers.
- Keep the concrete `ToolProvider` construction path and existing Orchestrator.
- Prove that hidden tools remain callable only through explicitly authorized
  internal paths and that disabled tools cannot be resolved.

### Stage 2: Skill dependency and activation

- **Landed (bounded first slice):** extend Skill frontmatter with at most 16
  typed `builtin`/`mcp` dependency references, include non-empty declarations
  in the bounded metadata catalog, and expose `Discovery::activate_skill` as a
  typed metadata-only activation result.
- The activation result does not read the Skill body, start an MCP server,
  enable a provider, or grant approval; Host policy remains the authority for
  resolving dependencies.
- **Next slice:** resolve selected dependencies before a Turn and add explicit
  Skill activation to the local/App Server Turn input model, without starting
  unselected MCP servers or exposing unrelated tools.
- Preserve progressive disclosure and bounded prompt composition.

### Stage 3: Plugin provider selection

- Treat Plugin discovery as catalog metadata plus provider inputs.
- Map Plugin MCP tools to the existing MCP provider.
- Map explicitly supplied callback tools to Dynamic Tool handling.
- Reject arbitrary provider IDs from untrusted Thread data.
- Keep Plugin installation, startup, approval, and retry out of Core.

### Stage 4: Internal ThreadItem projection

- Add the smallest internal Item representation needed to project existing
  Core/Host events.
- Use `callId` as the initial tool Item id.
- Map Shell, Edit/Write, MCP, Dynamic, image, and message events.
- Emit started/completed state from one App Server event projection path.
- Do not yet replace Core Session messages or add a second persistence log.

### Stage 5: Public ThreadItem protocol

- Add v2 `Turn.items`, Item lifecycle notifications, and bounded deltas.
- Add cursor-based `thread/items/list` only when persisted projection and resume
  semantics are stable.
- Update App Server README, generated schemas, Python SDK, TypeScript output,
  Studio, TUI, and cookbook examples in the same public-protocol batch.
- Add public approval correlation tests for Item and request IDs.

### Stage 6: Artifact references

- Adapt existing ResultStore/ImageStore outputs to bounded Item references.
- Define retention, fork, resume, redaction, and size limits.
- Add Artifact read/list only if Studio/SDK has a demonstrated use case.
- Never make artifact contents automatically model-visible.

### Stage 7: Goal Runtime integration

- Goal Runtime schedules ordinary Thread Turns.
- Milestone evidence references settled Turn/Item IDs and bounded result handles.
- Verifier output remains an isolated derived artifact and cannot rewrite the
  main Thread history.
- Automatic continuation, settings notifications, and Goal notifications stay
  under the existing Goal Runtime proposal and are not duplicated here.

## Six-question admission record for every stage

Every implementation batch must answer all six questions in the PR template:

```text
1. Layer
   Tool Catalog/Host, Capabilities provider, Core protocol projection, App Server
   protocol, or client SDK/UI. Explain why the change cannot stay at the edge.

2. Duplicate responsibility
   Identify existing Skill discovery, CapabilityRegistry, ToolRouter, Orchestrator,
   Session, Event, and App Server paths. Name the canonical owner.

3. Replace vs add
   State which current provider/profile/message/result path is replaced or reused.
   A second tool loop, approval authority, Session log, or Artifact store is not
   accepted without an explicit decision.

4. Net line delta
   Report runtime and all-Rust before/after values. Default to net-zero growth or
   identify an explicit offset. Keep every batch within the change-size guidance.

5. Visible surface
   List changes to model-visible tools/prompts, Item events, persistence, approval
   correlation, public JSON-RPC, generated schemas, SDKs, and frontend behavior.

6. Boundary evidence
   Provide public App Server/local-client evidence for Skill selection, disabled
   tool behavior, approval denial, Item lifecycle, bounded results, resume/fork,
   and any timeout/cancel/steer race affected by the batch.
```

## Verification matrix

The following evidence is required before the corresponding stage is accepted:

| Area | Required evidence |
|---|---|
| Tool exposure | model request contains only selected ToolSpecs; disabled/hidden tools are not accidentally callable |
| Skill activation | selected Skill loads bounded metadata/instructions; unselected Skill does not alter prompt or tools |
| Plugin/MCP | selected provider starts only after policy; denied startup/call produces structured non-empty failure |
| Builtin approval | auto-approved and user-approved calls both emit the same observable lifecycle; denial never reaches Runtime |
| ThreadItem | same id appears in started/completed events and persisted read/list output |
| Item deltas | deltas are bounded progress and completed Item is authoritative |
| Artifact | large result is referenced, bounded, resumable, and not implicitly injected into model context |
| Thread lifecycle | resume/fork does not replay settled side effects or lose selected capability policy |
| Control race | interrupt, steer, timeout, approval, and continuation order is deterministic where applicable |
| Frontends | App Server, Python SDK, Studio, and TUI consume the same Thread/Turn/Item semantics |

## Guardrails and non-goals

The proposal does not authorize:

- arbitrary executable Plugin loading from `.agents/plugins`;
- Skills that execute tools during discovery;
- a Plugin-owned approval or sandbox implementation;
- a second `ToolRouter`, ToolOrchestrator, turn loop, or Session authority;
- unbounded Skill bodies, tool arguments, tool outputs, or artifact previews;
- a generic Artifact protocol before ResultStore/ImageStore semantics are settled;
- copying all Codex App Server methods or every ThreadItem variant;
- moving workflow orchestration back into the REPL;
- restoring the removed `workflow/plan/set` compatibility adapter;
- adding public raw system-prompt replacement;
- deleting Core tests, Actor/CAS/Session boundaries, or approval evidence to meet
  the line target.

## Risks and decisions still open

| Risk/open decision | Required resolution |
|---|---|
| Default Builtin Provider has a fixed catalog | Stage 1 must add Host-owned selection without reintroducing removed tools or duplicating constructors |
| Skill dependencies can expand capabilities unexpectedly | Resolve only against an allowlisted Thread policy and record missing/denied dependencies |
| Dynamic Tool callback may block a connection | Reuse the App Server request lifecycle and isolate blocking work before public enablement |
| Item projection can duplicate Event and Session logic | Keep one projection adapter and one durable authority; add no parallel loop |
| Artifact references can leak paths or sensitive output | Use opaque IDs, bounded previews, redaction, and explicit ownership |
| Public Item schema grows quickly | Start with the smallest real variants and use pagination/item views |
| Whole-Rust budget is nearly saturated | No implementation stage begins without a measured offset and a few-hundred-line batch |

## Acceptance criteria

This proposal is ready for implementation only when:

1. the six-question gate is accepted for Stage 1;
2. a whole-Rust offset leaves enough margin for the selected batch and follow-up
   fixes;
3. the initial Builtin/Host/MCP/Dynamic origin and exposure vocabulary is stable;
4. the Thread/Turn/ThreadItem ownership boundary does not require moving Session
   authority into Core or App Server;
5. approval correlation remains `threadId → turnId → itemId/callId → requestId`;
6. the artifact contract is bounded and sidecar-based;
7. README, CHANGELOG, Agent Notes, schemas, SDKs, and frontend consumers are
   included in the affected public-protocol batch;
8. a public App Server scenario can distinguish selected, denied, completed, and
   resumed capability behavior.

## Current decision

**Proposed: accept the direction; the six-tool exposure preparation and the
first bounded Skill dependency/activation metadata slice are implemented.**

The default Builtin catalog, its direct deletion, the first Host-owned Tool
Catalog slice, Thread-level Builtin selection, and bounded Skill dependency
metadata/activation are now landed. App Server Turn activation, Host dependency
allowlist resolution, Plugin provider selection, ThreadItem protocol work,
Artifact APIs, and Goal Runtime integration remain later stages.

## Implementation record — 2026-09-01 Skill dependency/activation slice

Six-question admission:

1. **Layer:** Capabilities Skill discovery and metadata; this is the owner of
   frontmatter parsing and bounded Skill declarations.
2. **Duplicate responsibility:** reuse `Discovery`, `CapabilityRegistry`, the
   existing Builtin catalog, and the existing MCP loader; no Router or
   Orchestrator was added.
3. **Replace vs add:** extend the existing Skill metadata record and add one
   typed activation projection; provider startup, approval, and execution are
   still owned by Host/Capabilities.
4. **Net line delta:** all Rust `28,987 → 29,178` (`+191`); the reported runtime
   (`core + protocol + host + app-server`) remains `17,343`. The batch stays
   below the few-hundred-line limit and consumes the remaining budget margin.
5. **Visible surface:** Skills with dependencies add bounded dependency metadata
   to the model-visible catalog. No App Server method, event, persistence,
   approval, or provider surface changes.
6. **Boundary evidence:** Capabilities tests prove typed activation returns the
   exact dependency declaration while MCP remains unloaded, and reject an
   unsupported dependency type. App Server Turn activation and allowlist
   resolution are intentionally not claimed by this slice.

Decision: **accept the bounded metadata slice and proceed to the next explicit
Turn/Host resolution batch only with a measured offset or further cleanup.**
