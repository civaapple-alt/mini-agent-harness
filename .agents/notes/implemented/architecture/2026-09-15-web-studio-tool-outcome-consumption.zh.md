# Web Studio Tool Outcome 消费

状态：implemented
日期：2026-09-15
批次：Iteration 8，完成公共工具结果到 Web Studio 的消费链路
范围：`mini-agent-web` Python SDK、Gateway 历史投影、Web Studio 前端和 TUI

## Decision

Web Studio 的 tool block 同时保存生命周期 `status` 和运行时 `outcome`。
`outcome` 在 SDK 边界按字符串解析，当前已知值由 `KnownToolOutcome` Literal
记录，但未知字符串原值透传并在前端使用通用文案。前端只负责归并和展示，不
触发 retry、approval 或本地执行。

## Harness hypothesis

如果 live event、Item history 和 Session catalog 都进入同一个有界 tool block，
Web Studio 就不会把 `retryable`、`deferred` 或新增未知状态误报成普通失败，也
不会因为诊断文本改变而改变控制行为。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| outcome 产生 | Core/Host/Capabilities | 保持现有结构化结果 | Web 根据文本重新分类 |
| 公共投影 | App Server Item/事件 | 已有 `outcome` 继续透传 | Gateway 创建第二份状态权威 |
| SDK 边界 | Python `ThreadItem`/事件解析 | 已知值提供 Literal，字符串值前向兼容 | 对未知值抛协议错误 |
| Web Studio | `messageState`、`ToolCard` | 保留状态并选择展示文案 | 自动 retry 或 approval |

## Cross-repository contract

`status` 仍是 `inProgress`、`completed`、`failed` 三值生命周期。`outcome`
使用服务端的 snake_case 字符串。旧 Item 缺失该字段时前端保持旧行为；未来
未知字符串保存原值，UI 显示“未知状态”，不降级为失败。

## Verification

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-agent-web | `npm test` | node 53 passed；Vitest 36 passed |
| mini-agent-web | `npm run build` | passed；Vite 仅报告既有 chunk size warning |
| mini-agent-web | `npm run lint` | passed |
| mini-agent-web | `uv run pytest -q tests/sdk/test_sdk_events.py tests/gateway/test_session_manager.py tests/tui/test_tui_rendering.py` | 79 passed；1 个既有 Windows asyncio 资源析构 warning |
| mini-agent-web | `uv run ruff check ...` | passed |

## Consequences

- live `tool_finished` 和历史 `ThreadItem` 现在在前端使用同一 outcome 语义。
- SDK 不再把 forward-compatible 的未知字符串类型伪装成已知 Literal。
- 本批没有新增执行接口、授权缓存或 Web 本地控制权。

## Remaining risks

- Stopping 阶段的 EOF/reconnect 恢复尚未完成，本批只验证普通 live/history 归并。
- Web Studio 仍没有自动重试和审批续接按钮，这些动作必须继续走 App Server。
- 本批没有真实 provider、MCP transport 或完整端到端浏览器故障注入。
