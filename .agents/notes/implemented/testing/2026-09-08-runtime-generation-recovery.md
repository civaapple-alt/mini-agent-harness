# Gateway Runtime Generation Recovery 证据

Status: implemented
Date: 2026-09-08
Scope: Gateway `SessionManager` runtime restart 与 Web Studio control-plane projection

## Decision

Gateway 在成功完成当前 Project 的 App Server 重绑后，广播有界的
`gateway/runtime/restarted` notification。消息只携带 `projectId` 和单调的
`runtimeGeneration`，不复制 Thread、Goal 或 Session 状态。Studio 收到新 generation
后清空每个 Thread 的 revision cursor，并重新读取 canonical workflow projection；
浏览器 WebSocket 不断开时也不会继续使用旧 App Server 进程的 in-memory revision。

## Evidence

Gateway 单测 `test_restart_broadcasts_runtime_generation` 验证旧 client 被停止、
新 client 完成绑定且广播 generation。前端单测验证 generation 只接受更大的值，
忽略重复或乱序通知；Studio 的 `App.jsx` handler 会清 cursor 并调用
`loadWorkflows()`。

验证命令：

```text
.venv\Scripts\python.exe -m pytest tests/gateway -q
.venv\Scripts\ruff.exe check server tests/gateway
npm run test
npm run lint
npm run build
```

当前结果：Gateway `43 passed`；前端 Node `18 passed`、Vitest `8 passed`，Lint、
Build 和 Ruff 均通过。

## Boundary and known gap

该 generation 是 Gateway 本地控制面信号，不是 App Server 公共协议字段，也不提供
事件重放。Gateway 进程完全退出时由 WebSocket reconnect 路径重新读取 workflow；
跨进程事件 replay、真实 Provider 质量和跨平台隔离仍不在本证据范围内。
