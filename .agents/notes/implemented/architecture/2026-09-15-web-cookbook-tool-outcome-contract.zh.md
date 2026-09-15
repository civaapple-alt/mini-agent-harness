# Web Cookbook Tool Outcome 契约

状态：implemented  
Date: 2026-09-15  
Batch: Cookbook 与 SDK 契约同步，Batch 1  
Scope: mini-agent-web Python Cookbook 与 SDK 消费证据

## Decision

Live Cookbook 示例通过 SDK 的 typed event 和 item projection 读取工具结果，
并将生命周期 `status` 与工具 `outcome` 分开展示。离线协议示例覆盖五种已知
outcome，并验证未来 outcome 字符串不会被转换成失败。

本批没有修改 SDK API、wire field、审批规则、重试路径或 Web Studio 执行行为。
示例仍保持独立，Live 示例仍需要 App Server 和 Provider 凭证。

## Harness hypothesis

如果 Cookbook 直接消费 typed outcome 契约，开发者就不会把旧的
`is_error` 到 `OK/ERROR` 映射复制到 Gateway 或 Web Studio。未来的服务端
outcome 也会继续对消费者可见。

## Ownership and boundaries

- App Server 与 mini-codex Host/Capabilities 负责 admission、审批、重试、执行
  和权威 outcome。
- Python SDK 负责解析并保留 `ToolFinishedEvent.outcome` 与
  `ThreadItem.outcome`。
- Cookbook 只展示 SDK projection，不审批工具、不重试调用、不在本地执行。

## Cross-repository contract

`ThreadItem.status` 继续表示生命周期，`ThreadItem.outcome` 继续表示工具结果。
已知 outcome 为 `completed`、`failed`、`needs_approval`、`deferred` 和
`retryable`。未知字符串保持原值透传。

## Verification

- `uv run python cookbook/python-demo/06_protocol_compatibility.py`：通过，包含
  五个已知和一个未知 outcome fixture。
- `uv run pytest -q tests/cookbook tests/sdk`：41 passed。
- `uv run ruff check cookbook/python-demo sdk/python tests/cookbook tests/sdk`：通过。
- `uv run ruff format --check cookbook/python-demo sdk/python tests/cookbook tests/sdk`：
  18 个文件已格式化。
- `git diff --check`：通过，仅有 Windows 换行提示。
- 未调用真实 Provider，未启动 App Server，未修改公共协议。

## Remaining risks

- Live 示例只做编译校验，不进入默认的无 Provider 测试路径。
- Batch 2 仍需补离线恢复、Session fork 冲突、EOF 和独立执行策略示例。
