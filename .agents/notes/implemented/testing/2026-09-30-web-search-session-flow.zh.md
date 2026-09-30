# Web search 到网页正文的有界场景

## 场景

### Hypothesis

选择并配置一个搜索服务后，Agent 只看到统一的 `web_search` schema；提供方响应变成有界结构化来源。需要正文时，Agent 调用已有的 `web_fetch`。首次公网 URL 保持 Host 审批边界；长页面续读从同一 Session 的 `ResultStore` 读取，既不重发 HTTP 请求，也不重新审批。

### Public path

设置面：Gateway `GET|POST /api/web-search/settings` → Python SDK → App Server `web/search/settings/read|update` → Host 设置存储。工具面：Capabilities `web_search` → `web_fetch` admission/execution。

### Setup

使用临时目录测试 Host key 存储和 Session `ResultStore`，用本地 fixture 解析 DeepSeek/Exa/Kimi 响应；网页抓取由 deterministic stub 提供长正文。测试不读取真实用户 key，不访问供应商服务。

### Stimulus

1. 保存一个 provider key，读取返回的设置视图；确认只含 provider 和 credential configured flags。
2. 以 fixture 执行公共 `web_search` 工具；确认 tool name、schema 和输出字段与具体 provider 无关。
3. admission 一个公网 `web_fetch` URL，确认结果为 `ApprovalRequired`；模拟 Host 在授权后执行，得到首个 8 KiB 页面和 handle/cursor。
4. 使用 handle/cursor 再次 admission 并执行；确认续读为 `Allowed`、返回缓存正文且 fetch stub 调用计数仍为 1。
5. 重开 Session `ResultStore` 并用 cursor 读取下一页；确认正文和 URL 元数据可恢复。

### Trace and settlement

Host 设置读回不含 key value；协议序列化测试确认也不会回显密钥。公网 fetch 先进入审批要求，授权后完成；续读绕过网络，只读 Session 缓存。页面元数据包含 `kind=web_fetch`，其他工具产生的结果 handle 会被拒绝。

### Failure cases

- DeepSeek 响应没有结构化 `web_search_tool_result` 时返回失败，不从模型 prose 猜 URL。
- 超出 URL 上限或非 HTTP(S) 的搜索来源不会进入统一结果。
- provider 未配置 key 时 Host 不构造搜索 runtime；选 `none` 时也不构造。
- 不属于网页缓存的 Session result handle 不能通过 `web_fetch` 读取。
- 已过期的缓存 handle 返回有界失败，Agent 可以重新 fetch URL。

### Commands

第一条命令在 Harness 根目录执行；其余命令在 `mini-agent-web` 根目录执行。

```sh
cargo test -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server-protocol -p mini-agent-app-server
uv run pytest -q tests/gateway/test_web_search_settings.py tests/sdk/test_sdk_apis.py -k web_search
npm --prefix frontend run test:ui -- src/tests/WebActivitySummary.test.jsx src/tests/WebSearchSettingsPanel.test.jsx src/tests/ToolCard.test.jsx
node --test frontend/src/tests/webSearchApi.test.js
```

### Gap

不验证实际 provider 计费、实时排名或凭据权限；没有发起 DeepSeek、Exa、Kimi 的真实请求。Gateway/SDK 使用 mock App Server；Studio 使用组件和 API fixture，没有浏览器级实时审批操作。

## Change admission

1. **归属**：提供方协议解析属于 Capabilities；machine-wide provider 与 key 属于 Host；设置方法属于 App Server；Gateway/SDK/Studio 只转发或展示。Core 不变。
2. **已有 owner**：`web_fetch` 已拥有 URL 规范化、SSRF 检查、审批和 HTML 提取；`ResultStore` 已负责有界会话缓存和恢复。本实现复用它们。
3. **删除或替换**：移除旧的 model/provider Responses 搜索能力字段和锁定 UI；使用一个 Host provider selection 替代。
4. **行数变化**：`python3 scripts/line_budget.py --base origin/main --check-delta --json` 报告 Core + Protocol 净增量 `0`、Control Plane `+288`、Release Rust `+887`，无脚本 violations。仓库脚本当前 kernel 上限为 `6,500`，而本次调用提供的 AGENTS.md 写为 `6,000`；`origin/main` 和当前分支的 kernel 都是 `6,101`。本次没有增加该值。
5. **可见面与持久化**：新增 `web_search` schema/结构化结果和两个设置 RPC；`ResultStore` 为网页缓存记录 bounded metadata，保留原 Session 结果限制。不添加 Core event 或通用插件框架。
6. **边界证据**：协议、Host store、Capabilities admission/cache、SDK、Gateway 和 Studio 都有定向 fixture 测试。付费 API 真实交互与真实浏览器审批是剩余证据缺口。
