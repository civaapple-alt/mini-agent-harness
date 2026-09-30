# Web search and page reading

Mini Agent exposes one model-facing `web_search` tool. Host selects its concrete
provider and the Capabilities adapter normalizes results to bounded URLs,
titles, snippets, and publication dates. Providers do not add their own
model-visible tools or prose answers. With provider `none`, or without the
selected provider's key, `web_search` is not exposed.

When Host loads a selected provider and key, a new Thread enables the paired
`web_fetch` tool by default. You can disable `web_fetch` in that Thread's
Workspace tools. An already running Thread keeps its current tool catalog.

## Provider settings

Web Studio's **Settings → Web search** page selects one provider or disables
search. Keys are write-only and are stored by Host separately from the chat
model catalog:

| Provider | Search endpoint | Adapter behavior |
| --- | --- | --- |
| DeepSeek native | `https://api.deepseek.com/anthropic/v1/messages` | Calls DeepSeek's Anthropic-compatible Messages API with the native `web_search_20250305` server tool. The adapter reads structured search-result blocks and citations; it does not extract URLs from model prose. |
| Exa | `https://api.exa.ai/search` | Requests search results and highlights, then normalizes `results[]`. |
| Kimi Basic | `https://api.moonshot.cn/v1/tools/search` | Calls Basic search with `include_content=false` so the shared `web_fetch` tool owns page reading. |

The Kimi search key is separate from the key and Responses endpoint used for
Kimi chat inference. Selecting a search provider does not change the chat model.
Kimi Pro search is not used by this integration; its results already contain
provider-extracted body chunks, while this flow keeps one common page-reading
path.

The provider selection is saved in `~/.mini-agent/web_search.json`. Keys are
plaintext files under `~/.mini-agent/web-search-credentials/`; Unix permissions
are restricted to `0700` for that directory and `0600` for files. This storage
is not encrypted. App Server and Gateway responses expose only which keys are
configured. Updating settings affects Threads whose runtime is built after the
change; an already running Thread keeps its current tool catalog.

Search calls use the selected provider directly. There is no cross-provider
fallback. Provider, transport, or response-format errors return a bounded tool
failure. Selecting a provider and entering its key opts the Agent into search
requests that may incur provider charges.

## Search, fetch, and continuation

1. `web_search` returns up to 10 structured results. The Agent can cite a result
   URL directly or pass that URL to `web_fetch` when it needs page text.
2. A new public URL goes through the existing Host URL checks and approval
   policy. Interactive policy asks before fetching. Automatic and Trusted
   policies still validate the URL and network destination. Search snippets
   and page contents are untrusted input.
3. Short pages return in one tool result. Longer pages are stored in the
   Session-owned `ResultStore`; `web_fetch` returns an 8 KiB page plus an opaque
   handle and cursor. The Agent continues by calling `web_fetch` with that
   handle and cursor. Continuation reads the cached result, does not fetch the
   URL again, and does not ask for approval again.
4. Session-backed handles survive Session resume. A handle can expire when its
   result is evicted by the existing bounded cache; the tool returns a failure
   so the Agent can fetch the URL again if needed.

Web Studio shows search result links and deduplicated page links in the main
activity stream even when activity details are collapsed. It reports running,
approval, completion, and failure states. Tool output details hide cache
handles and cursors from the UI.

## Bounds and API

Search query size is at most 2,000 bytes. Results are limited to 10 entries;
URLs are limited to 2,000 bytes, titles to 256 characters, and snippets to 640
characters. Provider responses are capped at 1 MiB and requests time out after
35 seconds. Page fetch bounds are in [Harness limits](limits.md).

The App Server methods are `web/search/settings/read` and
`web/search/settings/update`. Updates accept `provider` and optional
`deepseekApiKey`, `exaApiKey`, or `kimiApiKey`; an empty key removes it. The
response contains `provider` and `*ApiKeyConfigured` flags only. Web Studio
uses the Gateway route `GET|POST /api/web-search/settings`; the Python SDK
exposes `get_web_search_settings()` and `update_web_search_settings()`.

Provider references:

- [DeepSeek Anthropic API and supported structured search blocks](https://api-docs.deepseek.com/guides/anthropic_api/)
- [Exa Search API](https://docs.exa.ai/reference/search)
- [Kimi web search best practices](https://platform.kimi.com/docs/guide/best-practices-for-web-search)
