# Web Studio 上下文占比与缓存命中率呈现

- status: implemented
- date: 2026-09-30

## 决策

- 上下文来源按序列化字节占比显示为分段条和百分比；估算 Token 数保留为次级信息，避免把估算值误读成 Provider 的实际分类用量。
- 输入区收起状态显示模型窗口占用与最近一次模型请求的整体缓存命中率。详情面板保留 Provider 报告的输入和缓存 Token 数。
- 缓存命中率按最近一次 Provider 报告的 `cached_input_tokens / input_tokens` 计算，不把缓存 Token 分摊给上下文来源，也不称为会话历史平均值。
- 缓存字段缺失、输入为零或 Provider 数值不一致时，命中率显示未知；Provider 明确报告缓存为零时显示 `0.0%`。
- 输入区按钮是上下文用量详情的折叠按钮，不是控制自动注入的开关。收起时仍显示窗口占用和命中率。

## 影响范围

- Web Studio composer、工作区上下文卡、上下文用量 helper、`docs/session-context.md` 和 Unreleased Changelog。
- App Server 用量协议、持久化和工具授权均不变；指标继续以 Provider 最新一次真实报告为准。

## 验证

- `node --test src/tests/context_usage.test.js`：7 项通过。
- `npx vitest run src/tests/ContextUsageControl.test.jsx src/tests/PromptContextCard.test.jsx`：6 项通过。
- `npm run lint` 与 `npm run build`：通过；Build 保留现有大 chunk 体积提示。
- `git diff --check`：通过。
