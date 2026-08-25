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

Project `.agents` content is repository-controlled input. Skill, collection,
plugin, and marketplace instructions can influence model behavior. Stdio MCP
servers execute third-party code, while HTTP MCP sends arguments to a remote
service. Mini Agent Harness validates package boundaries, requires explicit
marketplace selection, sanitizes the inherited stdio environment, and asks
before MCP connection and each tool call. These are guardrails, not a sandbox;
review extension code and endpoints before approval and use auto mode only with
trusted extensions.

Direct file tools are confined to the startup workspace and reject `.git`
paths. Shell commands are operating-system processes and can access anything
the current user can access. Provider requests can contain prompts,
conversation history, and tool or file content selected during the run.
