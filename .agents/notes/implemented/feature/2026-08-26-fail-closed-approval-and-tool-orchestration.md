# Fail-Closed Approval Controller and Tool Orchestration

Status: implemented

## Context

Autonomous coding agents require robust safety boundaries before executing potentially destructive external actions (e.g. executing shell scripts, overwriting files, or invoking external MCP tools). In non-interactive CI/script contexts, prompt-based approval easily hangs processes or creates security vulnerabilities.

## Decision

The host tool orchestration layer implements a comprehensive safety and permission matrix via [`ApprovalController`](../../../../crates/mini-agent-capabilities/src/workspace.rs):

1. **Fail-Closed by Default**:
   - In non-interactive (non-TTY) environments, sensitive tools (shell commands, file modifications, MCP actions) immediately fail closed with an explicit error unless `--auto` is explicitly provided.
2. **Interactive TTY Interception**:
   - In interactive REPL mode, sensitive operations prompt the operator with `approve {action}? [y/N]`. Users can switch to full copilot speed with `/auto` or restore per-step approval via `/auto off`.
3. **Workspace Path Containment**:
   - All file tools canonicalize paths and reject traversal escapes (e.g. `../../`) or operations touching `.git/` metadata.
4. **Unknown Tool Error Projection**:
   - Non-existent tool calls are intercepted by the registry and returned as structured errors to the model, allowing recovery-capable models to self-correct in subsequent steps without halting the run.

## Consequences

- Secures headless/script workflows from runaway destructive operations.
- Provides predictable, transparent approval mechanics across both interactive and automated execution modes.
