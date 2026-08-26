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

### Step 3: Sandbox Strategy Selection (`SandboxAttempt`)
The orchestrator determines the execution environment based on tool requirements and configuration:
- **Sandbox Preference**: Tools declare whether they require sandboxing (e.g. `ShellTool` defaults to sandboxed; `WorkspaceFileTool` uses path containment).
- **Driver Selection**:
  - `Docker`: Spawns commands inside a scoped, volume-mounted container with stripped host credentials.
  - `Bubblewrap` (Linux) / `AppContainer` (Windows): Lightweight OS namespace and process token isolation.
  - `Native Host`: Direct execution via `pwsh`/`sh`.

### Step 4: Execution & Escalation
- Executes the tool inside the selected sandbox attempt.
- If a sandboxed command fails due to sandbox constraints (e.g. needs access to host tools declared under `Commands Outside Sandbox`), the orchestrator prompts the user for sandbox escalation to run on the native host.

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

## 3. Acceptance Criteria

1. **Strict Core Boundary**: `mini-agent-core` requires zero modifications; all orchestration and sandboxing code lives in `mini-agent-cli`.
2. **Deterministic Sandboxing**: When `--sandbox docker` is specified, shell tools execute within the container mount and cannot escape to host paths.
3. **Session Approval Cache**: User approvals with "always allow for this session" cache cleanly in `ApprovalStore` and do not re-prompt on identical tool calls.
4. **Fail-Closed Guarantee**: Any tool call triggering a `deny` rule or requiring ungranted approval fails closed with a clear, descriptive `ToolError`.

---

## 4. Implementation Plan & Work Breakdown

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
