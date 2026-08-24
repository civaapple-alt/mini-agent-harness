# Data and privacy

Mini Agent Harness sends no product telemetry, analytics, update checks, or crash
reports. It has no project service of its own.

Inference requests go to the configured `OPENAI_BASE_URL`. A request contains
the system prompt, the current bounded conversation, tool definitions, and any
tool or file output already added to that conversation. The workspace is not
uploaded wholesale, but selected content can leave the machine when the model
reads it through a tool or it appears in command output.

This repository ignores `.env`, but mini-agent cannot guarantee another
workspace's ignore rules. Never commit provider credentials; prefer the process
environment or a CI secret manager. Event traces are created only when
`--trace PATH` is provided, use create-new semantics, and may contain prompts,
model output, tool arguments, file content, commands, and errors. Review and
redact traces before sharing them.

Conversation history, result handles, the input queue, and managed-process
records are process-local and are not persisted. Managed child process trees
are stopped when the CLI exits.
