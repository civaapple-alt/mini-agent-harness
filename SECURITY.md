# Security

## Supported versions

Security fixes are made on the latest released minor version.

## Reporting a vulnerability

Do not open a public issue containing credentials, private source code, or an
unpatched vulnerability. Use the repository host's private security advisory
feature. If that feature is unavailable, contact the repository owner through
a private channel before publishing details.

## Runtime boundary

Mini Agent Harness provides sandbox process containment (`--sandbox native|docker`). Reads never prompt. The
default interactive session and TTY `ask` run writes and shell commands without
per-step approval. `/auto off` restores prompts. Noninteractive `ask` fails
closed on those tools unless `ask --auto-approve` (or `-y`). Managed-process
tools are not part of the Builtin catalog.
Unattended `auto` (unlimited steps unless `MINI_AGENT_MAX_STEPS`, compact) also skips prompts. Use `/auto off` when
you want to review each effect; use auto-approval only in a workspace you trust.

Project `.agents` content is repository-controlled input. Skill and plugin
instructions can influence model behavior. Stdio MCP
servers execute third-party code, while HTTP MCP sends arguments to a remote
service. Mini Agent Harness validates package boundaries, sanitizes the
inherited stdio environment, and asks
before MCP connection and each tool call. These are guardrails, not a sandbox;
review extension code and endpoints before approval and use auto mode only with
trusted extensions.

Direct file writes stay in the startup workspace and reject `.git` paths.
`read_file` may open files only inside the startup workspace, subject to the
same path policy as other workspace tools.
Shell commands are operating-system processes and can access anything
the current user can access. Provider requests can contain prompts,
conversation history, and tool or file content selected during the run.

### Docker policy gate

The current Docker contract is deliberately narrow: `docker info` must confirm
that the daemon is reachable, the selected workspace is bind-mounted at
`/workspace`, and container-only temporary files must not appear in the host
workspace. This is runtime evidence for the Capabilities boundary, not a claim
of complete security isolation.

Before adding Docker network, Linux capability, privilege, read-only, or CPU,
memory, and process-count limits, the change must first record an explicit
policy decision covering:

1. the threat model and the supported Docker/host platforms;
2. the exact defaults, opt-outs, and compatibility impact of each restriction;
3. how daemon availability, mount behavior, and each restriction will be
   observed in bounded tests; and
4. which failure result is returned when the policy cannot be satisfied.

The implementation may proceed only after that policy has cross-platform
evidence and a boundary test. An availability check or workspace-mount probe
alone is not sufficient evidence for a stronger sandbox claim.
