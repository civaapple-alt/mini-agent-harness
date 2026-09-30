# Web Studio 上下文占比与缓存命中率呈现

- status: implemented
- date: 2026-09-30

## 决策

- 上下文来源按序列化字节占比显示为分段条和百分比；估算 Token 数保留为次级信息，避免把估算值误读成 Provider 的实际分类用量。
- 输入区收起状态显示模型窗口占用与会话累计缓存命中率。详情面板仍显示最近一次请求的 Provider 输入和缓存 Token 数。
- App Server 的每个 Turn presentation 持久化本轮所有模型响应的 Provider 用量合计。Gateway 再从有界读取到的全部 Session Turn 记录汇总，避免受 UI 历史卡片的最近 64 回合上限影响；命中率按有缓存字段报告的输入 Token 加权计算。
- 会话面板同时展示包含缓存数值的用量报告覆盖数。旧历史回合没有逐请求累计数据时标记为未跟踪，不将单个最近请求伪装成历史均值。
- 缓存字段缺失、缓存报告输入为零或 Provider 数值不一致时，命中率显示未知；Provider 明确报告缓存为零且输入大于零时显示 `0.0%`。
- 输入区按钮是上下文用量详情的折叠按钮，不是控制自动注入的开关。收起时仍显示窗口占用和缓存命中率；点击浮层外部或按 Escape 会关闭浮层，切换项目或 Thread 会重置展开状态。

## 影响范围

- Web Studio composer、工作区上下文卡、上下文用量 helper、`docs/session-context.md` 和 Unreleased Changelog。
- 在已有 Turn presentation 持久化字段内增加兼容旧数据的逐回合累计值；Gateway 只读汇总并通过会话历史响应提供聚合。Provider、工具授权和 SessionStore 写入边界不变。

## 验证

- `node --test src/tests/context_usage.test.js`：9 项通过。
- `npx vitest run src/tests/ContextUsageControl.test.jsx src/tests/PromptContextCard.test.jsx`：11 项通过。
- `uv run pytest -q tests/gateway/test_session_manager.py -k 'session_catalog_projects_context or session_context_cache_usage or session_context_cache_ratio or session_catalog_keeps_unreported_cached_usage or session_catalog_reads_bounded_history_without_web_state'`：5 项通过；覆盖超过 64 回合的完整 Session 汇总、恢复读取和安全整数边界。
- `uv run ruff check server/session_catalog.py tests/gateway/test_session_manager.py`、`npm run lint`、`npm run build`：通过；Build 保留大 chunk 体积提示。
- `cargo fmt --all`、`cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings` 和 `cargo test -p mini-agent-capabilities`：通过，156 项测试通过。
- `python3 scripts/line_budget.py`：通过；Core + Protocol `6,101/6,500`，Control Plane `35,987/38,000`，Release `52,809/55,000`。
- `git diff --check`：通过。
