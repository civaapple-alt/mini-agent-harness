# Builtin tool contract

The default model-visible Builtin tool set is deliberately small:

```text
read_file | apply_patch | shell | read_image
```

`web_fetch`, MCP tools, child-task tools, Notebook tools, background Shell
tasks, and scheduled wake-up tools are explicit Host-composed capabilities.
They are not compatibility names for the default set. `write_file` and
`edit_file` are not supported fallback tools. Use `apply_patch` for workspace
changes.

## Ownership and admission

Core resolves a registered tool by name and owns the bounded execution loop.
The protocol handler validates parameters and describes the requested action.
Host `ToolOrchestrator` applies admission, approval, and execution ordering.
The concrete `ToolRuntime` owns the side effect and its workspace and sandbox
configuration. App Server selects an allowlisted runtime composition and
projects structured outcomes. SDK, Gateway, and Web Studio only consume that
projection.

Tool admission follows this order:

```text
deny → Plan lock → workspace and sandbox → approval → execution → event and receipt
```

An approval grant does not override an earlier denial. The App Server preserves
the distinct tool lifecycle status and execution outcome so clients do not have
to infer `needs_approval`, `deferred`, or `retryable` from error text.

## Read files

`read_file` uses bounded pagination. It accepts a workspace-relative `path`
and optional `offset` and `limit`. The default page is 200 lines, the maximum
is 2,000 lines, and a result includes a `next_offset` when more content is
available. One page is bounded to about 15 KiB and the source-file read limit
is 8 MiB.

Use the returned offset to continue a long read. Do not depend on a truncated
result as a complete file.

```json
{
  "name": "read_file",
  "arguments": {"path": "src/main.rs", "offset": 0, "limit": 200}
}
```

## Apply patches

`apply_patch` accepts the Codex `*** Begin Patch` text format with Add, Update,
Move, and Delete operations. A patch is limited to 512 KiB, 16 file
operations, and 32,000 hunk lines. The runtime validates paths and hunks before
performing a side effect. If a later write fails, it attempts to roll back
completed writes.

```json
{
  "name": "apply_patch",
  "arguments": {
    "patch": "*** Begin Patch\n*** Update File: README.md\n@@\n+# Updated\n*** End Patch"
  }
}
```

Paths are relative to an admitted workspace root. The same path policy,
approval policy, and Plan-mode lock apply to the whole patch.

## Maintenance

Add a default tool only when the four existing tools cannot express a required
workflow. Define its bounded schema, admission behavior, event projection, and
scenario evidence before exposing it. Keep implementation decisions and
one-time migration evidence in `.agents/notes/`.
