# Host `web_fetch` / `read_image` vs session, compact, and prefix cache

Status: proposed

## Context

`web_fetch` and `read_image` are host CLI tools (`crates/mini-agent-cli/src/web.rs`, `image.rs`). Core still only stores `Message::Tool` as bounded UTF-8. Image pixels are projected onto the provider request in the host adapter (`openai/responses.rs`, `openai/chat.rs`); they are not harness history.

This proposal records how those tools interact with context accounting, resume, fork, trace, session summary, compaction, and provider prefix-cache hits — and which host-only follow-ups are justified. Related shipped notes: [context compaction](../../implemented/feature/2026-08-24-context-compaction.md), [durable sessions](../../implemented/feature/2026-08-24-durable-sessions-and-recovery.md), [provider seams](../../implemented/feature/2026-08-27-deepseek-and-glm-provider-seams.md), [hard limits](../../implemented/architecture/2026-08-24-hard-limits-system.md).

## What is true today

| Tool | Core `Message::Tool` | Host extras |
| --- | --- | --- |
| `web_fetch` | Markdown, then core **16 KiB** head+tail | None. Checkpoint text is the truncated body. |
| `read_image` | Envelope (`<path>`, `<mini_agent_image id=… file_id? …>`). Pixels are not in core. | `ImageStore` (RAM, FIFO 16). Optional write to `session/attachments/`. Wire: DeepSeek `file_id`, GLM data URL (≤4 images, 8 MiB inline). |

`context_bytes_for` (1 MiB request ceiling) counts system prompt + JSON messages + tool specs. It does **not** count projected image bytes.

`ImageStore::bind_session_file` sets `attachments/` for future writes. `get()` is in-memory only; resume does not reload disk. `SessionStore::fork` copies checkpoint messages, not `attachments/`.

Compaction (`auto` / `ContextLimitBehavior::Compact`) keeps the latest context item and the last two assistant groups (≤128 KiB), summarizes the prefix, and currently passes the **full tool catalog**. Host then `attach_images = !request.tools.is_empty()`, so a compact request can attach live images and GLM compact can switch to Chat Completions. Mentor already uses an empty catalog, so it never projects images.

## Proposal

Keep pixels and HTTP bodies out of core. Close three host gaps so session lifecycle and cache behavior match the envelope-only contract.

### 1. Resume and fork restore GLM pixels from disk

On `bind_session_file`, load `attachments/{id}{ext}` (and `{id}.json` when a `file_id` exists) into `ImageStore` up to `MAX_STORED_IMAGES`. On fork, copy the parent `attachments/` directory into the child session dir (or reload from parent then re-bind). DeepSeek turns that already carry `file_id` in the envelope stay as they are.

Without this, GLM `read_image` works only until process restart; resume/fork show the envelope and then `Missing`.

### 2. Compaction and mentor stay text-only on the wire

Pass an empty tool list on compaction requests (mentor already does). Then:

- images are not projected (`attach_images` is false);
- the summarizer cannot call `web_fetch` / `read_image` (today a tool-calling summary is discarded and mechanical trim runs);
- GLM compact stays on Responses, not Chat Completions.

The compact prompt already says not to call tools; the catalog should match.

### 3. Do not pretend projected bytes are inside the 1 MiB JSON ceiling

Do not add a core concept for images. Optionally, in the host adapter, skip or placeholder-project images when building a compaction request even if tools were non-empty — redundant if (2) ships. A host-side inline-byte cap already exists (`MAX_INLINE_REQUEST_BYTES` 8 MiB); leave the 1 MiB JSON ceiling as a core safety bound, not a tokenizer.

### Out of scope

- Screenshot, browser, or Codex `view_image`.
- Putting image bytes into `session.jsonl`, trace JSONL, or `summary.json`.
- A second persistence framework, cache API, or core `Message` variant.
- Changing `web_fetch`’s 16 KiB core projection (same class as `read_file` / shell previews).

## Impact map (no code required to accept this reading)

**Context.** Two extra tool schemas on every request (small, stable). Each `web_fetch` can add ~16 KiB to JSON history. Image envelopes are cheap in JSON; GLM data URLs are expensive only on the wire. Several fetches in the 128 KiB compact tail can push an older group into the summarized prefix.

**Resume / fork.** Fetch text restores. Image envelopes restore. GLM pixels do not, unless (1) ships. DeepSeek `file_id` survives until Files API expiry (7 days). Resume invalidation text is about process IDs and result handles, not `att-*`.

**Trace.** `ToolStarted` / `ToolFinished` carry truncated tool text (envelope or ≤16 KiB markdown). No pixels. Many fetches fatten JSONL; images do not. `trace summary` counts `tool_calls_truncated` only.

**Summary.** `summary.json` / `signals.json` are metadata indexes; pixels never appear. Compaction summaries see fetch markdown and image envelopes, not pixels (unless compact projects images — (2) removes that).

**Compact.** Recent fetch/image groups usually stay in the tail. Prefix summaries cannot see GLM pixels after resume. Full tool catalog on compact is the main leak into protocol-switch and data-URL cost.

**Prefix cache.** Stable system prompt and stable tool catalog remain cache-friendly after the one-time catalog change. GLM image turns already miss cache versus the previous text turn: different model (`glm-5.3` → `glm-5.3-flash`) and different endpoint (Responses → Chat Completions). DeepSeek `file_id` is small and reusable; GLM data URLs are unique suffixes. Compaction intentionally replaces the message prefix; system-prompt cache can still hit. `cached_input_tokens` from the two protocols are not comparable turn-to-turn.

## Acceptance criteria

- Resume and fork of a GLM session that called `read_image` in-process still attach a live data URL on the next image-bearing request (store hit, not `Missing`), without copying the file into the workspace.
- Forked child has its own `attachments/` (copy or re-bind); parent files are not mutated.
- DeepSeek resume still sends `file_id` from the envelope when present; no extra Files upload.
- Compaction requests send `tools: []` (or equivalent empty catalog). Tests show no `input_image` / `image_url` on compact bodies, and no GLM Chat Completions URL.
- Mentor behavior unchanged (already empty tools).
- `web_fetch` history, resume, trace, and compact remain truncated text; no new core limit.
- `python scripts/line_budget.py` still under 20k/30k. Host-only changes.

## Risks

- Reloading attachments can revive images the FIFO 16 would have dropped in RAM; cap reloads to the same 16, newest-first.
- Copying attachments on fork duplicates up to 16 × 4 MiB on disk; that is session-local, not workspace, and is preferable to silent vision loss.
- Empty tools on compact slightly changes summarizer behavior (cannot call tools even if it ignores the prompt). That is the intended fallback path today when it does call tools.
- Not counting data URLs in core `context_bytes` remains a lying ceiling for GLM image turns; fixing that in core would violate the host/core split. Document in `docs/limits.md` if (2) ships without a host request-size check on compact.
