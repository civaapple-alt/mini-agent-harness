# Web Studio 恢复请求断开后的会话接管

- status: implemented
- date: 2026-09-30

## 问题与复现证据

用户手动停止主会话后点击恢复，刷新 Web Studio 后输入区仍显示“正在恢复整个会话”。对项目
`typesafe-ai-ws` 的只读状态检查显示：Thread 没有活动 Turn，App Server Runtime 为 `idle`，
`execution_recovery` 为空；同一会话持久化的 `session_control` 仍为 `resuming`。最近一个 Shell 结果为
`cancelled by user`，与用户截图中的手动停止一致。

Gateway 在 App Server 已持久化 `resuming` 后，直接在 `/session-control` HTTP 请求中 await 恢复流程。
如果浏览器刷新导致该请求中断，恢复协程随请求取消，持久状态却没有回滚或接管任务。刷新后的只读查询
只投影 `resuming`，不会重新启动恢复；面板也禁用了 `resuming` 状态下的 Continue。

## 决策

- 把恢复编排放进 SessionManager 独立 asyncio Task，并用 `asyncio.shield` 隔离 HTTP 客户端断连；
  仍向未断连的调用者返回原恢复结果。
- 加载子任务面板时，如果 App Server 持久状态仍是 `resuming`，Gateway 使用原 request ID 接管一次
  尚未在当前进程尝试的恢复。活动中的同一任务会合并，避免重复创建 `session_resume` Turn；失败后不由
  高频轮询无限重试，显式“重试恢复”操作可再次尝试。
- `resuming` 状态在输入区与运行面板提供可点击的“重试恢复”，解决恢复因其他边界错误而退出时没有前端自助入口的问题。
- Session 状态、执行历史与检查点继续由 App Server 所有；Gateway 仅恢复已有控制意图，不增加新的会话
  状态、持久字段、协议方法或授权路径。

## 变更准入

1. **所属层：** Web Gateway 负责恢复任务接管与 HTTP 断连隔离；Web Studio 负责恢复重试入口。App Server 仍是 Session 状态与执行检查点的权威。
2. **重复职责：** 复用已有 `session_control`、`_finish_session_resume` 和 Thread 子任务状态读取；不增加第二份历史或状态账本。
3. **旧概念：** 将请求作用域恢复执行替换为按 Session 合并的后台任务；冻结任务、Session 状态和 App Server Turn loop 保持原职责。
4. **行数预算：** Core + Protocol、Control Plane 与 Release Rust 源码均无变化，净增 `0`；变更限于 Web Gateway、Studio 测试、文档和过程证据。
5. **可见面：** 不修改模型输入、工具 schema、App Server 事件或持久化格式；继续使用用户先前明确发起的恢复请求和既有 `session_resume` Turn。
6. **边界测试：** bounded Gateway 场景验证持久 `resuming` 读取后接管恢复；取消隔离、同一 Session 合并和输入区/运行面板手动重试另有回归测试。App Server 事务与 Core loop 未变化，所有证据离线运行，不调用付费 Provider。

## 验证

- `uv run pytest -q tests/gateway/test_session_manager.py -k 'continue_resume_work_survives_request_cancellation or resuming_session_read_restarts_one_gateway_resume_job or continue_resumes_only_parent_frozen_children_then_drains_queue'`：3 passed。
- `uv run pytest -q tests/gateway/test_gateway_api.py`：17 passed，包含持久 `resuming` 的公共路由接管场景。
- `npm exec -- vitest run src/tests/InputBar.test.jsx src/tests/ChildTasksPane.test.jsx`：28 passed；`npm run lint`、`npm run build` 通过，Build 保留已有的大 chunk 提示。
- `uv run ruff check` 与 `uv run ruff format --check` 覆盖 4 个变更 Python 文件，均通过；`git diff --check` 通过。
- Gateway 两个完整测试文件合跑为 135 passed、34 failed。失败集中在本次未改动的 WebSocket 发送锁和 Child reconciliation/AsyncMock 旧断言路径；定向恢复测试及完整 `test_gateway_api.py` 均通过。未单独在基线 checkout 重跑这些失败项。
- `python3 scripts/line_budget.py`：通过；Core + Protocol `6,101/6,500`，Control Plane `35,987/38,000`，Release `52,809/55,000`，Rust 净增 `0`。
