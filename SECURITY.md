# Security

## Supported versions

Security fixes are made on the latest released minor version.

## Reporting a vulnerability

Do not open a public issue containing credentials, private source code, or an
unpatched vulnerability. Use the repository host's private security advisory
feature. If that feature is unavailable, contact the repository owner through
a private channel before publishing details.

## Runtime boundary

Mini Agent Harness does not sandbox shell commands. Interactive mode asks before file
writes, shell commands, and managed-process starts. `auto` and `ask --auto`
remove those approval prompts and should run only in a disposable or otherwise
trusted workspace.

Project `.agents/skills` and `.agents/plugins` content is repository-controlled
input. Skill descriptions and instructions can influence model behavior, and
stdio MCP servers execute third-party code. Mini Agent Harness validates fixed
package boundaries, isolates invalid components, sanitizes the inherited MCP
environment, and asks before server startup and each MCP tool call. These are
guardrails, not a sandbox; review plugin code before approval and use auto mode
only with trusted plugins.

Direct file tools are confined to the startup workspace and reject `.git`
paths. Shell commands are operating-system processes and can access anything
the current user can access. Provider requests can contain prompts,
conversation history, and tool or file content selected during the run.
