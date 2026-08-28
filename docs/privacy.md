# Data and privacy

Mini Agent Harness sends no product telemetry, analytics, update checks, or crash
reports. It has no project service of its own.

Inference requests go to the configured `OPENAI_BASE_URL`. A request contains
the system prompt, the current bounded conversation, tool definitions, and any
tool or file output already added to that conversation. The workspace is not
uploaded wholesale, but selected content can leave the machine when the model
reads it through a tool or it appears in command output.

Provider credentials belong in the process environment, CI secrets, or
`~/.mini-agent/.env`. This repository ignores `.env`, but mini-agent cannot
guarantee another workspace's ignore rules. Never commit provider credentials.

Interactive, one-shot `ask`, and `auto` conversation history is persisted in
durable JSONL files under `~/.mini-agent/sessions/<workspace>/<session-id>/`.
These files contain prompts, world-state context, reasoning, assistant messages,
tool calls and results,
errors, and complete settled checkpoints. They can contain source code or
secrets exposed during a turn. Review those files before sharing them. Session
files are neither encrypted nor uploaded by mini-agent.

Mentor commands send the complete latest settled checkpoint to the effective
mentor endpoint, which may differ from `OPENAI_BASE_URL`. This can include all
of the durable content described above. The bounded mentor criteria, model
output, producer model, and source checkpoint fingerprint are appended to the
session as a derived item. Mentor output is not sent to later primary turns
unless a user explicitly copies it into the conversation.

Result handles are appended to the same `session.jsonl` log and are restored when
the session is resumed. The input queue, in-flight turns, and managed-process
records remain process-local. Managed child process trees are stopped when the
CLI exits. Persistence does not make an interrupted external effect replay-safe.

Project skills and compatible plugin instructions contribute only bounded
metadata until the model chooses to read an instruction file or resource.
Stdio MCP servers are third-party local processes and may access files,
networks, or external services. Streamable HTTP MCP sends tool arguments to the
configured remote server and receives its tool results. MCP servers do not
inherit provider credentials implicitly, but explicit `env` or HTTP header
placeholders can pass selected environment values. Persistent stdio plugin
state is stored under `.agents/plugin-data/`.
