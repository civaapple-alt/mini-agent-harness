# Session 级冻结、恢复与子任务报告送达

**状态：已实现**

## 决策

主线程 Stop 冻结当前父 Session 的全部活动。App Server 先持久化 `freezing`，Gateway 再协作式中断 parent Turn、暂停活动子 Turn；排队 operation 保持原状态。所有活动 Turn 结算后，App Server 将状态转为 `frozen`。冻结期间，子任务调度、重试和自动唤醒不能启动新的 Turn。

只有用户显式 Continue 才会进入 `resuming`。Gateway 只恢复 `control_source=parent_freeze` 的子任务，排空保留的队列，再以 `turnSource=session_resume` 启动 parent 的恢复 Turn。用户或 main agent 单独暂停、停止的子任务不会因父 Session Continue 而被自动恢复。

子任务控制来源为 `main_agent`、`user_panel` 或 `parent_freeze`，随 operation 投影并跨 Turn 结算保留。每条子报告独立投影为 `reported` 或 `main_received`。`task_read` 成功返回报告时，在父 Session 的有界 sidecar 中写入幂等回执；报告与停止竞态时，已写入子 Session 的报告不会丢失，也不会在冻结期间唤醒 parent。

## 所属层与不变量

- Capabilities 的 `SessionStore` 拥有 Session 控制状态与报告读取回执；App Server 以 `session/control` 暴露状态转换并拦截冻结期间的新 Turn。Core 的单 Turn Loop 不变。
- Gateway 负责中断 parent/children、等待结算、恢复父冻结的子任务和排空原队列。Gateway 不维护第二份权威状态账本；重启后从 App Server operation、Turn 与 Session control 状态对账。
- 子任务身份仍由创建它的 parent Session 限定。控制与报告回执都绑定 Thread、operation、attempt，避免同名任务或旧 attempt 串线。
- Session control 状态只允许 `running → freezing → frozen → resuming → running`。冻结/恢复结算必须匹配稳定 `requestId`；重复结算幂等，过期 request 被拒绝。
- 报告回执以 `(child_thread_id, operation_id, attempt)` 合并最大 cursor，最多 4096 条、总计不超过 1 MiB。报告正文仍只保存在子 Session。

## 边界证据

- Capabilities 与 App Server 定向测试通过：Session freeze/resume 状态跨 SessionStore reopen 保持，过期 request 被拒绝；报告回执重读幂等；`task_read` 返回报告并持久化 `main_received`；父冻结控制来源在子 Turn 结算后仍保留；活动 parent Turn 可先接受 freeze，再等待 Gateway 中断；冻结 Session 拒绝普通 Turn，只允许 `session_resume` Turn。
- Web 定向验证通过：父冻结只暂停活动 child，保留 queued work；Continue 只恢复 parent 冻结的 child；冻结 parent 不排空队列；面板、composer 与 Python SDK 能发送并显示 Session 控制状态及报告送达状态。
- 受影响 Rust 包测试：`mini-agent-capabilities` 134 项、`mini-agent-app-server` 74 项、`mini-agent-protocol` 10 项、`mini-agent-app-server-protocol` 20 项全部通过。相关 Clippy、格式检查和 Harness 行数门禁通过。
- Web 定向测试：Gateway 3 项、SDK 2 项、前端 28 项通过；Ruff、ESLint 与 Vite build 通过。测试未调用付费模型。
- 行数门禁当前值：Core + Protocol 4768/6000，Control Plane 31773/35000，Release 45548/50000；相对 `HEAD` 的净增为 Core + Protocol 1 行、Control Plane 867 行、Release 1000 行，均在硬上限与 Release 增量门禁内。

## 剩余限制

本轮运行了受影响包与 Web 定向验证，没有运行完整工作区测试或完整 Web 测试矩阵。浏览器端人工重启与真实 App Server 进程竞态仍可在后续运行验收中补充；本次边界回归使用本地 fake/deterministic model，不访问付费服务。
