# 子代理多轮协作与消息流恢复

- status: proposed
- date: 2026-09-20

## 目标

父代理应能查看子代理的中间报告、在其仍运行时补充指令，并在成功完成后把评审意见交回同一个子 Session 开始新 Turn。任务状态、消息流和运行面板须能从 App Server 的持久记录恢复；主会话运行中的父 Turn 不因子报告被打断。

## 决策

- 一个子任务保持稳定的 child Thread、Session 和 operation ID。初次执行、失败重试和完成后的后续委托使用不同的 `attempt_kind`，并在同一任务卡中呈现轮次历史。
- 模型使用 `task_control` 的有界 `assign` 动作提交评审或补充指令。Gateway 读取权威子任务状态：`running`、`in_progress` 或 `awaiting_approval` 时 steer 当前 Turn；`completed` 时经 `child/task` 持久化新的 follow-up attempt，再在原 Thread 启动 Turn。`queued` 使用已有排队任务编辑动作；失败、取消和步数上限继续使用原重试语义。
- Follow-up 提示词最多 32 KiB。持久队列请求包含父子 Thread、operation ID、预期完成 attempt 和稳定 request ID；App Server/SessionStore 校验 lineage、状态与 attempt，并让重复 request ID 返回同一分配结果。并发或顺序组暂不可运行时保留 queued 状态，由现有队列排空路径启动；进程重启后从 Session operation 重建。
- child steer 在提交前先持久化 request ID reservation，再记录接受或确定未接受的结果。重启后若只剩 reservation，App Server 返回 `pending`，Gateway 刷新子 Turn 和 operation 状态，不自动重发该 ID。该路径保证至多提交一次，不保证一定送达；若状态检查与 reservation 之间子任务已完成，则返回 `not_submitted`，Gateway 可用同一 request ID 创建 follow-up。
- 子报告不是终态，报告后仍在运行的任务继续接受 steer。父会话仍将报告和终态合并唤醒；父模型调用期间不打断，空闲后至多续行一次。内部更新不伪装成用户输入或重复 steer Toast。
- 子 Session 详情只显示其本地持久活动，按本地 Turn 边界呈现，不把 fork 的父 checkpoint 混入消息流。父面板一项对应一个 child Session，显示最新阶段、报告与按 `initial`、`retry`、`follow_up` 区分的轮次记录。
- 执行段显示按 Turn 与稳定 block 顺序推导。当前执行段保持展开；仅在后续执行段开始后折叠前段。进度、失败、审批、委派和最终回复单独显示； steer 输入保留独立气泡。重启恢复沿用同一分组和输入边界。未完成提示随服务端 Turn 状态清除或更新；“查看当前 Turn”定位到本轮最新活动。
- 右侧停靠运行面板保留主消息流可见，子智能体是独立页，详情在页内返回列表。继续使用 `/children` 和 `child_operation_updated` 现有投影，不新增平行状态源或查询端点。

## 所有权与风险

Capabilities 定义有界工具形状；Host/App Server/SessionStore 校验父子归属、持久化 follow-up 意图和 attempt 轮次；Gateway 负责基于实时状态路由、排队及父唤醒；WebStudio 只投影持久状态。Core 不增加调度、存储或恢复概念。

主要风险是并发完成与 assign 的竞态，以及 Gateway 在持久化排队后、Turn 启动响应前崩溃。预期通过预期 attempt 的状态比较、稳定 request ID、operation/attempt 身份和队列重建覆盖；不允许仅靠 Gateway 内存去重。Steer reservation 可避免重试重复注入指令，但进程若在 reservation 与结果记录之间退出，调用结果仍可能不确定，且不会自动重发。

## 六项变更准入

1. **所属层：** Capabilities 定义模型可见控制动作，Host/App Server 执行授权边界与 Session 持久化，Gateway 负责调度与路由，WebStudio 呈现状态；Core 保持不变。
2. **重复职责：** 扩展已有 `task_control`、`child/task`、`SessionOperation`、Turn 启动元数据、`/children` 及其前端消费；不新增 Gateway task ledger、子 transcript 副本或查询端点。
3. **旧概念：** 保留稳定 child Session 和 operation ID，扩展 attempt 类型并复用已有队列、Turn 启动、steer 和失败重试路径；完成后的返工不再需要 fork 新 Session。
4. **行数预算：** 开工基线为 core + protocol `4,673`、Control Plane `27,082`、Release Rust `39,915`。实测分别为 `4,684`（`+11`）、`28,017`（`+935`）、`40,892`（`+977`）。Release 单 PR 增量最多 `1,000` 行，硬限为 `6,000`、`30,000`、`45,000`。证据命令为 `python scripts/line_budget.py --base ebdd0ff --check-delta --json`。
5. **可见面变化：** 是。新增有界 `assign.prompt`（32 KiB）和 `child/task queue_follow_up` 持久动作； operation/lifecycle 投影增加兼容可选的 `attempt_kind` 与 request 身份。旧记录缺少该字段时按通用历史轮次显示，不推造分类。
6. **边界测试：** 增加 Capabilities/App Server 所有权、CAS/幂等与恢复测试；Gateway 路由、并发、顺序组与重启测试；前端多轮卡片、子本地 transcript 和 live/restore 分组测试；新增不调用付费模型的跨仓 Harness Scenario。运行受影响 Rust 包测试、Clippy、格式、Web 前端验证及行数门禁；不运行全工作区测试。

## 验收

- 运行中（包括报告后仍运行）assign steer 同一 Turn；成功结束后 assign 在同 Session 开启下一 Turn；失败/取消/步数上限仍走重试。
- 并发已满时 follow-up 持久排队；重复 request、完成竞态及启动响应丢失不会重复执行；重启后队列继续排空。
- 父任务卡只显示一个稳定 child 项，轮次类型、报告和最新状态吻合；父唤醒不打断父 Turn，也不产生内部 steer 提示。
- 子详情无父 checkpoint 历史；Turn 输入、steer、活动段以及完成前后的摘要在实时和重开页面中一致。
- 侧栏抽屉的子智能体页可在主消息流仍可见时完成列表与详情往返；当前 Turn 导航落在最近活动。

## 实施证据

- Rust：`cargo fmt --all --check`；四个受影响 package 的测试共 222 项通过，Clippy 使用 `-D warnings` 通过，App Server 构建通过。
- Web Studio：Gateway、SDK 定向测试共 175 项通过；UI 定向测试 37 项和 `npm run test:unit` 74 项通过；Smoke Scenario 2 项通过；Ruff、ESLint 和 Vite build 通过。
- 行数：上述增量门禁通过，Core + Protocol、Control Plane 与 Release 总量均低于硬限。
- Smoke Scenario 通过 SessionStore 重开恢复 queued follow-up，并由真实 Gateway drain 启动后续 Turn；Gateway 单元测试覆盖 RPC 响应丢失后的持久轮次识别，以及队列启动错误的合并重试。场景不调用付费模型。

实现和跨仓证据完成并合并前，本提案保持 `proposed`。稳定用户行为写入 mini-codex App Server/Studio 文档及 WebStudio 子任务、故障排查和 changelog；完成验证后再按 notes 生命周期晋级。
