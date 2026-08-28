# Host Capability Profiles and Runtime Seams

Status: Stage 3 frontend routing and provider selection implemented; Stage 4 line-budget reduction pending (Windows scope)
Date: 2026-08-28

## Implementation update (2026-08-28)

Stage 1 is now wired through the current runtime without changing the core
execution loop. `mini-agent-host::RuntimeProfile` and
`CapabilityManifest` are implemented in
`crates/mini-agent-host/src/profile.rs`, with manifest/policy and workspace
profile-file loading split into focused private modules; `RuntimeBuilder` and
`AppServerRuntime` accept profiles while retaining default compatibility
constructors. The CLI selects `interactive`, `ask`, and `auto` profiles and
supports `--no-tools`; the effective manifest is printed at REPL startup,
shown by `/status`, included in one-shot JSON, and included by `status --json`.
An optional bounded `.agents/profile.json` is applied by the local CLI and
standalone App Server before explicit CLI deny overrides.
The ACP bridge defaults to `acp` and returns its profile and manifest during
`initialize`. `HostRuntimeFactory` now owns concrete Host construction, and
ACP can adapt a factory-built runtime without creating a second Thread loop.
Regular-agent prompt/rule selection is represented by
`PromptSources` and `RuleSources` under `PromptRulePolicy`; the two source
sets are independently selectable and report bounded source names (`builtin`,
`project`, `extensions`, `workflows`) without exposing prompt text. Explicit non-default
foundational agent/persona selections now render one bounded stable prompt
overlay before project and extension instructions are merged.
Sandbox and security selections are carried in the profile and reflected in
the manifest; REPL Plan/Goal commands now fail with a scope diagnostic when a
profile disables the corresponding workflow.
Extension profiles can now use `with_selected_extensions` for metadata-only
selection or `with_enabled_extensions` for an allowlisted enabled set;
discovery filters skills, plugins, and MCP server labels before prompt/tool
assembly and reports missing names as bounded diagnostics.
The App Server protocol initialize contract now carries an optional profile
request and a structured `capabilityManifest`; mismatched profile requests are
rejected before service initialization. Local clients, stdio JSON-RPC, and ACP
therefore share the same startup capability evidence.
The standalone App Server freezes the resolved workspace profile before
starting its thread factory, so later thread creation cannot silently reload a
different profile while continuing to advertise the original manifest.
The standalone binary now accepts the bounded `MINI_AGENT_PROFILE` startup
selector for the six builtin profiles and fails closed on unknown names before
provider or thread construction.
The manifest also includes the fixed prompt/rule precedence, explicit
`typed-agent-scope` rule resolution state, typed effective policy for workspace,
shell, process, and workflow scope, conflict diagnostics for shadowed sources,
an ordered `ruleSourceStatus` result (`active`, `shadowed`, or `disabled` with a
bounded reason), bounded fingerprints for loaded prompt/rule source metadata,
and the actual Harness context limits. Fingerprints are deterministic local
diagnostics; source text and credentials never cross the service boundary.
The regular `general` agent is now called out separately from foundational
agents: its `HarnessConfig` base prompt, output contract, context policy, and
independent prompt/rule source switches are profile inputs. The current Host
profile now groups the source switches under `RuntimeProfile.regular_agent`;
the base prompt remains the bounded `HarnessConfig` default, output formatting
stays at the frontend edge, and context limits remain a Core contract.

Verification completed on Windows: workspace `cargo check --workspace
--all-targets`, scoped Clippy with `-D warnings`, and the affected provider
seam test run pass serially (`mini-agent-capabilities` 111,
`mini-agent-host` 55, App Server 23, ACP 4, protocol 4). CLI and ACP/App
Server protocol tests prove
unavailable profile requests are rejected, and a CLI integration test proves
`ask --no-tools` sends an empty tool catalog and omits extension instructions.
Profile-file parsing and CLI integration tests also pass.
The line-budget report now separates capabilities from Host and ACP from App
Server; the current runtime total is 28,277/20,000 lines, so Stage 4 must
reduce implementation/test weight or revise the budget with explicit evidence.
Source-specific rule-body resolution (the current source fields select and
diagnoses bounded inputs rather than parse user-authored rule bodies) is
resolved by the fail-closed design: rule bodies are never accepted as an
untyped wire or profile-file input, and the effective Host policy is typed.
The stdio startup selector now resolves the requested allowlisted profile and
workspace profile before constructing the first Thread; the selected profile
is frozen for later thread-factory calls. Project and extension source
fingerprints are captured after selection, while workflow rules are represented
by the typed policy fingerprint. Cross-platform/CI evidence is intentionally
deferred. Local CLI, standalone App Server, and startup profile-file loading
are covered by bounded parsing and integration tests.
The startup callback test proves a wire profile is resolved before App Server
construction, and the selected-extension integration test proves an omitted
MCP server is not started. The complete Windows workspace test run passes
serially.

The Stage 2 migration established a real provider boundary: the new
`mini-agent-capabilities` crate now owns the image, OpenAI-compatible model,
sandbox, security, marketplace, workspace, process, web, subagent, session,
Result Store, skills, and MCP implementations. Host keeps compatibility
re-exports for existing callers, but `RuntimeBuilder` now selects model, tool,
extension discovery, and MCP loading through the capabilities registry. The
resolved profile carries an allowlisted `modelProvider` identifier. Host no
longer owns concrete Harness hands and feet; it retains profile resolution,
context/workflow composition, and application-level binding.

Provider selection now crosses the same bounded seam. The capabilities registry
publishes stable IDs for model, tool, extension, and policy providers; the
resolved `RuntimeProfile`, workspace profile file, and capability manifest carry
those IDs without carrying live provider instances or secrets. App Server and
ACP initialization accept an optional `providers` selector and fail closed when
it does not match the frozen runtime. The standalone App Server applies the
selector before constructing its first Thread, while in-process clients and ACP
verify that their request matches the already assembled profile. Only the
built-in provider IDs are available today; adding a new implementation requires
registering it in `mini-agent-capabilities` before it can be selected.

## Problem

`mini-agent-host` currently owns capability definitions, concrete implementations,
discovery, policy, and runtime assembly in one crate. The crate includes provider
adapters, workspace and process tools, skills, plugins, marketplaces, MCP,
personas, sandbox, security, sessions, images, and workflows. `RuntimeBuilder` therefore performs two different jobs:

1. define what capabilities exist and how they behave; and
2. decide which concrete capabilities a CLI invocation should load.

That coupling makes the Host heavy, makes App Server startup implicitly depend on
every extension subsystem, and makes a minimal or ACP-oriented runtime pay for
capabilities it does not select. It also makes capability selection difficult to
review: a change to a default discovery path can silently change the harness
assembled by every frontend.

## Goals

- Keep the four-layer boundary unchanged:

  ```text
  CLI client
      ↓
  App Server service boundary
      ↓
  Host / Workflows application host
      ↓
  Core / Protocol execution foundation
  ```

- Make capability selection explicit at the App Server startup seam.
- Keep capability definitions small and reusable while moving concrete loading
  behind profile adapters.
- Allow CLI, ACP, tests, and future embedders to choose different profiles
  without creating separate execution loops.
- Reduce Host startup work and make the line budget measurable by capability
  group.
- Preserve current default behavior until a profile has equivalent evidence.

## Non-goals

- Do not move provider/tool execution into Core.
- Do not make Core depend on plugins, marketplaces, MCP, sandbox, or OS policy.
- Do not serialize credentials, callbacks, filesystem handles, or concrete tools
  through App Server JSON-RPC.
- Do not introduce a general dynamic plugin runtime or an in-process scheduler.
- Do not split every current Host module into a new crate in the first stage.

## Proposed model

### 1. Declarative capability profile

Add a host-owned, edge-constructed `RuntimeProfile` (name to be finalized) that
contains only bounded selections and policy values:

```rust
pub struct RuntimeProfile {
    pub model: ModelProfile,
    pub tools: ToolProfile,
    pub extensions: ExtensionProfile,
    pub sandbox: SandboxProfile,
    pub security: SecurityProfile,
    pub persona: PersonaProfile,
    pub agent: AgentProfile,
    pub prompt: PromptProfile,
    pub rules: RuleProfile,
    pub workflows: WorkflowProfile,
    pub persistence: PersistenceProfile,
}
```

The profile is a plan for composition, not a bag of already-built trait objects.
It may contain concrete paths and bounded selectors locally, but no secrets or
live process handles. It should be cheap to clone, inspect, and include in
status diagnostics.

Suggested profile fields:

- `ModelProfile`: provider kind, model name, endpoint selection, and web-search
  mode; credentials are resolved only by the provider adapter.
- `ToolProfile`: workspace read/write, shell, web, image, process, subagent, or
  no-tool selection.
- `ExtensionProfile`: skill/plugin/marketplace/MCP selectors and explicit load
  depth (`None`, metadata only, selected entries, or enabled tools).
- `SandboxProfile`: native or Docker mode plus process limits.
- `SecurityProfile`: preset, approval mode, and sensitive-action policy.
- `PersonaProfile`: foundational role and bounded system-prompt overlays.
- `AgentProfile`: foundational agent contract (`explore`, `plan`, `general`) and
  its allowed file collaboration mode. It selects prompt behavior and output
  contracts; it does not create another execution loop.
- `PromptProfile`: bounded prompt sources and overlays for a regular agent,
  including the built-in base prompt, workspace `AGENTS.md`, selected skill
  instructions, persona text, and workflow rider text.
- `RuleProfile`: explicit behavioral rules and precedence for the regular
  agent, such as read-only/plan restrictions, approval requirements, output
  contracts, and repository-specific rules. Rules are data used by policy and
  context seams, not ad-hoc strings appended by individual tools.
- `WorkflowProfile`: Goal and Plan availability, trigger policy, verifier
  requirement, milestone/retry limits, and living-plan behavior. Goal/Plan are
  workflow services over the same Thread and Session, not alternative Harness
  implementations.
- `PersistenceProfile`: session/result-store binding. The current CLI profile
  always selects durable session persistence; an unbound profile is retained
  only for provider-free demos and low-level tests.

Use enums and named constructors for common profiles rather than boolean flags.
Initial constructors should include `interactive_default`, `ask_default`,
`auto_default`, `acp_minimal`, and `demo`.

### 2. Capability seams

Split composition into narrow seams that consume a profile and return host
artifacts:

```text
RuntimeProfile
    ├── ModelSeam       -> provider model + image projection
    ├── ToolSeam        -> ToolRouter and tool descriptors
    ├── ExtensionSeam   -> skills/plugins/marketplaces/MCP selection and loading
    ├── PolicySeam      -> sandbox + security + approval controller
    ├── ContextSeam     -> prompt/rules/world/agent/persona bounded context
    ├── WorkflowSeam    -> Goal / Plan / Mentor workflow services
    └── StateSeam       -> SessionStore + session-bound ResultStore
```

A seam is an assembly boundary, not a new execution engine. Each seam should
have one input profile and one result type with diagnostics. Concrete modules
such as `skills`, `marketplaces`, `mcp`, `workspace`, `sandbox`, and `security`
remain implementations behind those seams. Their public API should expose
bounded descriptors and factories instead of forcing callers to import all
implementation modules.

`AgentProfile`, `PersonaProfile`, and `WorkflowProfile` are resolved before the
first model request. Their prompt overlays and context items remain bounded and
stable for the lifetime of a runtime. A workflow may request a new turn or
append a bounded context item through App Server, but it cannot replace the
core loop or bypass policy seams.

### Prompt and rule configuration for regular agents

`general` is still a real agent profile, not an absence of configuration. Its
profile must explicitly select the prompt and rule sources used to construct the
model request:

```text
PromptProfile + RuleProfile
        ↓ ordered merge in ContextSeam / PolicySeam
stable system prompt + bounded rule context + capability manifest
```

The regular agent has a configuration layer distinct from a foundational agent
or a persona. It owns four bounded decisions:

| Regular-agent setting | Meaning | Owner |
| --- | --- | --- |
| `basePrompt` | Which built-in general-agent operating contract is used; the current default is the minimal coding-agent prompt carried by `HarnessConfig` | Host/Context seam |
| `promptSources` | Whether project, extension, and workflow prompt material is admitted | Host profile resolver |
| `ruleSources` | Whether project, extension, and workflow rules are admitted as policy inputs | Host/Policy seam |
| `outputContract` and `contextPolicy` | Response shape, stable-prompt behavior, per-item limits, and total context limits | Core contract, Host defaults |

`AgentKind::General` therefore means “use the configured regular-agent
contract”; it does not mean “use no prompt or rules”. `AgentKind::Explore` and
`AgentKind::Plan` add a stronger foundational contract and read-only rule, while
the regular agent starts from the general contract and composes the same
bounded source pipeline. A persona is an overlay on either contract, not a
replacement for the regular-agent settings.

The first profile-file and wire versions must expose only typed selectors for
this layer (for example a built-in `basePrompt` name, source switches, and
bounded context/output presets). They must reject arbitrary prompt bodies,
commands, paths, and credential material. A trusted local embedding may provide
an in-memory `HarnessConfig` or a future signed prompt bundle, but that input is
resolved before runtime construction and is represented at the manifest only by
its source name and fingerprint. This keeps regular-agent customization useful
without turning App Server into an unbounded prompt injection API.

The proposed source precedence is:

1. immutable Core safety and protocol rules;
2. Host policy rules for sandbox, security, approval, and workspace boundaries;
3. selected foundational agent contract;
4. selected persona overlay and file collaboration contract;
5. selected Goal/Plan/Mentor workflow rider;
6. workspace `AGENTS.md` and configured project rules;
7. selected skill/plugin instructions; the user prompt remains turn input and is
   never merged into the stable system prompt.

Later sources may add bounded context but cannot weaken an earlier safety or
policy rule. A source that conflicts with a higher-precedence rule is retained
only as a diagnostic (`shadowed` or `rejected`), never silently applied. Prompt
and rule text must use the existing per-item and total context limits; no source
may inject an unbounded file or a single item larger than the model context
contract.

`PromptProfile` should carry source identifiers, hashes, and bounded text or
loader references. `RuleProfile` should carry typed rule categories and
parameters where possible instead of a concatenated string. The resolved
manifest should report active source names and conflict counts, while omitting
prompt contents and secrets. This lets a regular agent explain which prompt and
rules are active without exposing the entire system prompt in status output.

The current host implementation provides the source admission seam,
independent prompt/rule manifest reporting, bounded local
`.agents/profile.json` loading, typed effective policy reporting, and typed
read-only enforcement for `explore` and `plan` agents. The default regular
agent's base prompt still arrives through `HarnessConfig`, while its real
prompt/rule selections are grouped under `RuntimeProfile.regular_agent`.
Output formatting remains a frontend concern and context limits remain the
Core `HarnessConfig` contract; neither is exposed as arbitrary profile text.
Source identifiers are diagnostics rather than user-authored policy text.

The first implementation can keep these seams as private modules in
`mini-agent-host`. After the dependency graph is stable, independent seams that
are reused by non-CLI embedders may move to focused crates. Avoid creating a new
crate solely to rename an existing module.

### 3. Capability assembly and effective scope

Profiles are assembled from reusable capability groups, then frozen before the
App Server starts. A frontend does not need to know how a skill, MCP server, or
sandbox is implemented; it selects a profile and applies explicit overrides.

```text
Built-in profile (interactive / ask / auto / ACP)
        + frontend overrides (--no-tools, --no-extensions, ...)
        + workspace and environment policy
        ↓
ResolvedProfile
        ├── enabled capability names
        ├── omitted capability names and reasons
        ├── extension load depth
        └── safe policy summary
```

The resolved profile produces a `CapabilityManifest` alongside the concrete
artifacts. The manifest is bounded, contains names and policy summaries only,
and is used for diagnostics and API responses. It must never contain provider
credentials, arbitrary command text, or filesystem secrets.

The effective scope should be visible at startup and through `status`:

```text
capabilities: model, workspace-read, shell, web-fetch, image, process
extensions: selected (skills=2, plugins=0, mcp=1)
sandbox: native
security: default / interactive approval
```

JSON status and App Server/ACP initialization responses should expose the same
manifest as structured fields (`enabled`, `disabled`, `extensionDepth`,
`sandbox`, `security`). This makes a restricted runtime observable instead of
leaving users to infer why a tool is unavailable.

Common frontend selection rules:

| Caller | Default profile | Explicit scope controls |
|---|---|---|
| REPL | `interactive_default` | `--no-tools`, `--no-extensions`, agent/persona/workflow, sandbox/security overrides |
| one-shot `ask` | `ask_default` | `--no-tools`, bounded agent/persona, approval and sandbox overrides |
| `auto` | `auto_default` | `--no-tools`, extension depth, Goal/Plan policy, sandbox/security overrides |
| ACP | `acp_default` (safe, bounded) | allowlisted profile, agent/persona, workflow and capability request |
| deterministic demo/tests | `demo` / `acp_minimal` | fixed fixture profile only; Goal and Plan disabled |

`--no-tools` is a scope override, not a second execution path: it resolves to a
profile with an empty `ToolProfile`, skips tool and MCP process construction,
and leaves the same Thread, Harness, App Server, event, and session semantics in
place. If an extension selector requires a tool, the resolver reports it as
disabled rather than silently re-enabling tools. The CLI should print the
effective manifest once and include it in `status --json`.

### 4. Profile resolver at the edge

The CLI resolves command-line arguments, environment, workspace files, and
built-in defaults into a `RuntimeProfile`. App Server does not rediscover the
workspace or infer a profile from command names. It receives a profile-backed
`HostRuntimeFactory` at startup and asks it to build one runtime.

For local CLI use:

```text
CLI args/env/workspace
        ↓ resolve_profile()
RuntimeProfile + safe diagnostics
        ↓ AppServerRuntime::start(profile)
HostRuntimeFactory::build(profile)
        ↓
Thread + Harness + ordered events
```

For external JSON-RPC/ACP, the wire protocol receives a bounded profile name or
capability request. The host maps that request to an allowlisted local profile;
unknown capability names fail closed. The wire layer never accepts arbitrary
filesystem paths, commands, credentials, or serialized tool implementations.

### 5. Explicit extension load depth

Extension discovery must be selected rather than implicit:

| Depth | Behavior |
|---|---|
| `None` | no skills, plugins, marketplaces, or MCP |
| `Metadata` | bounded names/descriptions only; no tool process starts |
| `Selected` | load only explicitly selected skills/plugins/servers |
| `Enabled` | selected entries become tools after approval and policy checks |

The current CLI defaults map to `Enabled` for configured capabilities, while
`acp_minimal` and deterministic tests use `None` or `Metadata`. This keeps
startup deterministic and prevents a profile from unexpectedly starting every
configured MCP server.

### 6. Agent, persona, and workflow composition

Foundational agents and personas are composable profile inputs, not separate
runtime types:

```text
AgentProfile(general)
    + PersonaProfile(reviewer)
    + WorkflowProfile(goal_with_verifier)
    ↓
bounded system prompt + file contract + workflow policy
```

The foundational agent supplies the base operating contract (`explore`, `plan`,
or `general`). A persona adds a bounded role overlay and optional collaboration
contract (`review_file` or `summary_file`). The workflow profile controls
whether `/plan`, `/goal`, mentor verification, retries, and milestone state are
available. These layers must be merged once during profile resolution so the
model receives one stable prompt foundation; they must not each append an
independent hidden system prompt on every turn.

`WorkflowSeam` owns Goal/Plan state transitions and verifier adapters, while
`ContextSeam` renders their bounded context items. Both invoke turn control via
App Server and use the shared Session/Result Store. They cannot directly call a
provider, execute a tool, or create a second Thread loop. A profile with
workflows disabled must reject `/goal`, `/plan`, and mentor workflow requests
with a visible capability-scope diagnostic.

## Migration plan

### Stage 1: Introduce profile types without behavior changes

- Add `RuntimeProfile`, capability group types, and named constructors.
- Model foundational agent, persona, and Goal/Plan workflow selection as profile
  data rather than implicit Host defaults.
- Move current CLI default calculations into `resolve_profile()`.
- Make `RuntimeBuilder` accept a profile while retaining a compatibility
  constructor for one release.
- Add `CapabilityManifest` diagnostics to startup output and `status --json`
  without exposing secrets.

### Stage 2: Extract seams inside Host

- Extract model, tools, extensions, policy, context, and state composition into
  focused provider modules; concrete model, image, sandbox, security, and
  marketplace implementations belong in `mini-agent-capabilities`, while Host
  retains profile resolution and runtime orchestration.
- Expose narrow provider descriptors/factories and keep compatibility exports
  only during migration; Host must not become the permanent owner of concrete
  capability implementations.
- Make each seam return bounded diagnostics and selected capability metadata.
- Keep prompt assembly in one `ContextSeam`; add snapshots for agent/persona/
  workflow combinations and disabled-workflow errors.
- Ensure `ResultStore` and `SessionStore` are injected through `StateSeam`.
- Remove unconditional discovery and unused artifact construction from the
  builder.

### Stage 3: Route every frontend through the same factory

- REPL, headless CLI, local App Server, JSON-RPC, and ACP select an allowlisted
  profile and use the same capability assembler.
- Add `--no-tools` and equivalent bounded wire capability requests; verify the
  effective manifest is visible to users and clients.
- App Server owns the service lifecycle; Host only builds the requested runtime.
- Preserve one Thread/Harness loop, one event stream, and one session record.
- Add profile identity to initialization/status metadata, not to model context
  unless a bounded context item explicitly requires it.

### Stage 4: Measure and split only proven seams

- Run the line-budget script by capability group.
- Move a seam to a new crate only when it has an independent consumer and a
  stable public contract.
- Keep provider, OS process, and extension implementations out of Core.

## Acceptance criteria

- Existing CLI interactive, ask, auto, demo, mentor, Goal, resume, steer, and
  follow-up behavior remains covered by the current integration suite.
- A minimal profile starts without skill/plugin/marketplace/MCP discovery and
  exposes no unselected tools.
- REPL and ACP can invoke the same capability assembler with different built-in
  profiles, and `--no-tools` produces an observable model-only scope without
  changing turn execution semantics.
- Foundational agent, persona, and Goal/Plan workflow combinations produce one
  bounded, stable prompt foundation; disabling workflows makes their commands
  fail with an explicit scope diagnostic.
- A regular `general` agent has an explicit Host-owned prompt/rule
  configuration (`RuntimeProfile.regular_agent`) plus the bounded base prompt
  and Core context limits; it reports active source identifiers, precedence
  conflicts, and context sizes without returning prompt contents or secrets.
- Human-readable startup output and structured status/App Server/ACP responses
  show the same bounded effective capability manifest.
- An extension profile loads only the requested entries and reports bounded
  diagnostics for rejected or unavailable entries.
- Sandbox and security decisions are identical for equivalent profiles.
- Session-backed result handles still reload from `session.jsonl` after restart.
- Local App Server and ACP observe the same Thread/Turn event semantics for the
  same profile.
- `mini-agent-host` line count and startup work decrease or are isolated into
  separately reported capability groups; no line-budget increase is accepted as
  an incidental result of the refactor.
- No credential, callback, process handle, or arbitrary path crosses the JSON-RPC
  boundary.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Profile becomes a second configuration language | Keep profile fields bounded, provide named constructors, and derive it from existing config once at the edge |
| Capability overrides are ambiguous (`--no-tools` vs extension selectors) | Resolve in a fixed order: built-in profile, workspace policy, then explicit deny overrides; expose disabled reasons in the manifest |
| Agent/persona/workflow overlays duplicate or destabilize context | Compose them once in `ContextSeam`, snapshot the resulting bounded prompt, and keep workflow state out of recurring hidden prompts |
| Seams become hidden execution loops | Require seams to return artifacts/diagnostics only; all turns continue through App Server and Core |
| ACP receives more power than local CLI | Use allowlisted profile names and fail-closed capability requests |
| Extension discovery behavior changes silently | Add profile snapshots and selected-capability assertions to integration tests |
| Splitting crates increases dependency depth | Start with private Host modules; extract only after an independent consumer exists |
| Persistence is accidentally made optional again | Keep durable persistence in the normal profiles and inject the session-bound ResultStore explicitly |

## Open decisions

- Whether `RuntimeProfile` belongs in `mini-agent-host` or a small host-contract
  crate alongside the App Server startup DTOs.
- Whether prompt/rule source descriptors should be shared with Core as typed
  context fragments or remain entirely in Host while Core enforces only size and
  ordering contracts.
- Whether future regular-agent base-prompt presets are needed beyond the
  current `HarnessConfig` default; arbitrary prompt/rule text remains a
  trusted embedding concern rather than a profile-file or wire concern.
- Whether App Server should become generic over a `HostRuntimeFactory` before
  or after profile seams are extracted.
- The final extension-depth names and whether `Metadata` belongs in status-only
  startup paths.
- Which profile identity and selected-capability metadata should be exposed to
  external ACP clients.

## Evidence to collect before implementation is marked complete

- [x] Profile coverage for CLI, local App Server, JSON-RPC, and ACP.
- [x] Minimal-profile and `--no-tools` tests with no extension process.
- [x] Selected-extension test showing only one MCP server is started.
- [x] Session restart test proving result handles reload from `session.jsonl`.
- [x] Windows full workspace test and affected-crate Clippy.
- [deferred] Workspace/Host line-budget reports and staged crate splitting.
- [deferred] macOS, Linux, and CI results for equivalent profiles.
