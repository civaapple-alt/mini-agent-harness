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
default interactive session and TTY `ask` run writes, shell commands,
managed-process starts, and MCP without per-step approval. `/auto off` restores
prompts. Noninteractive `ask` fails closed on those tools unless `ask --auto-approve` (or `-y`).
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
