# Host `web_fetch` / `read_image` vs session, compact, and prefix cache

Status: implemented

## Context

`web_fetch` and `read_image` are host CLI tools. Core still only stores `Message::Tool` as bounded UTF-8. Image pixels are projected onto the provider request in the host adapter; they are not harness history.

Related: [context compaction](../feature/2026-08-24-context-compaction.md), [durable sessions](../feature/2026-08-24-durable-sessions-and-recovery.md), [historical provider seams](../../archived/feature/2026-08-27-deepseek-and-glm-provider-seams.md), [hard limits](2026-08-24-hard-limits-system.md).

## Decision

Keep pixels and HTTP bodies out of core. Session lifecycle restores host image bytes from disk; compaction stays text-only on the wire.

### History vs wire

| Tool | Core `Message::Tool` | Host extras |
| --- | --- | --- |
| `web_fetch` | Markdown, then core **16 KiB** head+tail | None. Checkpoint text is the truncated body. |
| `read_image` | Envelope only. Pixels are not in core. | `ImageStore` plus `session/attachments/{id}{ext}` and `{id}.json`. Wire: Responses `input_image` with a provider file id or data URL. |

`context_bytes_for` (1 MiB) counts JSON messages, not projected image bytes.

### Resume and fork

`ImageStore::bind_session_file` reloads `attachments/` (newest 16 by `att-{nanos}-{seq}`, magic-checked, ≤4 MiB). Fork copies the parent `attachments/` directory into the child session dir before bind. DeepSeek `file_id` in the envelope still wins when present; no extra Files upload on reload.

### Compaction

The compact auxiliary request sends `tools: []`. Host therefore does not project images and cannot call `web_fetch` / `read_image`. The Goal verifier is also empty-catalog.

## Consequences

- `read_image` survives process restart and `fork` without copying the original file into the workspace.
- Compact cannot attach projected image data.
- The 1 MiB JSON ceiling still does not include projected image bytes; those remain host wire payloads.
- Prefix cache: the tool catalog is stable and compaction still replaces the message prefix.

## Verification

- `bind_session_reloads_bytes_without_reupload`, `bind_session_reloads_inline_images_without_file_id`
- `forks_an_existing_session_into_a_new_session` copies `attachments/`
- `compacts_context_and_continues_the_tool_loop` asserts compaction `tools` is empty
