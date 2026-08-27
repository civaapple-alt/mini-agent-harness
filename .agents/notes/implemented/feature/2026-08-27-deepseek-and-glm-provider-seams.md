# DeepSeek and GLM Coding Plan provider seams

Status: implemented

## Context

mini-agent has one OpenAI Responses adapter in the CLI host (`crates/mini-agent-cli/src/openai.rs`). It posts `{OPENAI_BASE_URL}/responses` with function tools. Vendors differ in built-in search, image input, and Files APIs. Folding those differences into core, or adding a second protocol (Chat Completions / Anthropic), would blur the portable loop.

External references that constrain the host (not copied into core):

- DeepSeek: [Vision](https://api-docs.deepseek.com/zh-cn/guides/vision), [Files API](https://api-docs.deepseek.com/zh-cn/guides/files_api), [Responses image input](https://api-docs.deepseek.com/zh-cn/guides/responses_api#image-input), [Pricing](https://api-docs.deepseek.com/zh-cn/quick_start/pricing) (`deepseek-v4-flash-vision-exp`). Official harness: `D:\gh-ws\dsh-ws\deepseek-harness` (`read_image` + Files `file_id`).
- GLM Coding Plan: [接入工具](https://docs.bigmodel.cn/cn/coding-plan/tool/others), [Codex](https://docs.bigmodel.cn/cn/coding-plan/tool/codex), [GLM-5.3](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.3), [GLM-5.3-Flash](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash). Platform-only (not Coding Plan Responses): [网络搜索](https://docs.bigmodel.cn/api-reference/%E5%B7%A5%E5%85%B7-api/%E7%BD%91%E7%BB%9C%E6%90%9C%E7%B4%A2) `POST /paas/v4/web_search`, [网页阅读](https://docs.bigmodel.cn/api-reference/%E5%B7%A5%E5%85%B7-api/%E7%BD%91%E9%A1%B5%E9%98%85%E8%AF%BB) `POST /paas/v4/reader`. Coding Plan search: Remote MCP `webSearchPrime`.

Host web/vision tools live beside this seam: `web_fetch`, `open_file`, `read_image` (`web.rs`, `image.rs`, `workspace.rs`).

## Decision

### Responses and Chat Completions as sibling protocols

The host adapter is two protocols under `crates/mini-agent-cli/src/openai/`: Responses (`responses.rs`) for text/tools, Chat Completions (`chat.rs`) for GLM vision. `OPENAI_API_KEY` / `OPENAI_MODEL` / `OPENAI_BASE_URL` select the Responses product. There is no Anthropic client and no implicit URL rewrite.

GLM-5.3-Flash vision is documented only on Chat Completions (`type: image_url`, `image_url.url`). Putting that block on a Responses user message delivers the caption text and drops the image. Image turns POST `{OPENAI_CHAT_BASE_URL}/chat/completions` when that variable is set.

| Product | Responses `OPENAI_BASE_URL` | Chat `OPENAI_CHAT_BASE_URL` | Key | Text model | Vision model |
| --- | --- | --- | --- | --- | --- |
| DeepSeek | `https://api.deepseek.com` | unset | platform API key | `deepseek-v4-flash` / `deepseek-v4-pro` | `deepseek-v4-flash-vision-exp` |
| GLM Coding Plan | `https://open.bigmodel.cn/api/v1` | `https://open.bigmodel.cn/api/coding/paas/v4` | **coding-plan** key | `glm-5.3` | `glm-5.3-flash` |

GLM Anthropic `https://open.bigmodel.cn/api/anthropic` is still unused. `OPENAI_CHAT_BASE_URL` is required for GLM `read_image`; it is never derived from `OPENAI_BASE_URL`.

### Built-in `web_search` vs host `web_fetch`

`is_official_search_endpoint` enables Responses `{"type": "web_search"}` only for `api.openai.com` and `api.deepseek.com`. BigModel stays off unless `MINI_AGENT_WEB_SEARCH=true`.

Known-URL reads use host `web_fetch` (public HTTP(S) or loopback; LAN/metadata blocked; public→loopback redirects refused). GLM platform `/paas/v4/web_search` and `/paas/v4/reader` are not host tools: they are a different product and key. Coding Plan search, if wanted, is optional HTTP MCP `https://open.bigmodel.cn/api/mcp/web_search_prime/mcp`.

### `read_image` projection

Core `Message::Tool` stays a short text envelope. Compaction and mentor requests (`tools` empty) never attach images.

DeepSeek projects images onto `function_call_output.output` as Responses `input_image` (`file_id`, or a string data URL fallback).

GLM-5.3-Flash documents Chat Completions `messages[].content[]` `{ "type": "image_url", "image_url": { "url": "<URL or data URL>" } }` ([OpenAI 兼容](https://docs.bigmodel.cn/cn/guide/develop/openai/introduction), [GLM-5.3-Flash](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash)). Responses `/responses` does not consume that block (caption arrives, pixels do not). For `glm-*` image turns, when `OPENAI_CHAT_BASE_URL` is set, the host posts Chat Completions: `role: tool` keeps the envelope text, then a user message with `type: text` + `type: image_url`.

| | DeepSeek | GLM Coding Plan |
| --- | --- | --- |
| Text model + image | ignored / placeholder | `glm-5.3` is text-only |
| Swap for that request | `…-flash` / `…-pro` → `deepseek-v4-flash-vision-exp` | `glm-5.3` → `glm-5.3-flash` |
| Bytes on the wire | `POST {base}/files` `purpose=user_data`, 7-day expiry, then `input_image.file_id` on tool output | no Files API; user-message `image_url.url` data URL |
| Later turns | reuse `file_id` | re-encode local attachment |

Files upload runs only when `OPENAI_BASE_URL` host is `api.deepseek.com`. Other hosts use `NoUpload` so `read_image` does not wait 60s on a missing `/files` API. `file_id` and data URLs are never mixed in one request.

`read_image` and `open_file` accept workspace-relative paths or, with approval, an absolute local file (for example under Pictures). `read_file` stays workspace-bound. Do not copy outside images into the project.

Windows `open_file` for images writes raw bytes to `%TEMP%\mini-agent-open\` and launches that copy. `std::fs::copy` would keep NTFS `Zone.Identifier` (Mark of the Web) from a browser download and Photos would prompt “此文件是否来自可靠来源?”. HTML and other files still open in place so relative assets keep working.

Session `attachments/` is reloaded on `bind_session_file` and copied on `fork`, so GLM inline data URLs survive resume. Compaction sends an empty tool catalog and does not project images.

### Reasoning

GLM-5.3 thinking cannot be disabled; default effort is `max`. Responses bodies for `glm-5.3*` include `"reasoning": { "effort": "max" }`. DeepSeek and OpenAI do not get that field (their defaults stay vendor-side).

### Host tools that are vendor-neutral

These are CLI tools, not vendor built-ins: `read_file`, `read_image`, `edit_file`, `write_file`, `open_file`, `web_fetch`, `shell`, process tools, subagents. Function schemas are the same on both vendors.

## Consequences

- Switching vendor is an env change plus the small host branches above, not a new crate.
- Search and page-read for GLM Coding Plan must not be “fixed” by wrapping `/paas/v4/*` with the coding-plan key.
- Screenshot, headless browser, and Codex `view_image` stay out: vision is `read_image` of an existing file plus the vendor vision model.
- User-facing copy: `docs/configuration.md`, `docs/troubleshooting.md`, README `.env` examples.
