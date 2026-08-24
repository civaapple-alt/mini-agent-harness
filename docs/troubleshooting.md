# Troubleshooting

Start with:

```sh
mini-agent doctor
mini-agent status
```

`doctor --json` and `status --json` provide the same information for scripts.

## Missing provider configuration

Copy `.env.demo` to `.env`, set `OPENAI_API_KEY`, and confirm `OPENAI_MODEL` and
`OPENAI_BASE_URL`. Process environment values override `.env` values.

## PowerShell commands fail on Windows

mini-agent intentionally uses `pwsh`, not Windows PowerShell. Install
PowerShell 7 and confirm `pwsh` is on `PATH`; `mini-agent doctor` checks this.

## A noninteractive tool call is denied

`ask` cannot stop a script to obtain approval when stdin is not a terminal.
Use `ask --auto` only inside a workspace and execution environment you trust.

## A command produces too much output

Foreground output is captured with a hard limit. Large completed results return
a preview and a process-local handle; the model can call `read_tool_result` to
inspect a byte range or literal match. Use managed-process tools for servers and
watchers instead of a foreground shell command.

## A trace path already exists

Trace creation refuses to overwrite files. Choose a new path or explicitly
move the existing trace first.
