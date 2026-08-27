# DeepSeek and GLM Coding Plan provider seams

Status: implemented

## Context

mini-agent has one OpenAI Responses adapter in the CLI host (`crates/mini-agent-cli/src/openai.rs`). It posts `{OPENAI_BASE_URL}/responses` with function tools. Vendors differ in built-in search, image input, and Files APIs. Folding those differences into core, or adding a second protocol (Chat Completions / Anthropic), would blur the portable loop.

External references that constrain the host (not copied into core):

- DeepSeek: [Vision](https://api-docs.deepseek.com/zh-cn/guides/vision), [Files API](https://api-docs.deepseek.com/zh-cn/guides/files_api), [Responses image input](https://api-docs.deepseek.com/zh-cn/guides/responses_api#image-input), [Pricing](https://api-docs.deepseek.com/zh-cn/quick_start/pricing) (`deepseek-v4-flash-vision-exp`). Official harness: `D:\gh-ws\dsh-ws\deepseek-harness` (`read_image` + Files `file_id`).
- GLM Coding Plan: [接入工具](https://docs.bigmodel.cn/cn/coding-plan/tool/others), [Codex](https://docs.bigmodel.cn/cn/coding-plan/tool/codex), [GLM-5.3](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.3), [GLM-5.3-Flash](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash). Platform-only (not Coding Plan Responses): [网络搜索](https://docs.bigmodel.cn/api-reference/%E5%B7%A5%E5%85%B7-api/%E7%BD%91%E7%BB%9C%E6%90%9C%E7%B4%A2) `POST /paas/v4/web_search`, [网页阅读](https://docs.bigmodel.cn/api-reference/%E5%B7%A5%E5%85%B7-api/%E7%BD%91%E9%A1%B5%E9%98%85%E8%AF%BB) `POST /paas/v4/reader`. Coding Plan search: Remote MCP `webSearchPrime`.

Host web/vision tools live beside this seam: `web_fetch`, `open_file`, `read_image` (`web.rs`, `image.rs`, `workspace.rs`).

## Decision

### One Responses client

Both vendors use the existing adapter. `OPENAI_API_KEY` / `OPENAI_MODEL` / `OPENAI_BASE_URL` select the product. There is no Anthropic client and no Chat Completions path.

| Product | Base URL | Key | Text model | Vision model |
| --- | --- | --- | --- | --- |
| DeepSeek | `https://api.deepseek.com` | platform API key | `deepseek-v4-flash` / `deepseek-v4-pro` | `deepseek-v4-flash-vision-exp` |
| GLM Coding Plan | `https://open.bigmodel.cn/api/v1` | **coding-plan** key (not a general platform key) | `glm-5.3` | `glm-5.3-flash` |

GLM Chat Completions URL `https://open.bigmodel.cn/api/coding/paas/v4` and Anthropic `https://open.bigmodel.cn/api/anthropic` are the wrong endpoints for this host: the client always posts `{base}/responses`.

### Built-in `web_search` vs host `web_fetch`

`is_official_search_endpoint` enables Responses `{"type": "web_search"}` only for `api.openai.com` and `api.deepseek.com`. BigModel stays off unless `MINI_AGENT_WEB_SEARCH=true`.

Known-URL reads use host `web_fetch` (public HTTP(S) or loopback; LAN/metadata blocked; public→loopback redirects refused). GLM platform `/paas/v4/web_search` and `/paas/v4/reader` are not host tools: they are a different product and key. Coding Plan search, if wanted, is optional HTTP MCP `https://open.bigmodel.cn/api/mcp/web_search_prime/mcp`.

### `read_image` projection

Core `Message::Tool` stays a short text envelope. The host projects images onto `function_call_output.output` as `input_image` blocks. Compaction and mentor requests (`tools` empty) never attach images.

| | DeepSeek | GLM Coding Plan |
| --- | --- | --- |
| Text model + image | ignored / placeholder | `glm-5.3` is text-only |
| Swap for that request | `…-flash` / `…-pro` → `deepseek-v4-flash-vision-exp` | `glm-5.3` → `glm-5.3-flash` |
| Bytes on the wire | `POST {base}/files` `purpose=user_data`, 7-day expiry, then `input_image.file_id` | no Files API; `input_image.image_url` data URL |
| Later turns | reuse `file_id` | re-encode local attachment |

Files upload runs only when `OPENAI_BASE_URL` host is `api.deepseek.com`. Other hosts use `NoUpload` so `read_image` does not wait 60s on a missing `/files` API. `file_id` and data URLs are never mixed in one request.

`read_image` accepts workspace-relative paths or, with approval, an absolute local file (for example under Pictures). `read_file` stays workspace-bound. Do not copy outside images into the project.

### Reasoning

GLM-5.3 thinking cannot be disabled; default effort is `max`. Responses bodies for `glm-5.3*` include `"reasoning": { "effort": "max" }`. DeepSeek and OpenAI do not get that field (their defaults stay vendor-side).

### Host tools that are vendor-neutral

These are CLI tools, not vendor built-ins: `read_file`, `read_image`, `edit_file`, `write_file`, `open_file`, `web_fetch`, `shell`, process tools, subagents. Function schemas are the same on both vendors.

## Consequences

- Switching vendor is an env change plus the small host branches above, not a new crate.
- Search and page-read for GLM Coding Plan must not be “fixed” by wrapping `/paas/v4/*` with the coding-plan key.
- Screenshot, headless browser, and Codex `view_image` stay out: vision is `read_image` of an existing file plus the vendor vision model.
- User-facing copy: `docs/configuration.md`, `docs/troubleshooting.md`, README `.env` examples.
