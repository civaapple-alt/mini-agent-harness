# 子任务多轮协作、自主控制与恢复

- status: implemented
- date: 2026-09-20
- implementation: 2026-09-26

## 决策

App Server 持久化的 Child operation 是任务状态和控制结果的唯一依据。Capabilities 提供有界的 `task_list`、`task_read` 与 `task_control`；Gateway 执行控制和调度；WebStudio 展示同一状态投影。Core 保持单 Turn Loop，不持有任务账本或恢复逻辑。

主线程可分页读取最多 32 个子任务摘要，再用 `task_read` 读取报告。每个子任务控制绑定 `child_thread_id`、`operation_id`、正数 `attempt` 和稳定 request ID。Gateway 在执行前核验 operation 身份和 attempt；过期请求返回可见结果。主线程的 `task_control` 与运行面板动作进入同一个 Gateway 控制服务。

排队任务可修改提示词或取消。运行任务可 steer、暂停或停止，也可排入一条后续指令。后续指令不覆盖当前提示词；每个 operation 最多保留一条。重复 request ID 返回原结果；队列满时明确拒绝。当前 attempt 成功后，Gateway 在同一 Child Session 启动后续 Turn；失败时保留受阻指令，重试成功后再启动。取消当前 operation 时同时取消待执行指令。

暂停和停止先将控制请求写入 Session operation，再协作式中断当前 Turn。`pausing` 与 `cancelling` 在 Turn 结算前占用并发槽位。App Server 确认结算后才投影为 `paused` 或 `cancelled` 并释放槽位。继续操作沿用相同 Session、operation 和 attempt；重试沿用 Session 与 operation，并递增 attempt。

Gateway 重启后从 operation 和 Turn 状态重新连接活动任务、重发待确认中断并排空队列。处理中的请求继续显示为处理中，直到服务端投影确认最终状态。旧 operation 记录通过默认字段兼容读取；Gateway 不建立并行状态账本。

子报告和终态合并为父 Session 的待处理唤醒。父 Turn 运行时不因子报告被中断；父 Turn 结束后再续行。子 Session 详情只显示子 Session 自己的活动，不把 fork 的父 checkpoint 混入消息流。初次执行、重试与 follow-up 在同一任务卡中分轮呈现。

## 所有权与边界

- Capabilities 定义有界模型工具和分页结果。
- App Server/SessionStore 校验 lineage、operation、attempt、控制 ID，并持久化状态迁移。
- Gateway 统一父工具与用户面板的控制路由、并发槽位、队列排空和重启恢复。
- WebStudio 展示服务端确认结果，不缓存第二份任务状态。
- Core 保持单一 Turn Loop，不承担任务调度和持久化。

稳定用户行为见 Harness 的 `docs/app-server.md` 和 Web 的 `docs/child-tasks.md`。面板交互决策另记于 Web 的子任务控制 notes。

## 六项变更准入

1. **所属层：** Capabilities、App Server、Gateway 和 WebStudio；Core 不变。
2. **重复职责：** 扩展现有 `SessionOperation`、`task_control`、Child RPC 与运行面板，没有增加任务账本或第二个执行循环。
3. **旧概念：** 复用稳定 Child Session、operation、attempt、队列和协作式中断；暂停恢复不创建新 operation，重试也不 fork Session。
4. **行数预算：** 验证结果和命令记录在下方。Harness 上限为 Core + Protocol `6,000`、Control Plane `35,000`、Release `50,000`；Release 单次变更增量上限 `1,000`。
5. **可见面变化：** 增加分页 `task_list`、暂停/继续/停止/后续指令操作，以及 `pausing`、`cancelling`、`paused` 和 follow-up 状态。新字段使用兼容默认值读取旧记录。
6. **边界证据：** App Server 测试覆盖分页、幂等、持久化、暂停结算、同 Session 继续和重试；Gateway、SDK、前端和无付费模型跨层场景覆盖调度、用户控制与状态恢复。

## 验证

- `cargo fmt --all --check` 通过。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- `cargo test -p mini-agent-capabilities -p mini-agent-app-server-protocol -p mini-agent-app-server` 通过，共 216 项：Capabilities 125、App Server 71、Protocol 20。
- `cargo build -p mini-agent-app-server` 通过。
- Web Gateway/SDK 定向测试通过 149 项；使用该 App Server 构建的跨层场景通过 2 项，没有调用付费模型。
- Web 前端通过 74 项 Node 测试和 128 项 Vitest 测试；ESLint、Ruff 和 Vite build 通过。Vite 报告主 JS chunk 为 708.10 KB，超过 500 KB 提示线。
- `python3 scripts/line_budget.py` 通过：Core + Protocol 为 `4,767/6,000`，Control Plane 为 `30,692/35,000`，Release 为 `44,007/50,000`。
- `python3 scripts/line_budget.py --base c48ae273aad7f26dd444c0545bc33699a7c45ada --check-delta --json` 通过：Release 增量 `970/1,000`，Control Plane 增量 `692`。

Web 面板按钮、排队上限、错误反馈和刷新投影由 Web 定向测试覆盖。Gateway/App Server 场景使用实际构建的二进制验证持久 follow-up 恢复和队列排空。
