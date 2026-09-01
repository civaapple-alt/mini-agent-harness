# Changelog

All notable changes to Mini Agent Harness are documented here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Architecture proposal:** add a complete Codex-aligned design for Skill,
  Plugin, Builtin/Host/MCP/Dynamic Tool selection, `Thread` → `Turn` →
  `ThreadItem` projection, approval correlation, and sidecar Artifact references.
  The full design remains gated on release-source budget; this batch only
  implements its six-tool default exposure policy and records the admission gate.
- **Line budget:** change the enforced 30,000-line total to cover supported
  release packages only; `mini-agent-cli`, including the experimental REPL, is
  still reported separately but no longer blocks the release-source gate.
- **Builtin tool scope:** the default model-visible catalog is now limited to
  `read_file`, `write_file`, `edit_file`, `shell`, `web_fetch`, and `read_image`.
  Managed-process tools and `read_tool_result` are removed; `ResultStore` remains
  an internal bounded result sidecar.
- **Host Tool Catalog:** add the first Host-owned catalog with explicit tool
  origin, exposure, admission class, provider, and stable name. The catalog
  filters only the built-in provider; externally registered providers keep their
  own tool surface outside the Thread-level Builtin selection.
- **Thread tool selection:** extend `thread/settings/update` with optional
  `builtinTools`. Host validates the bounded six-tool subset and Core applies a
  reversible Router visibility filter; omission preserves the active Thread's
  selection, an empty list hides all Builtins, and MCP/external tools remain
  available. Durable capability selection for newly created or forked Threads
  remains a later persistence batch.
- **Skill dependency/activation:** extend bounded `SKILL.md` frontmatter with
  optional typed `builtin`/`mcp` dependency references and expose a local
  `Discovery::activate_skill` metadata result. Activation does not read Skill
  bodies, start MCP, enable providers, grant approval, or add an App Server
  method; Turn input and Host allowlist resolution remain deferred.
- **Plugin provider selection:** selecting a validated Plugin name now retains
  all of its discovered MCP provider inputs through the existing extension
  selection path. This fixes Plugin-to-server selection without starting MCP,
  granting approval, or adding a Plugin-specific execution path.
- **ThreadItem projection:** add bounded `items` to App Server `turn/event` and
  `turn/read`, derived from existing Core events/messages. The first subset is
  `UserMessage`, `AgentMessage`, `Reasoning`, generic `ToolCall`, and
  `ContextCompaction`; tool items reuse `callId`, and no second history store or
  Artifact API is introduced.
- **Goal Runtime convergence:** make the Codex-shaped `thread/goal/set|get|clear`
  contract the canonical new Goal control plane. Host Goal state now carries
  bounded objective, token budget, and timestamps with old-state defaults, and
  the App Server Runtime Actor owns a serialized `GoalRuntime` state component.
  The existing Thread/Turn loop remains the only execution loop; automatic
  continuation, Goal notifications, and retirement of manual workflow controls
  remain separately gated.
- **Planning:** merge the Goal Runtime plan into the Codex-aligned capabilities
  record as its execution appendix. The remaining automatic continuation,
  settings/Goal notifications, and retirement of manual Goal controls stay
  gated by public boundary evidence.
- **Test maintenance:** remove duplicate App Server approval-broker coverage
  already exercised by the public Shell approval scenario, and remove the
  private image-magic unit check covered by the workspace `read_image` path.
- **REPL scope:** remove the `/status`, `/info`, and `/session` management
  displays plus the duplicate parent-side capability projection. Runtime,
  capability, world, MCP, and session metadata inspection remains available
  through App Server clients; REPL session persistence/resume entry points,
  streaming, approval, `/steer`, and turn execution remain unchanged.
- **Breaking App Server API:** replace `workflow/plan/set` with the typed
  `thread/settings/update` `collaborationMode` setting. Plan Mode now updates
  the bounded Host prompt and approval lock through the Runtime Actor, and
  persisted Plan Mode is restored when a runtime is rebound. No compatibility
  wrapper for the removed method is provided.

### Added

- Externalized stable Core, Capabilities, and App Server built-in prompt bodies
  into crate-owned Markdown templates embedded at compile time. Prompt bytes,
  bounded composition, and the existing allowlisted profile selection remain
  unchanged; arbitrary public system-prompt replacement was intentionally not
  added.
- App Server approval routing now emits a server-owned `approval/resolved`
  notification after `approval/respond`, so clients can observe the complete
  `requested → resolved → tool result` lifecycle. Shell approval requests now
  preserve `requestId`, `turnId`, and `callId` across that lifecycle. This is
  additive to the protocol and does not change approval policy or Session commit
  semantics.
- Added the first `ToolRouter → ToolExecutionDelegate → Host ToolOrchestrator`
  migration seam. Core resolves a tool and delegates execution lifecycle ownership
  to Host; legacy tool execution and approval behavior remain compatible while
  typed admission is prepared for a later bounded migration.
- Added typed `ToolAdmission` for the built-in Shell tool. Host now approves the
  bounded command before invoking Shell's post-admission execution hook; other
  tools remain on the explicit legacy path until their own migration evidence is
  accepted. Added a public App Server JSON-RPC scenario covering Shell approval
  request/respond/resolved and the resulting `turn/event` lifecycle.
- Migrated `edit_file` and `write_file` to typed admission with approval before
  file mutation; their direct legacy `execute` entry points remain approval-safe
  for compatibility.
- Migrated MCP tool calls and outside-workspace `read_image` to typed admission.
  MCP server startup approval remains a separate Host assembly gate, while
  workspace/session image reads and other read-only tools retain their legacy path.
- Split the tool contract into `ToolHandler` (schema, argument parsing, and
  admission) and `ToolRuntime` (legacy and post-admission side effects). `Tool`
  remains their composition boundary, so Router resolution and Core history
  handling stay unchanged.

### Removed

- Removed the managed-process Builtin tools and the model-visible
  `read_tool_result` tool to keep the Builtin catalog limited to six tools. The
  App Server `capabilityManifest.rulePolicy.processExecution` field is also
  removed because no supported Builtin capability consumes it.

### Audited

- Audited the Core/Host `ToolRouter → ToolOrchestrator` approval path. The first
  execution-delegate seam now exists, but there is still no full central approval
  orchestrator: Core dispatches, built-in Capabilities describe typed admission,
  Host owns the migration-time approval decision, and App Server owns approval
  transport plus settled-turn persistence. Approval-gated model tool calls are now
  migrated in bounded slices; read-only tools and MCP server startup remain explicit
  legacy/assembly boundaries.

### Changed

- Isolated the App Server worker on a dedicated Tokio runtime thread so the
  synchronous approval callback cannot block the public connection transport.
  The callback and single-Thread worker semantics remain explicit; this does not
  claim a fully asynchronous Tool API.
- Narrowed the interactive REPL to a core-capability reference client. It keeps
  turns, streaming events, approval, `/steer`, startup-selected manual/auto
  execution, and session persistence/resume entry points, while Plan/Goal
  workflow orchestration and session metadata inspection are consumed through
  the App Server by Studio/SDK clients instead of duplicated REPL management
  commands.
- Removed duplicate World/MCP/extension management presentation from the REPL.
  Host still injects the bounded world context and loads configured capabilities;
  Studio/SDK clients use the App Server `world/*` and `mcp/*` methods for
  inspection and retry.
- Removed the interactive `/session` metadata display. The worker still uses
  the existing session authority for resume detection and persistence;
  Studio/TUI owns user-facing session identity and storage inspection.
- Reduced the REPL implementation by 31.6% from its 649-line baseline. Removed
  `/help`, `/queue`, `/new`, and runtime `/auto` mode switching; manual versus
  automatic execution is selected at startup (`mini-agent` or `mini-agent auto`),
  while `/steer`, approval, streaming, turn execution, and session resume remain.

## [0.6.0] - 2026-09-01

### Added

- Added a repository pull request template that requires every change to
  declare its layer, duplicate-path analysis, replacement strategy, line
  budget, visible-surface impact, and boundary-test evidence.
- Added a pull-request admission CI check that requires the six-question
  template, six non-empty answers, replaced placeholders, and six checked
  confirmations; answer quality remains a reviewer responsibility.
- Audited HTTP 429 handling and retained the bounded fail-fast provider policy:
  one response becomes one `OpenAiError::Api` without implicit retry. Retry count,
  backoff, jitter, `Retry-After`, and cancellation semantics remain a separate
  deferred policy decision.
- Audited CLI automatic Trace export and kept implicit Session-file export and the
  retired external `--trace` option out of scope. The bounded explicit artifact
  contract is now implemented through `ask --trace-jsonl PATH`.
- An earlier Docker sandbox audit ran before the Linux daemon was reachable; it
  recorded the existing smoke test as clear-error/preflight evidence only, not
  evidence of container isolation or portability.
- Audited model/provider comparison: the built-in registry currently exposes one
  OpenAI-compatible provider, and the Host model factory is only a composition
  seam. No cross-provider quality claim or paid-provider CI gate was added.
- Tightened the PR admission checker to count only the six confirmation boxes
  under `### 准入确认`, so unrelated checklist items cannot satisfy the gate.
- Tightened the same checker to match each stable confirmation label exactly once,
  so an unrelated checked item inside that section cannot mask an unchecked
  admission confirmation.
- Documented the evidence-triggered next-iteration order: bounded CLI Trace
  contract, CLI MCP-timeout seam, compaction evidence, Docker isolation, and
  provider/retry policy, with missing evidence treated as a defer condition.
- Recorded the CLI Trace contract audit: reuse the bounded caller-owned
  `JsonlTrace`, require an explicit artifact lifecycle and total-size limit, and
  defer automatic CLI export until a public CLI scenario proves the boundary.
- Added measurable Core compaction evidence without changing runtime behavior:
  the existing scenario now asserts a trigger at least 70% of its bounded fixture
  budget, while the production trigger remains 50% and recent-tail retention stays
  protected.
- Added a real Docker runtime probe after the daemon became reachable: Docker
  29.6.1 with `alpine` verifies the `/workspace` mount and container-only temporary
  files through the Capabilities path. Network, capability, and resource isolation
  remain unclaimed pending an explicit policy and cross-platform evidence.
- Corrected Docker preflight to query the daemon with `docker info` instead of only
  checking the CLI version, so an unavailable daemon fails closed before `docker run`.
- Recorded the Docker security-policy gate: stronger network, capability, privilege,
  read-only, or resource restrictions require an explicit threat model, supported-platform
  policy, compatibility/failure contract, and boundary evidence before implementation.
- Added a proposed Docker isolation policy note with candidate strict defaults,
  threat-model choices, cross-platform evidence requirements, and fail-closed behavior;
  no Docker runtime defaults changed.
- Recorded a current-host Docker strict-policy feasibility probe: the candidate flags
  were accepted and produced read-only root, writable `/tmp`, zero effective capabilities,
  no routes, and bounded cgroup values; this remains non-cross-platform evidence only.
- Extended the current-host Docker strict-policy probe to the existing `/workspace`
  bind mount: a workspace file reached the host and a container-only `/tmp` file did
  not reach the workspace; this remains non-cross-platform evidence only.
- Refreshed the unified CLI-through-App-Server proposal with the current budget
  snapshot (`16,243/20,000` runtime and `29,815/30,000` all Rust); its historical
  over-budget snapshot remains labeled as such, while cross-platform and
  authorized real-provider evidence remain open.
- Consolidated the framework maturity review and VS Code harness next-iteration
  note into one canonical architecture note, preserving the mini/Codex layer,
  Turn/Step, steering, hard-limit, scenario, and six-question evidence records.
- Added an AGENTS rule to keep future framework and Harness evidence updates in
  the canonical merged note and synchronize its documentation indexes.
- Repaired documentation links after note consolidation, including the PR
  template and moved Core/Host/Capabilities/App Server source paths; historical
  deleted implementation paths are no longer presented as live links.
- Marked the retired Real LLM scenario runner as historical and corrected the
  remaining event-loop note to the current session-storage path; no paid-provider
  behavior was added.
- Marked retired subagent/persona notes as historical and updated Goal/Plan
  documentation to the current Host/App Server ownership; no subagent or
  protocol behavior was reintroduced.
- Added opt-in CLI Trace export through `ask --trace-jsonl PATH`: it creates a new
  file, emits only bounded redacted event metadata, caps each record at 8 KiB and
  the artifact at 256 KiB, and fails on overwrite or finalization errors. CLI public
  scenarios cover success/redaction and refusal to overwrite an existing artifact.

### Changed

- Activated Stage 3 normal budget admission. The runtime `20,000`-line and
  release-source `30,000`-line ceilings remain hard gates; the experimental
  CLI/REPL remains separately reported and excluded from the release-source
  gate. New changes default to net-zero growth or must identify an explicit
  offset.
- Aligned `AGENTS.md` with the Stage 3 workflow: affected-package validation is
  the default, while local full-workspace tests require explicit approval;
  CI remains responsible for the full matrix.
- Implemented next-iteration harness notes based on the VS Code coding harness
  article, including turn/round evidence, bounded product scenarios, and
  mandatory six-question validation records for future practice updates.
- Pull request admission now requires Harness Scenario/Eval evidence when a
  change affects prompt, tool schema, loop-control, context, events, or
  persistence; public unit tests alone are not sufficient for those changes.
- Added the first bounded harness scenario baseline: 8 representative CLI
  scenarios pass, backed by App Server 28/28 and CLI interactive 13/13
  regression evidence. Broader tool-failure/retry coverage and provider
  comparison remain tracked as follow-up gaps.
- Added CLI public-path evidence for unknown-tool recovery: the existing
  bounded tool failure is projected into the next provider request and the
  settled answer completes through `mini-agent ask --json`.
- Added App Server public-boundary evidence for approval denial: existing
  `NeedsApproval` status and non-empty reason are preserved in `ToolFinished`,
  the settled checkpoint, and the next model round without a new protocol type.
- Added a bounded CLI public-path cross-file refactor scenario that reads and
  edits two workspace files through the canonical tool path and verifies both
  settled file results without adding a production refactorer.
- Added the local App Server `JsonlTrace` sink and redacted JSONL records with
  bounded model-input, tool-manifest, event-payload, and output-size metadata;
  the JSON-RPC wire shape and retired external `--trace` path remain unchanged.
- Added Stage 2 test-only fault evidence: a Core `FaultInjectionModel` and
  Responses parser cases cover missing or malformed tool arguments, partial
  model streams, and retryable tool results without adding a production
  provider or execution path. Responses now also has boundary evidence that
  HTTP 429 maps to a bounded API error without implicit retry; provider-level
  retry/backoff policy remains a follow-up.
- Added explicit MCP/sandbox refusal evidence: denied MCP connections expose no
  server, tool, or plugin data and retain one bounded diagnostic; an already
  connected tool returns a structured non-empty `Failed` reason before sending
  its call; denied shell commands retain a structured `Failed` reason before
  sandbox execution, with no marker side effect. At that point Docker
  availability and isolation remained open.
- The earlier Docker smoke-test audit was non-authoritative because a present
  Docker CLI did not prove daemon availability or container isolation; the later
  runtime probe is recorded above as the current partial evidence.
- Audited the failure/timeout/retry evidence matrix across Core, Capabilities,
  App Server, and CLI, explicitly separating covered public paths from unit-only
  evidence and deferred CLI MCP timeout projection, provider backoff, and Docker
  isolation work.
- Added a controlled MCP call-timeout check through the real stdio/RMCP capability
  path; production keeps the 118-second bound, while tests use a test-only 50ms
  seam and assert a bounded structured `Failed` reason. App Server projection is
  covered separately; CLI transport projection remains a deferred follow-up.
- Added an App Server public-boundary scenario proving that the structured MCP
  timeout result remains visible in `ToolFinished`, the durable checkpoint, and
  the next model round; the CLI public MCP transport projection remains open.
- Audited the CLI MCP timeout evidence gap: the fast timeout seam is scoped to
  Capabilities unit tests, while CLI integration compiles the dependency with its
  118-second production bound. Deferred adding a production-only test option or
  configuration field until a user-facing timeout policy justifies that surface.
- Reconfirmed the CLI MCP timeout gap after the Trace batch: no bounded cross-layer
  seam exists, so no production timeout option, test-only runtime flag, or alternate
  transport was added; the CLI projection remains explicitly deferred.
- Added a local baseline rerun/report recipe and recorded the next-iteration
  evidence decisions for bounded JSONL Trace export, test-only fault injection,
  slow-scenario scheduling, Compaction measurement, structured permission
  rejection, and timeout/steer race ordering. The retired external `--trace`
  path and current 50% Compaction trigger remain unchanged.
- Documented the current budget snapshot (`16,243` runtime lines and `29,815`
  total Rust lines) in the README and Agent Notes.

### Fixed

- Goal timeout now requests `turn/interrupt`, waits for the App Server turn to
  settle and persist its checkpoint, and only then records the workflow goal as
  failed. This removes the race where `goal/fail` was rejected while the turn
  was still running.

## [0.5.0] - 2026-08-30

### Changed

- Removed the duplicate public `RuntimeBuilder` API; `HostRuntimeFactory` is
  now the single named Host runtime construction entry point.
- Tightened workflow ownership: App Server workflow commands are the frontend
  boundary, while Host exposes only the wrapped `HostWorkflowStore` persistence
  seam and no longer publishes the workflow implementation module.
- Kept `mini-agent-capabilities` as one crate while making implementation
  modules private behind a curated root facade.
- Renamed the current Goal verification implementation from the legacy Mentor
  naming to `verifier`; new configuration uses `VERIFIER_OPENAI_*`.
- Removed the standalone paid-provider experiment crate and exploratory test
  targets from the mainline workspace; provider evaluation is now external.
- Reduced extension discovery to workspace-local skills, installed plugins, and
  MCP configuration; marketplace and skillset clone traversal is no longer in
  the mainline path.
- Removed the remaining standalone `demo`, `doctor`, `status`, `sessions`, and
  legacy `mentor` CLI command paths; runtime status is available through the
  interactive `/status` command and Goal verification remains an internal
  tool-free workflow gate.
- Removed stale session replay/derived-state adapters, unused persona and
  profile variants, duplicate capability/discovery/security accessors, and
  redundant App Server runtime proxies. The 0.5.0 baseline is 28,932 Rust
  source lines, including tests, with 15,745 lines in the runtime layers.
- Unified Runtime Actor ownership for Thread lifecycle, World, Workflow, MCP,
  Session persistence, and RuntimeRevision state changes behind one ordered
  App Server worker queue.
- Made `RuntimeRevision` participate in mutation CAS checks. Stale runtime
  actions now return structured revision conflicts instead of silently applying
  last-write-wins updates.
- Moved settled-turn Session persistence into the Runtime Actor boundary. The
  worker persists the checkpoint before releasing `TurnFinished`, and reports
  persistence failures through the settled turn result.
- Added App Server action result metadata: `actionId`, `actionSequence`, and
  `stateRevision`. Admitted-action errors expose the same metadata in
  JSON-RPC `error.data`; Core `eventSequence` remains independent.
- Removed redundant Runtime management wrappers and the unused `UpdateWorld`
  command while retaining typed local checkpoint/context operations needed by
  the CLI runtime.

## [0.4.0] - 2026-08-29

### Added

- Runtime capability profiles for interactive, ask, auto, and ACP frontends,
  with bounded prompt/rule source manifests and explicit `--no-tools` scope.
- Optional bounded `.agents/profile.json` overrides for local CLI and App
  Server profile selection, including allowlisted extension names.
- Host-embedded external tool-provider registration through
  `CapabilityRegistry::with_tool_provider`, with a runnable echo-provider
  example.
- Generic external model-provider registration through
  `CapabilityRegistry::with_model_provider` and
  `AppServerRuntime::<M>::start_with_model_factory`, with a compile-checked
  example in `mini-agent-app-server/examples/external_model_provider.rs`.

### Changed

- Host runtime construction now derives policy and sandbox settings from the
  resolved profile. Frontend approval callbacks remain transport adapters, and
  explicit CLI flags are tracked separately so workspace profile values are
  preserved when no override was requested.
- CLI README and subcommand help now document session, run, safety, sandbox,
  web-search, max-step, and JSON options consistently. Interactive
  `--session-id` resumes the requested durable session.
- CLI `ask`, one-shot `auto`, interactive REPL, `demo`, `mentor`, and Goal
  verifier turns now use the local App Server boundary and its ordered event
  stream. The App Server also exposes the same runtime through typed local and
  JSON-RPC clients, with ACP mapping on top.
- Core execution contracts are separated from the protocol crate. Session,
  context, turn lifecycle, cancellation, and queued input are owned by the
  execution core, while wire payloads remain in protocol.
- Core no longer re-exports Protocol types from its root API. Harness
  run-control, context compaction, tool-batch execution, and model-event
  forwarding now live in focused private modules while the public turn API is
  unchanged.
- The full workspace Rust line total, including tests, is enforced at 30,000
  lines for the 0.4.0 release; the runtime layers retain their 20,000-line
  ceiling.
- Host responsibilities are separated from App Server transport and worker
  orchestration. Runtime configuration, provider setup, tools, workspace
  policy, persistence, Goal state, and Plan state remain in the Host layer.
- Provider execution now uses one Responses protocol adapter. The former GLM
  Chat Completions adapter, model-specific routing, and `OPENAI_CHAT_BASE_URL`
  setting were removed.
- Source line budgets now report core, protocol, host, app-server, ACP, and CLI
  separately, including production, unit-test, and integration-test lines.
- Session `session.jsonl` is now the single durable runtime record: result
  handles append to it and reload on resume, while the external trace file,
  `--trace`, and trace replay/summary commands are retired. Interactive, ask,
  and auto sessions are always persistent.
- RuntimeBuilder and AppServerRuntime now accept a profile seam; ACP initialize
  reports its default profile and capability manifest while compatibility
  startup paths retain their existing defaults.
- HostRuntimeFactory and RuntimeBuilder accept an optional capability registry;
  the default registry and all existing CLI/App Server paths remain built-in.
- Local App Server bootstrap now resolves runtime configuration, workspace
  profiles, overrides, and Harness limits for both `ask` and the REPL worker;
  CLI code remains focused on input, approval, and rendering.
- App Server workflow management now binds Goal/Plan persistence, verifier
  state, milestone transitions, and prompt construction to the runtime session;
  the REPL no longer calls the Host Goal module directly.
- Workflow management is now available through typed App Server JSON-RPC
  methods and ACP `session/workflow/*` mappings without exposing Host paths.
- Session metadata, workspace state, execution policy, and MCP status/retry are
  now App Server protocol methods shared by JSON-RPC and `LocalAppServerClient`.
  The CLI REPL uses that management boundary instead of direct Host world state
  or capability session access.
- CLI no longer directly compiles against Host or Capabilities. Launch and
  observation contracts come from the App Server frontend facade; provider
  evaluation remains outside the mainline workspace.
- Goal/Plan lifecycle, verifier evidence, and restart pause operations now use
  the same `LocalAppServerClient` workflow control plane as JSON-RPC.
- App Server frontend profile, configuration, approval, workflow, and output
  observer contracts no longer expose Host implementation types. The REPL
  presentation loop and App Server worker are maintained in separate modules.

### Fixed

- Interactive run control now distinguishes immediate `/steer` input from
  queued follow-up input and preserves the shared worker checkpoint boundary.
- REPL managed processes, restart/resume flows, and settled App Server results
  now use the same session and event lifecycle as headless turns.
- Goal verifier failures, retries, rejection, exhaustion, timeout, and
  checkpoint restart paths now have deterministic integration coverage.
- Web fetch results can be continued in bounded pages instead of exceeding the
  tool response limit.

## [0.3.0] - 2026-08-27

### Added

- Session `attachments/` for `read_image` reload on resume and copy on fork, so GLM inline vision survives process restart. Compaction auxiliary requests send an empty tool catalog (no image projection, no `web_fetch` during summarize).
- Split the host provider adapter into Responses (`openai/responses.rs`) and Chat Completions (`openai/chat.rs`). GLM image turns use Chat Completions only when `OPENAI_CHAT_BASE_URL` is set; the Responses root is not rewritten to `…/api/coding/paas/v4`.
- Windows `open_file` for images writes a Mark-of-the-Web-free temp copy before `start`, so Photos does not prompt that the file came from an untrusted location. HTML still opens in place.
- GLM Coding Plan over the existing Responses adapter: `OPENAI_BASE_URL=https://open.bigmodel.cn/api/v1`, `OPENAI_MODEL=glm-5.3` (image turns use `glm-5.3-flash`). Files API upload is DeepSeek-only; GLM `read_image` is inline data URL. Built-in Responses `web_search` stays off for BigModel.
- Host `read_image` reads a workspace PNG/JPEG/GIF/WebP or, with approval, an absolute path outside the workspace (for example Pictures). It uploads once with DeepSeek Files API (`purpose=user_data`, 7-day expiry); later turns send `input_image.file_id`. DeepSeek `deepseek-v4-flash` / `deepseek-v4-pro` requests that include images are sent as `deepseek-v4-flash-vision-exp` for that request only. Do not copy outside images into the project.
- Host `web_fetch` GETs a known public HTTP(S) URL or a loopback dev server (`localhost`, `127.0.0.1`) and returns bounded markdown via `htmd` (JavaScript not executed; LAN/cloud-metadata and credentialed URLs rejected; public→loopback redirects refused). Host `open_file` opens a workspace file or, with approval, an absolute local path (for example Pictures) in the OS default app.
- First-class Plan Mode (`/plan`, `/plan <prompt>`, `/plan off`) with workspace modification locking and a session-directory living plan (`plan.md`).
- Autonomous Goal Mode (`/goal <objective>`) state machine (`goal/state.json`, `goal/plan.md`) with milestone tracking and independent verifier gate support.
- Builtin foundational agent prompts (`explore`, `plan`, `general`) and 7 specialized personas (`reviewer`, `implementer`, `security-auditor`, `test-writer`, `researcher`, `design-doc-writer`, `design-doc-reviewer`) in `persona.rs`.
- Dual-mode file collaboration contracts (`review_file`, `summary_file`) with automated prompt rendering, issue lifecycle state tracking (`open`, `fixed`, `wontfix`, `addressed`), and live review statistics in `SpawnAgent`.
- Modular session directory layout generating atomic `summary.json`, `signals.json`, and `prompt_context.json` snapshots alongside durable `session.jsonl` records.
- Fast $O(1)$ session discovery in `mini-agent sessions` reading lightweight `summary.json` metadata indexes without parsing full conversation streams.
- Subagent tree execution tracking recording lifecycle metrics (`started_at_ms`, `completed_at_ms`, `duration_ms`, `steps`, `exit_code`, `status`) in `meta.json` and structured deliverables in `output.json` under the durable parent session's `subagents/<id>/` directory.
- Subagent preset role configurations (`explore`, `plan`, and `general`) and `fork_context` prompt controls in `SpawnAgent`.
- Subprocess CLI-driven subagent execution tool (`spawn_agent`), allowing parent agents to delegate bounded tasks to isolated child `mini-agent ask "<prompt>" --json` processes with zero prompt pollution, OS-level crash/memory isolation, and structured result aggregation.
- Multi-turn interactive subagent session tool suite (`send_subagent_message`, `list_subagents`) enabling stateful conversational follow-ups and refinement with child agents via durable session resumption (`--session-id`).
- Implicit persistence encapsulation for subagents: `spawn_agent` automatically provisions unique session identifiers (`sub-<time>-<task>`) and commits child checkpoints for seamless multi-turn resumption while standard CLI invocations remain clean and ephemeral.
- Offline trace summary and replay for the selected JSONL trace or session file.
- Multi-item validation on durable session restore (`restore_history`) validating `Context`, `User`, `Assistant`, and `Tool` message bounds individually.
- Turn-atomic trimming (`remove_first_message_group`) ensuring function calls and settlement outputs are dropped as cohesive groups during compaction.
- Settlement-aware loop detection tracking `(name, arguments, content)` to avoid false positive stall warnings during polling.
- Session resume and fork invalidation notice informing the model that prior process IDs and result preview handles have expired.
- Backward-compatible session lookup for legacy `.agents/sessions/<id>.jsonl` and `.agents/sessions/<id>/session.jsonl` across `resume`, `fork`, and `sessions` listing.
- Canonical security action normalization and glob wildcard matching (`**/.env*`, `rm -rf /*`, `gh auth *`) in `SecurityPolicy`.
- Docker sandbox execution validation ensuring container isolation or failing closed with clear diagnostics.
- Backward-compatible `--auto` alias for `--auto-approve` in `mini-agent ask`.
- Decoupled CLI architecture with dedicated `args.rs` and `harness_builder.rs` modules, reducing `main.rs` to a lightweight dispatcher.

### Fixed

- `/plan <prompt>` now enters Plan Mode and starts drafting with that prompt instead of being rejected as an unknown command.
- Plan Mode writes go to the session living plan (`plan.md` under the session directory); relative `plan.md` is aliased there and other workspace mutations stay locked.
- REPL Plan Mode overlays the builtin `plan` Software Architect foundation plus a living-plan rider so the model plans into `plan.md` instead of emitting the deliverable.
- Plan Mode `write_file plan.md` replaces the initialized session living plan instead of failing with "file already exists".
- `/goal <objective>` now starts executing the first milestone immediately instead of only writing `goal/state.json` and returning to the prompt.
- Goal Mode can read and update session `goal/plan.md` (relative `goal/plan.md` maps there) instead of failing with "path escapes the workspace".
- Windows shell capture forces UTF-8 (`PYTHONUTF8`, PowerShell `$OutputEncoding`, and `Get-Content`/`Set-Content` default Encoding) so UTF-8 HTML previews are not shown as mojibake.
- Subagent tree records (`meta.json`, `output.json`, `parent_session_id`) live under the parent session at `~/.mini-agent/sessions/<workspace>/<parent-id>/subagents/<child-id>/`, not in the project `.agents/sessions/` directory.
- `spawn_agent` and `send_subagent_message` run child `ask` with `--max-steps 50` (and a 300s default timeout) so reviews are less likely to stop after 8 steps.
- REPL `tool>` lines for `spawn_agent` and `send_subagent_message` show task/persona/session_id and a bounded message preview.
- Security deny rules now properly match human-formatted tool action strings, preventing destructive commands from bypassing deny filters.
- Windows background managed process trees are guaranteed to terminate via `taskkill /PID <pid> /T /F` and sandbox attachment.
- CLI argument errors and root help on usage failure now print to `stderr`, keeping `stdout` pure for machine-readable JSON consumers.
- Prompted `mini-agent auto` now correctly applies `--security-preset` and `--sandbox` configurations.

### Changed

- Interactive sessions are in-memory by default; `--persist` saves settled checkpoints under `~/.mini-agent/sessions/`, while `--ephemeral` (`--no-persist`) makes the in-memory choice explicit. `auto` sessions persist by default.
- Strict `max_steps = 0` step limit evaluation halts immediately; unconstrained runs pass `usize::MAX`.
- Adaptive `web_search` default enables web search for official OpenAI/DeepSeek endpoints while disabling it for custom local LLM endpoints unless explicitly configured.
- Lowered default `max_model_response_bytes` to 64 KiB (~16K tokens) and bounded compaction summaries to 32 KiB (`max_user_input_bytes`).
- Reduced `MAX_PROJECT_INSTRUCTIONS_BYTES` to 16 KiB (~4K tokens) with head/tail truncation.
- Raised durable session `MAX_RECORD_BYTES` to 2 MiB to reliably serialize full 1 MiB checkpoints.
- Compaction prioritizes authoritative `WorldState` context over transient loop advisories.
- OpenAI request serialization suppresses `web_search` and function schemas when `request.tools` is empty (e.g. during auxiliary compaction and mentor requests).

## [0.2.0] - 2026-08-26

### Added

- `mini-agent --version` and the REPL welcome include the git revision, for
  example `mini-agent 0.2.0 (c0ffee12abcd)`.
- User-level provider settings at `~/.mini-agent/.env`, below process
  environment and workspace `.env`, so a PATH-installed binary can keep
  credentials out of project directories.
- Minimal project-scoped Agent Skills and Agent Plugins 1.0.0 discovery with
  progressive skill disclosure and approval-gated stdio MCP tools via RMCP.
- Cloned skill collections, Claude/Grok plugins and explicitly selected local
  plugin marketplaces, including compatible plugin-agent instructions.
- Standalone project MCP configuration and approval-gated streamable HTTP MCP
  with bounded headers, environment placeholders, and connection timeouts.
- Individual immediate skills can be selected from cloned marketplaces without
  enabling their containing plugin bundle.
- `.agents/marketplaces.json` can name `skills` and `plugins` separately.
  Explicit `skills` selectors walk a cloned collection for a nested `SKILL.md`
  directory and do not require a Claude or Grok marketplace manifest.
- Marketplace and skillset objects accept a local `path` so an existing clone
  does not need to live under `.agents/`. `.agents/skillsets.json` enables
  only the listed skills; without that file, `.agents/skillsets/` still loads
  each collection in full. `read_file` can open those configured roots.
- Interactive `/mcp` retries configured servers that were denied or unavailable
  during startup without clearing conversation history.
- The REPL welcome block lists bounded skill, plugin, and MCP summaries and
  reports MCP transitions from inactive to enabled.
- Bounded world-state detection for host, shell, project markers, execution
  mode, approval policy, and a fixed local command catalog, exposed through
  `status`, `/world`, and `/world refresh`.
- Typed context messages mapped to Responses API developer items and retained
  across context compaction.
- Opt-in project-local durable sessions with append-only session, thread, turn,
  and item records, settled checkpoints, bounded listing, torn-tail recovery,
  `/session`, `--persist`, `sessions`, and `resume SESSION_ID`.
- Independent `mentor insight` and `mentor verify` commands with a dedicated
  model profile, zero tools, bounded criteria, source-linked derived items, and
  strict isolation from primary conversation replay.
- Tool execution orchestrator with security presets (`--security-preset default|full-machine|turbomode|custom`), priority rule evaluation (`deny > ask > allow`), and session-level approval decision caching (`ApprovalStore`).
- Windows-first native Win32 `JobObjectGuard` process containment (`--sandbox native`) with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` guaranteeing atomic child process tree destruction and zero orphaned zombie processes.
- Multi-branch durable session forking (`mini-agent fork <SESSION_ID>`) to branch settled checkpoints into independent exploration lanes.
- Offline deterministic trace replay (`mini-agent trace replay <PATH> [--json]`) and metrics computation (`mini-agent trace summary <PATH> [--json]`) over JSONL observation event logs and durable `session.jsonl` files.
- Interactive `/status` slash command in REPL displaying current workspace, active security preset, sandbox containment, approval mode, copilot loop status, and durable session metadata.
- Built-in Responses API web search integration enabled by default (passed as `{"type": "web_search"}` tool to DeepSeek/OpenAI Responses API), with `--web-search`, `--no-web-search`, and `MINI_AGENT_WEB_SEARCH=true|false` configuration options.
- Non-interactive script approval flags `--auto-approve`, `-y`, and `--yes` on `mini-agent ask`.
- Ephemeral session opt-out flags `--ephemeral` / `--no-persist` for memory-only interactive REPL and auto sessions.
- Repetitive tool-call loop detection warning in the harness run loop to prevent unattended stalls in autonomous copilot runs.
- Remote HTTP MCP server circuit breaker with fail-fast open circuit and half-open probe recovery during backend outages.

### Fixed

- `read_file`, `edit_file`, and `write_file` accept in-workspace absolute paths
  (e.g. `D:\path\to\workspace\file.txt`) without error, automatically resolving
  and validating them within the workspace security boundary.
- Interactive `read_file` (and other non-shell) `tool[ok]>` lines stay on one
  bounded preview. `shell`, `process_read`, and `read_tool_result` still print
  their full stdout/stderr in the terminal.
- Oversized root `AGENTS.md` no longer aborts startup; the host keeps a bounded
  head and tail with an explicit truncation warning.
- `ask` no longer echoes streamed assistant text before printing its final
  result; terminals retain one `assistant>` tag while redirected stdout stays
  raw.
- Interactive startup no longer blocks before displaying the REPL when an MCP
  server requires connection approval.
- Tool start events now show bounded shell and managed-process commands; file
  tools show only their path and arbitrary MCP arguments remain hidden.
- `/auto` mode changes no longer replace augmented project and extension
  instructions; they append a world-state item while keeping the stable prompt.

### Changed

- Copilot/auto no longer stops at 128 model steps. `max_steps = 0` means no
  step cap; set `MINI_AGENT_MAX_STEPS` to impose one. Context compaction still
  keeps long runs inside the 1 MiB request ceiling.
- Auto context compaction keeps the latest world-state item and the last two
  model-step groups (capped at 128 KiB) verbatim. Only the older prefix is
  summarized. If the compaction request would exceed the 1 MiB ceiling, oldest
  prefix messages are dropped until it fits. Empty or unhelpful summaries fall
  back to mechanical prefix trim instead of aborting the run.
- Durable sessions live under `~/.mini-agent/sessions/<workspace>/<session-id>/`
  instead of `.agents/sessions/` in the project tree.
- Skill discovery reads only a bounded YAML frontmatter prefix, so an oversized
  `SKILL.md` can still be selected. `read_file` accepts up to 128 KiB so those
  instruction files can be opened.

- Interactive REPL and Auto sessions are durable by default under `~/.mini-agent/sessions/`; `--ephemeral` provides memory-only scratch sessions.
- Disambiguated `auto` CLI semantics: `mini-agent auto` exclusively represents the autonomous copilot loop, while `mini-agent ask` uses `--auto-approve` / `-y` for non-interactive approval bypass.
- Default interactive sessions and TTY `ask` run tools without per-step
  approval. `/auto` is the copilot loop; `/auto off` restores prompts.
  `run` is an alias of `ask`.
- Raised the default model request context from 256 KiB to 1 MiB and the model
  response ceiling from 64 KiB to 384 KiB. These remain provider-neutral byte
  bounds under DeepSeek V4's 1M-token context and 384K-token output.
- Raised the root `AGENTS.md` bound from 16 KiB to 64 KiB.
- Changed the project license from Apache-2.0 to MIT.
- Renamed the project from mini-codex to Mini Agent Harness, with the
  `mini-agent` executable and `mini-agent-core` / `mini-agent-cli` crates.
- Interactive reasoning, assistant, tool, and context tags use distinct colors
  on terminals while redirected output remains plain text.
- Plain terminal `ask` output streams assistant deltas without repeating the
  completed answer; redirected and JSON output remain completion-buffered.

## [0.1.0] - 2026-08-24

### Added

- Release automation and cross-platform verification.
- Post-build verification of every release archive checksum.
- `ask`, `status`, `doctor`, and `--version` CLI contracts.
- Per-command help and `--` option delimiters for prompts beginning with `-`.
- Machine-readable output for `ask`, `status`, and `doctor`.
- Bounded portable model/tool harness with explicit event traces.
- Streaming OpenAI Responses adapter, including DeepSeek reasoning events.
- Interactive, one-shot, automatic, and deterministic demo modes.
- Workspace-confined file tools and approval-gated shell execution.
- Managed background processes and bounded large-result handles.
- Cache-friendly context compaction for automatic mode.
