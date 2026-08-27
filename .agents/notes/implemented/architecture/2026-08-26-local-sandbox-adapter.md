# Local Tool Safety and Process Sandbox Adapter

Status: implemented

## Decision

Host-side tool execution stays in mini-agent-cli. The current adapter keeps
four concerns separate:

1. ApprovalController and SecurityPolicy decide whether a normalized file,
   shell, network, or subagent action is allowed, requires TTY approval, or is
   denied.
2. Workspace tools enforce workspace/path rules and bound captured output.
3. ProcessSandbox manages child-process lifetime. On Windows, native mode
   attaches the child to a Job Object; on macOS and Linux, native mode kills
   the child process group on timeout. This is process-tree cleanup, not
   filesystem or network isolation.
4. Docker mode wraps shell execution in docker run --rm -i, mounts the selected
   workspace at /workspace, and requires Docker to be available. It is selected
   explicitly with --sandbox docker; it is not an automatic fallback.

The supported sandbox values are native (default), docker, and none. none
disables the process-tree adapter but does not bypass the separate approval and
path checks.

## Operational boundaries

- Native mode is a lifecycle guard, not a security boundary.
- A child process still shares the workspace unless Docker is selected.
- --auto and turbomode change approval behavior; they do not silently enable
  Docker or make native execution isolated.
- Docker availability is checked before a Docker command runs and failure is
  reported clearly.
- The core crate has no knowledge of approval, OS processes, Docker, or paths.

## Verification

The behavior is covered by unit tests in sandbox.rs, workspace.rs, and
security.rs. Cross-OS claims require CI coverage; macOS development does not
substitute for the Windows Job Object path.

## Not implemented by this decision

This note does not claim ConPTY or PTY support, filesystem ACL isolation,
Landlock or bubblewrap, network filtering, sandbox escape escalation,
automatic Docker pairing, or a generic ToolOrchestrator framework. Those
require a separate experiment and evidence before becoming project contracts.
