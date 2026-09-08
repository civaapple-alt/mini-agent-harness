# Fork 与并发 Attach 组合证据

Status: implemented  
Date: 2026-09-08  
Scope: `mini-agent-web/server/session_manager.py` 的 Thread fork、attach 和
Project binding 组合

## Decision

Gateway 对同一 Thread 的 client 创建、fork 和 canonical binding 使用
`SessionManager` 的同一异步锁临界区。fork 先复用已绑定的 source client，完成
App Server 分叉，再写入 child metadata 并绑定 child；并发 attach 在该序列完成
前不能创建第二个 client。Gateway 仍只是 App Server Session/Runtime 的适配层，
不新增自己的 Session authority。

## Evidence

`test_fork_and_concurrent_attach_share_the_forked_binding` 注入一个暂停中的
fork，并同时启动 child attach，观察到：

- attach 在 fork 完成前保持等待，没有提前创建 client；
- fork 完成后 child 继承 source Project binding；
- attach 返回成功，并复用同一个 source App Server client；
- `_create_client` 没有被第二次调用。

回归命令：

```text
.venv\Scripts\python.exe -m pytest tests/gateway/test_session_manager.py tests/gateway/test_gateway_goals_and_items.py -q
.venv\Scripts\ruff.exe check server tests/gateway/test_session_manager.py tests/gateway/test_gateway_goals_and_items.py
```

本次组合回归与相关 Gateway 路由测试通过；`f358316` 为实现提交。

## Boundary and known gap

该测试证明的是 Gateway 本地 fork/attach 的并发绑定顺序，不等价于 App Server
跨进程事件 replay，也不证明浏览器 WebSocket 在 Gateway 内部重启后能收到新的
generation 信号。后者仍由 Goal revision/reconnect 证据记录为未完成边界。
