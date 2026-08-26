# Pluggable Local Sandbox and Multi-Tier Security Permission Architecture

Status: proposed

## Context

Current shell and file tool execution in the CLI runs unsandboxed on the host machine (`pwsh` on Windows, `sh` on Unix) guarded primarily by basic interactive TTY prompts. In production, enterprise, or autonomous unattended runs, agents need a robust, configurable security model that combines:
1. **Hard Process & Environment Isolation** (Docker, bubblewrap, or OS-native sandbox containers).
2. **Granular Multi-Tier Permissions** covering File reads/writes, Network URLs, Terminal Commands, Commands Outside Sandbox, and external MCP tools.
3. **Predefined Security Presets** aligning developer ergonomics with safety requirements.

This proposal incorporates the Antigravity Security Architecture model into Mini Agent Harness, maintaining a strict microkernel boundary where [`mini-agent-core`](crates/mini-agent-core/src/tool.rs) remains pure and agnostic of security enforcement, while [`mini-agent-cli`](crates/mini-agent-cli/src/workspace.rs) manages sandboxes and approval matrices.

---

## Security Presets & Permission Matrix

### 1. Security Presets

Predefined operational profiles that balance velocity against blast-radius containment:

| Preset | Terminal Execution Policy | File Access Policy | Network Policy | Intended Use Case |
|---|---|---|---|---|
| `default` | Requires explicit manual review/approval for all shell commands | Read/write allowed strictly inside workspace directory; outside files require manual approval | Allowed URLs require approval | Standard day-to-day development with security-first defaults |
| `full machine` | Requires manual review for all terminal commands | Full read/write access across entire host filesystem without path prompts | Standard URL approval | Cross-repo or system-wide development where workspace containment is too restrictive |
| `turbomode` | Auto-executes all terminal commands without review prompts | Full read/write to all local files without review prompts | Unrestricted network access | High-trust, isolated, or disposable environments (e.g. CI/CD runners, containerized ephemeral instances) |
| `custom` | Granular override per file path, command pattern, network domain, and MCP server | Configurable | Configurable | Enterprise policy enforcement or specialized workflow tuning |

#### Custom Preset Dimensions
- **Outside-of-Folders File Policy**: `[always ask | allow | deny]`
- **Terminal Command Auto-Execution**: `[require review | always proceed]`
- **Artifact Review Policy**: `[always ask | always proceed]` (Specifies agent behavior when requesting confirmation on generated plans/specifications)

---

## Fine-Grained Permission Rules

The security controller evaluates actions against an ordered rule list (`deny` > `ask` > `allow`):

### 1. File Permissions (`File Access Rules`)
- **File Reads**:
  - `allow <path-pattern>` (e.g., `crates/**`, `docs/**`)
  - `ask <path-pattern>` (e.g., `~/.ssh/**`, `/etc/**`)
  - `deny <path-pattern>` (e.g., `**/.env`, `**/*.pem`, `**/*.key`)
- **File Writes**:
  - `allow <path-pattern>` (e.g., `src/**`, `target/scratch/**`)
  - `ask <path-pattern>` (e.g., `Cargo.toml`, `scripts/**`)
  - `deny <path-pattern>` (e.g., `.git/**`, `.github/workflows/**`)

### 2. Network Permissions (`Network Access Rules`)
- **Read URLs / Domains**:
  - `allow <url-or-domain-pattern>` (e.g., `api.github.com`, `crates.io`, `http://localhost:*`)
  - `ask <url-or-domain-pattern>` (e.g., `https://*`)
  - `deny <url-or-domain-pattern>` (e.g., `http://169.254.169.254/*` metadata service)

### 3. Terminal & Tooling Permissions
- **Terminal Commands**:
  - `allow <cmd-pattern>` (e.g., `cargo test*`, `git diff*`, `Get-ChildItem*`)
  - `ask <cmd-pattern>` (e.g., `git push*`, `Invoke-WebRequest*`)
  - `deny <cmd-pattern>` (e.g., `rm -rf /`, `gh auth *`, `vault *`)
- **Commands Outside Sandbox**:
  - Defines which commands, if any, are permitted to escape the sandbox container to run on the native host (e.g., `allow: curl`, `deny: default`).
- **MCP Tools Authorization**:
  - `allow <mcp-tool-or-server>` (e.g., `mcp__filesystem__*`, `mcp__postgres_db__read_query`)
  - `ask <mcp-tool-or-server>` (e.g., `mcp__postgres_db__write_query`)
  - `deny <mcp-tool-or-server>` (e.g., `mcp__cloud_deploy__*`)

---

## Architecture & Implementation Boundary

```
+-------------------------------------------------------------------+
|                        mini-agent-cli                             |
|                                                                   |
|  +-------------------------------------------------------------+  |
|  |                 Security & Approval Controller              |  |
|  |  (Presets: default/full-machine/turbomode/custom)           |  |
|  |  (Rule Evaluator: File / Network / Terminal / Outside / MCP)|  |
|  +-------------------------------------------------------------+  |
|                                 |                                 |
|                +----------------+----------------+                |
|                |                                 |                |
|                v                                 v                |
|  +---------------------------+     +---------------------------+  |
|  |   Native Host Execution   |     |    Sandbox Adapter        |  |
|  |  (pwsh / sh subprocess)   |     |  (--sandbox docker/bwrap) |  |
|  +---------------------------+     +---------------------------+  |
+-------------------------------------------------------------------+
                                 ^
                                 |  Pure Tool Trait
                                 v
+-------------------------------------------------------------------+
|                       mini-agent-core                             |
|                                                                   |
|   Harness Loop -> ToolRegistry -> Tool::execute(&Value)           |
|   (Zero security knowledge; zero container or OS dependencies)   |
+-------------------------------------------------------------------+
```

1. **Microkernel Core Contract**:
   - [`crates/mini-agent-core`](crates/mini-agent-core/src/tool.rs) retains its existing zero-cost `Tool` trait (`fn execute(&self, args: &Value) -> Result<String, ToolError>`).
2. **CLI Sandbox Adapters (`crates/mini-agent-cli/src/workspace.rs`)**:
   - `SandboxKind`: `None`, `Docker`, `Bubblewrap`.
   - Workspace directory is volume-mounted with copy-on-write or explicit path containment.
   - Environment variables scrubbed before execution to avoid leaking host credentials.
3. **CLI Arguments & Configuration**:
   - Flags: `--security-preset [default|full-machine|turbomode|custom]`, `--sandbox [docker|bubblewrap|none]`.
   - Local configuration file `.mini-agent/security.json` for persistent workspace permission rules.

---

## Acceptance Criteria

1. Running with `--security-preset turbomode` executes tools unattended without interactive TTY prompts.
2. Running with `--security-preset default` blocks attempts to read/write outside workspace or run shell commands without explicit user approval.
3. When `--sandbox docker` is active, shell tools spawn inside the container environment and cannot mutate host files outside the mount.
4. Commands classified under `Commands Outside Sandbox` route explicitly through the host executor only if granted permission.
5. All security checks fail closed (`deny`) in non-interactive/scripted contexts unless explicit rules or presets allow them.

---

## Risks and Mitigation

- **Container Startup Latency**:
  - *Mitigation*: Maintain a persistent warm container daemon during an interactive multi-turn session instead of creating a fresh container per tool call.
- **Platform Availability**:
  - *Mitigation*: Fall back to OS-native containment (e.g. bubblewrap on Linux, AppContainer/restricted tokens on Windows) or fail closed if requested sandbox backend is unavailable.
- **Microkernel Bloat**:
  - *Mitigation*: Core remains 100% untouched; all sandbox and security rule evaluation lives exclusively in `mini-agent-cli`.
