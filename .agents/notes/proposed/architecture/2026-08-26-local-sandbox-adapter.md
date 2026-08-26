# Pluggable Local Sandbox Execution for Shell Tools

Status: proposed

## Context

Current shell execution runs unsandboxed directly on the host machine (`pwsh` on Windows, `sh` on Unix) guarded only by user TTY approvals. Enterprise deployments require hard process isolation (e.g. Docker, bubblewrap, Windows AppContainers) to prevent accidental host system mutations.

## Proposal

Define a lightweight sandbox execution interface in `mini-agent-cli/src/workspace.rs`:
1. Support an optional `--sandbox [docker|bubblewrap|none]` flag.
2. Isolate file system mutations and network calls strictly inside container mounts.
3. Keep the [`Tool`](crates/mini-agent-core/src/tool.rs) contract in `mini-agent-core` completely agnostic of container orchestration.

## Acceptance Criteria

- When `--sandbox docker` is specified, shell tools execute within the mounted container workspace.
- Deterministic behavior across platforms without bloating `mini-agent-core`.

## Risks

- Platform discrepancy (Docker daemon availability on Windows vs Linux).
- Subprocess startup latency.
