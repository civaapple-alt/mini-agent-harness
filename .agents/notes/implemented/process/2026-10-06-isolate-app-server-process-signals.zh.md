# SDK 与 App Server 子进程信号隔离

状态：implemented；日期：2026-10-06；范围：Python SDK、Gateway 进程生命周期

## 决策

`MiniAgentClient` 启动 App Server 时，将子进程放入独立的操作系统进程组或会话。Unix 使用 `start_new_session`；Windows 使用 `CREATE_NEW_PROCESS_GROUP`。终端 Ctrl+C 因此先交给 Gateway，让它运行正常清理流程，再通过 SDK 的 `stop()` 结束并回收 App Server。

SDK 启动的子进程仍由 SDK 管理。调用方应在正常退出路径调用 `stop()`。隔离信号组不会替代进程回收，也不保证父进程遭遇强制终止时子进程会自动退出。

## 原因

Gateway 与 App Server 同时收到终端中断，会让 App Server 在 Turn 尚未落盘或结算时被直接杀死。进程组隔离保留了 Gateway 处理退出和通知 SDK 子进程的机会。

## 验证证据

- 实现位于 `sdk/python/src/mini_agent/client.py`；使用说明位于 `sdk/python/README.md`。
- Harness 提交：`1ed1738`。该提交没有新增专门的进程信号集成测试；本文补录没有运行 SDK 套件或实际中断 Gateway。
