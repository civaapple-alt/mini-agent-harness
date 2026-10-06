# Responses 供应商预设与连接探测

状态：implemented；日期：2026-10-06；范围：Host、Web Studio 模型设置

## 决策

模型目录保存 Responses API Base URL。Host 在根地址后追加 `/responses`。Kimi API 与 Kimi Code 使用不同地址和模型 ID；Kimi Code 的 `/coding/v1` 预设建议 `k3-256k`。GLM Coding Plan 的 Responses 地址是 `https://open.bigmodel.cn/api/v1`；`/api/paas/v4` 与 `/api/coding/paas/v4` 属于 Chat Completions，不能用作 Responses 地址。Web Studio 为已保存的 GLM Chat Completions 地址提供一键修正。

连接测试只在用户点击后发送无工具请求。普通探测最多生成 32 个输出 token；请求有 12 秒超时和 4 KiB 响应上限。Kimi 使用低推理等级和 128 个输出 token；GLM 使用 `reasoning.effort: none` 和 128 个输出 token，避免默认推理消耗掉短探测的输出额度。界面只返回有限状态和短消息，不回传 API Key 或原始响应。

## 边界

本地模型建议不代表远端已验证。供应商可能对用户发起的探测请求计费。Mock 测试证明 Host 发出的协议字段和上限，不证明真实账号、模型权限或计费行为。

## 验证证据

- `crates/mini-agent-host/src/models.rs` 有 Kimi 与 GLM 的连接请求参数测试；Harness 提交包括 `d187fc0`、`f749979`。
- Web `ModelSettingsPanel.test.jsx` 覆盖 Kimi Code 与 GLM Responses 预设、旧地址纠正和推理映射；相关提交包括 `1965d32`、`0e3bbd6`。
- 配置边界写在 Harness `docs/configuration.md` 和 Web `docs/models.md`。本文补录没有发起真实供应商请求或重跑测试。
