# Responses 传输中断与安全重试边界

状态：implemented

## Decision

Responses adapter 仅自动重试连接建立失败。连接建立后、收到完整 HTTP 响应前断开时，
服务端可能已经接收请求；SSE 开始后也可能已向用户输出部分内容。这两种情况都不自动
重放，避免不确定的重复模型调用和重复副作用。

用户显式重新发送会以原始输入发起新 Turn，不会续接已断开的 HTTP/SSE 请求。Web Studio
保留重发输入中的图片、文件、引用路径、技能和工作流，并在失败提示中说明这个边界。

## Verification

- 本地 HTTP 回归服务完整读取请求体后关闭连接；adapter 返回失败且只发送一次请求。
- 连接建立失败仍由现有回归覆盖，最多进行两次短延迟重试。
- Web Studio 回归覆盖失败提示和重发输入字段完整性。
- `cargo test -p mini-agent-capabilities`：126 项通过；对应 Clippy、格式检查通过。
- `npm test`：74 项 Node 测试、129 项 UI 测试通过；lint 和生产构建通过。
- Release Rust 行数增量为 55，低于 1,000 行门禁；使用本地假 HTTP 服务，不调用付费 provider。
