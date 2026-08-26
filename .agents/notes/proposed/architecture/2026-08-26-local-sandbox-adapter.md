# Tool Orchestrator and Pluggable Local Sandbox Execution Architecture

Status: proposed

## Context

In production agent harnesses, executing model tool calls directly against the host OS creates significant security and reliability risks (accidental filesystem corruption, credential leakage, network exfiltration, or runaway processes).

Studying the architecture of `codex-rs/core/src/tools/` reveals a clear 5-stage tool execution pipeline:
```
Model ToolCall 
  └──> ToolRouter (Namespace matching & model spec filtering)
        └──> ToolOrchestrator
              ├── 1. Approval Policy (Security Presets & Rule Matrix)
              ├── 2. Network Policy (Domain/URL verification)
              ├── 3. Sandbox Selection (Container / OS isolation strategy)
              ├── 4. Execution & Sandbox Escalation
              └── 5. Result Truncation & Event Settlement
```

This proposal establishes a cohesive **Tool Orchestrator & Sandbox Adapter Architecture** tailored for `mini-agent-harness`, faithfully adapting Codex's proven orchestrator model while honoring our strict microkernel boundaries.

---

## Architectural Decomposition: Core vs CLI Adapter

```
+-----------------------------------------------------------------------------------+
|                                 mini-agent-cli                                    |
|                                                                                   |
|  +-----------------------------------------------------------------------------+  |
|  |                            Tool Orchestrator                                |  |
|  |                                                                             |  |
|  |  +---------------------------+  +-------------------+  +-----------------+  |  |
|  |  |    Approval Controller    |  |  Network Approver |  | Sandbox Manager |  |  |
|  |  | - Presets (default/turbo) |  | - Domain whitelist|  | - Docker driver |  |  |
|  |  | - File/Cmd Rule Evaluator |  | - Immediate/Defer |  | - Bwrap driver  |  |  |
|  |  | - Session Decision Cache  |  | - SSRF Deny rules |  | - Native Host   |  |  |
|  |  +---------------------------+  +-------------------+  +-----------------+  |  |
|  +-----------------------------------------------------------------------------+  |
|                                          |                                        |
|                                          v                                        |
|  +-----------------------------------------------------------------------------+  |
|  |                      Concrete CLI Tools & Runtimes                          |  |
|  |      (WorkspaceFileTool, ShellTool, McpTool, ReadUrlTool, MentorTool)       |  |
|  +-----------------------------------------------------------------------------+  |
+-----------------------------------------------------------------------------------+
                                           |
                                           | Implements standard Tool trait
                                           v
+-----------------------------------------------------------------------------------+
|                                mini-agent-core                                    |
|                                                                                   |
|  +--------------------+        +---------------------+        +----------------+  |
|  |      Harness       | -----> |     ToolRegistry    | -----> |   dyn Tool     |  |
|  | (Run Loop & Limits)|        | (Name & Spec bounds)|        | (Pure execute) |  |
|  +--------------------+        +---------------------+        +----------------+  |
|  * Microkernel core remains 100% pure; zero sandbox, OS, or approval knowledge   |
+-----------------------------------------------------------------------------------+
```

---

## 1. Tool Orchestration Pipeline (Codex Pattern)

When `mini-agent-core` invokes a registered `Tool::execute(&self, arguments: &Value)`, the CLI tool runtime routes execution through the `ToolOrchestrator`:

### Step 1: Approval Policy & Decision Caching
The orchestrator checks the action against the active security profile and cached session decisions:
- **Cached Decisions (`ApprovalStore`)**: If the exact action (e.g. editing a file in `src/` or running `cargo test`) was previously approved for the current session, prompts are skipped.
- **Rule Evaluator (`deny` > `ask` > `allow`)**:
  - **Forbidden (`deny`)**: Fails fast with `ToolError::Rejected("Action violates security policy")`.
  - **Skip (`allow`)**: Proceeds without user intervention.
  - **NeedsApproval (`ask`)**: Requests confirmation via interactive TTY prompt or fails closed in non-interactive mode.

### Step 2: Network Policy Verification
For network-enabled tools (e.g. `read_url`, HTTP MCP tools):
- Validates the target URL/domain against host policy.
- Blocks sensitive internal metadata endpoints (e.g. `169.254.169.254`, `localhost` in restricted presets).

### Step 3: Sandbox Strategy Selection & Multi-Tier Drivers (`SandboxAttempt`)
The orchestrator determines the execution environment based on tool requirements, OS capabilities, and configuration:
- **Sandbox Preference**: Tools declare their isolation profile (e.g. `ShellTool` defaults to sandboxed; `WorkspaceFileTool` enforces strict path containment).
- **Pluggable Multi-Tier Driver Matrix**:
  1. **Tier 1: Container Isolation (`--sandbox docker`)**:
     - Spawns commands inside a scoped, volume-mounted container (`docker` / `podman`) with stripped host credentials and isolated network bridge.
  2. **Tier 2: OS-Native Lightweight Sandbox (`--sandbox native`)**:
     - **Windows (`codex-windows-sandbox` pattern)**: Combines **NTFS ACL permission masking** (restricting writes strictly to workspace roots, explicitly denying read/write on user profiles/SSH keys), **Restricted User Tokens** (stripping admin privileges), and **Windows Filtering Platform (WFP)** network boundaries.
     - **Linux (`bubblewrap` / `landlock` pattern)**: Combines unprivileged user namespaces, read-only root overlays, and kernel-level Landlock filesystem restrictions.
  3. **Tier 3: Host Execution with Watchdog Bounding (`--sandbox none`)**:
     - Direct execution on host shell (`pwsh`/`sh`) guarded by approval controller and path containment.

### Step 4: Execution, Process Lifecycle & Sandbox Escalation
- **Deterministic Process Lifecycle & Zero-Zombie Guarantee (Codex `JobObject` Pattern)**:
  - Long-running or runaway commands spawned by the agent are bound to OS-level containment groups:
    - **Windows**: Enclosed within a dedicated `JobObject` with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When a step times out, the turn is cancelled, or the harness exits, the Windows kernel atomically kills all recursive descendant processes, preventing orphaned background daemons.
    - **Linux**: Enclosed within process groups (`PR_SET_PDEATHSIG` / cgroups) for atomic process tree destruction.
- **ConPTY / Pseudo-Console Support**:
  - Provides a real pseudo-terminal (ConPTY on Windows, PTY on Unix) to stream interactive ANSI outputs and handle piped stdin/stdout seamlessly.
- **Sandbox Escalation**:
  - If a sandboxed command fails due to sandbox constraints (e.g. requires access to host tools declared under `Commands Outside Sandbox`), the orchestrator prompts the user for sandbox escalation to execute on the native host.

### Step 5: Result Bounding & Observation
- Tool outputs are bounded by UTF-8 head/tail limits ([Hard Limits](.agents/notes/implemented/architecture/2026-08-24-hard-limits-system.md)).
- Observation events (`ToolStarted`, `ToolFinished`) are recorded for deterministic trace replays.

---

## 2. Security Presets and Policy Configuration

To unify ergonomics and strict safety, the CLI supports predefined operational presets and granular customization:

### Presets

| Preset | Terminal Commands | File Access | Network Policy | Typical Use Case |
|---|---|---|---|---|
| `default` | Requires TTY approval | Restricted to workspace directory; outside paths require approval | Approval required | Standard daily interactive development |
| `full machine` | Requires TTY approval | Unrestricted read/write across local filesystem | Standard approval | System-wide refactoring or cross-workspace maintenance |
| `turbomode` | Automatic execution (no prompts) | Unrestricted read/write | Unrestricted | Isolated Docker containers, ephemeral CI/CD environments |
| `custom` | Defined by rules | Defined by rules | Defined by rules | Enterprise policies and custom project rules |

### Custom Rule Dimensions (`.mini-agent/security.json`)
```json
{
  "preset": "custom",
  "files": {
    "read": { "allow": ["crates/**", "docs/**"], "deny": ["**/.env", "**/*.pem"] },
    "write": { "allow": ["src/**", "tests/**"], "deny": [".git/**", ".github/**"] }
  },
  "terminal": {
    "allow": ["cargo *", "git diff*", "git status*"],
    "ask": ["git push*", "npm publish*"],
    "deny": ["rm -rf /*", "gh auth *"]
  },
  "sandbox": {
    "driver": "docker",
    "allow_outside_sandbox": ["curl", "git fetch"]
  },
  "mcp": {
    "allow": ["mcp__filesystem__*"],
    "ask": ["mcp__postgres__write_query"]
  }
}
```

---

## 3. Integration with Harness Operational Modes

The `ToolOrchestrator` cleanly harmonizes with all three CLI execution modes:

```
+----------------------------------------------------------------------------------------------------+
|                                      Harness Execution Modes                                       |
|                                                                                                    |
|  +---------------------------+  +-------------------------------+  +----------------------------+  |
|  |     interactive (REPL)    |  |         ask (Script)          |  |         auto (Copilot)     |  |
|  |                           |  |                               |  |                            |  |
|  | - Preset: default         |  | - Preset: default (FailClosed)|  | - Preset: turbomode/custom |  |
|  | - Human-in-the-loop       |  | - Bounded 8 steps, no compact |  | - Unbounded steps, compact |  |
|  | - Interactive TTY prompts |  | - TTY: prompts / Non-TTY: deny|  | - Loop detection active    |  |
|  | - Session approval cache  |  | - --auto flag lifts deny gate |  | - Safe inside Sandbox      |  |
|  +---------------------------+  +-------------------------------+  +----------------------------+  |
+----------------------------------------------------------------------------------------------------+
                                                  |
                                                  v
+----------------------------------------------------------------------------------------------------+
|                                 ToolOrchestrator & Sandbox Engine                                  |
|         (Approval Policy Evaluator -> Network Verifier -> Sandbox Attempt Driver -> Result)        |
+----------------------------------------------------------------------------------------------------+
```

### 1. `interactive` Mode (REPL Pair Programming)
- **Workflow**: Developer interacts step-by-step with the agent in a terminal.
- **Security Policy**: Default preset (`default`).
- **Approval Flow**: Tools run automatically by default; sensitive operations (writing outside project directory, unknown commands) trigger interactive TTY prompts. Approvals can be remembered for the entire session in `ApprovalStore`.
- **Sandbox Synergy**: When `--sandbox docker` is specified, shell tools execute in the container by default. If a command needs host access, it triggers an interactive approval prompt to escalate outside the sandbox.

### 2. `ask` Mode (Single Script Turn & CI/CD Pipelines)
- **Workflow**: Deterministic, machine-ingestible execution (`mini-agent ask [--json] "prompt"` or stdin piping).
- **Execution Limits**: Bounded (8 steps, no compaction), clean output formatting.
- **Fail-Closed Governance**:
  - **On TTY**: Interactive confirmation fallback if an action requires approval.
  - **Non-TTY (Pipes/CI)**: **Fails closed (`deny`)** on any sensitive file write, shell command, or unapproved network call unless explicitly overridden by `--auto`.
- **Sandbox Synergy**: In CI environments, `mini-agent ask --auto --sandbox docker "run tests"` grants the agent full execution freedom inside the ephemeral container while guaranteeing zero host contamination.

### 3. `auto` Mode (Long-Running Autonomous Copilot & Goal Execution)
- **Workflow**: Unattended execution across complex multi-file refactoring, debugging, or test iteration.
- **Execution Capabilities**: Unlimited steps (`MINI_AGENT_MAX_STEPS`, default 0), dynamic prefix compaction preserving recent tool work, and repetitive tool loop detection.
- **Security Challenge & Sandbox Resolution**:
  - Running unattended autonomous agents natively on host OS risks destructive side-effects.
  - **The Sandbox Solution**: In `auto` mode, the orchestrator pairs `turbomode` execution *inside* the sandbox container (`--sandbox docker`). The model operates at maximum velocity (running builds, formatting, tests) inside the isolated container, while the host filesystem and environment are 100% safeguarded.
  - Unattended sandbox escape (`Commands Outside Sandbox`) is strictly evaluated against whitelist rules; unlisted escape actions fail closed immediately without hanging.

---

## 4. Acceptance Criteria

1. **Strict Core Boundary**: `mini-agent-core` requires zero modifications; all orchestration and sandboxing code lives in `mini-agent-cli`.
2. **Deterministic Sandboxing**: When `--sandbox docker` is specified, shell tools execute within the container mount and cannot escape to host paths.
3. **Session Approval Cache**: User approvals with "always allow for this session" cache cleanly in `ApprovalStore` and do not re-prompt on identical tool calls.
4. **Fail-Closed Guarantee**: Any tool call triggering a `deny` rule or requiring ungranted approval fails closed with a clear, descriptive `ToolError`.
5. **Mode Consistency**: `ask` in non-TTY fails closed without `--auto`, while `auto --sandbox docker` operates autonomously within container boundaries.

---

## 5. Implementation Plan & Work Breakdown

1. **`crates/mini-agent-cli/src/security/`**:
   - `mod.rs`: `SecurityPreset`, `PermissionProfile`, and `SecurityConfig` loader.
   - `approvals.rs`: `ApprovalStore` with session decision caching.
   - `rules.rs`: Path, command regex, and network URL matching evaluators.
2. **`crates/mini-agent-cli/src/sandbox/`**:
   - `mod.rs`: `SandboxDriver` trait and `SandboxAttempt` context.
   - `docker.rs`: Container lifecycle and bind-mount management.
   - `native.rs`: Host process execution with scrubbed environment.
3. **`crates/mini-agent-cli/src/orchestrator.rs`**:
   - Central `ToolOrchestrator` wrapping tool invocations with approval, network checks, and sandbox dispatch.
