# 子代理动态协作与进展跟踪

- status: proposed
- date: 2026-09-20

## 实施记录（2026-09-20）

跨仓实现已进入工作区。此记录保持 `proposed`，待变更合并后再按 notes 生命周期晋级。实现将子报告、父唤醒、父消息流、运行面板和子 Session 活动归入现有 App Server operation 与 Session 投影。

### 发现和修正

- 子报告之前通过真实 `steer` 唤醒父代理，造成重复的纠偏提示。现在父 Turn 运行时只更新批次投影；父 Turn 结束后，Gateway 合并更新并启动一轮 `turnSource: "child_wakeup"` 续行。手动 steer 在收到 `steer_ack` 后只提示一次。
- 默认并发上限为 2 时，超出的子任务应持久化为 `queued`。多行任务提示曾被通用校验拒绝，导致未创建 Session 的任务失败。operation 提示现在允许 LF、CR 和 tab，仍保留 32 KiB 上限；排队项等待空位，`task_read` 不查询排队项或不存在的 Session。DeepSeek 10053 是模型连接中断，与队列校验问题分开处理。
- 父 Turn 自动续行与新用户 Turn 可能同时通过空闲检查。Gateway 和 WebSocket Turn 提交共用按 Session 索引的启动锁。待处理更新最多保留 64 个不同子任务；每轮提交至多 16 个，队列溢出合并为数量和最多 8 个示例 ID。
- 子查看器之前显示 fork 继承的父 checkpoint。现在它只读取子 Session 本地活动项；旧 Session 没有本地项时显示空态。父 checkpoint 仍作为模型上下文和单独来源信息。
- 子任务详情之前替换主消息流。现在“子智能体”是抽屉顶层页，详情在同页内打开并可返回列表。运行中和终态任务各自突出当前阶段或最终结果。
- Turn/block 折叠此前依赖组件挂载时的局部状态。现在默认呈现由稳定 Turn 和 block 身份计算；成功活动归入摘要，进度文本、失败、审批、委派和最终回复保持独立。

### 实现边界

App Server 持久化 `turnSource` 于现有 `turn_started.presentation.turnSource` 记录，并在重开 Session 后恢复到 `/items` 投影。WebStudio 的待处理唤醒只存在于 Gateway 内存；Gateway 重启后不会重放唤醒，但 App Server 仍保留报告和 operation 状态。父代理后续可从持久投影读取状态。没有新增任务查询端点、Gateway 状态账本或子 transcript 副本。

### 验收证据

- Gateway 与 Python SDK：127 项聚焦测试通过；Gateway agent websocket：18 项通过；Ruff 通过。
- 跨仓 smoke 场景：`1 passed`。使用真实 App Server 验证报告持久化、重复和过期 attempt 处理、活动父 Turn 不被 `steer` 打断、空闲后只启动一次带来源标记的续行，以及 `/children` 回读。模型续行使用 stub，不调用付费模型。
- 前端 UI：21 个测试文件共 93 项通过；ESLint 和生产构建通过。Vite 仍报告 657.46 kB 的主 JS chunk 超过 500 kB 提示。
- Rust：App Server 69 项测试、App Server Protocol 20 项、Protocol 10 项、Capabilities 122 项通过；Workspace Clippy、`cargo fmt --all -- --check` 和文档链接检查通过。全工作区测试未运行。此前一次 App Server 测试执行在 `TurnFinished` 与最终 `RuntimeStatus::Completed` 发布之间遇到时序断言；隔离重跑 50 次和后续完整 App Server 包测试均通过。
- 行数门禁：当前值为 core + protocol `4,673 / 6,000`、Control Plane `27,082 / 30,000`、Release `39,915 / 45,000`。相对 `HEAD` 基线，增量分别为 `+10`、`+296` 和 `+306`。`python scripts/line_budget.py --base HEAD --check-delta --json` 通过，单次 release 增量上限为 1,000。

### 尚未覆盖

跨仓场景尚未逐一覆盖顺序组缺号、失败后恢复、Gateway 重启后从父代理下一轮读取，以及浏览器级刷新后的全部活动重建。Gateway 重启不重放唤醒是当前实现的边界；持久报告仍可读。完整的顺序组和进程重启场景通过后，再评估是否把本记录晋级为 `implemented`。

## 方案范围与目标行为

本节记录当前跨仓实现的设计范围。App Server/Host 拥有运行状态、控制权和持久化；WebStudio 呈现公共投影、合并待处理唤醒并发送控制请求；Core 不新增子代理调度或持久状态。

实现覆盖以下行为：

- 子代理提交有界进度报告。父代理用 `task_read` 的游标分页读取报告、生命周期和最终结果。完整工具记录留在子 Session。
- 父代理空闲后处理合并的报告和终态。Gateway 不用 `steer` 打断正在进行的模型请求；若更新在续行 Turn 运行时到达，等该 Turn 结束后再合并续行。App Server 用结构化 `turnSource: "child_wakeup"` 标记自动续行，供事件和 Session item 投影识别。
- `task_control` 编辑或取消排队任务，向运行任务发送 steer 或 cooperative cancel，重试失败任务，或取消顺序组。父代理可继续用 `delegate_task` 添加任务。每项操作均校验父子关系并保持幂等。
- 顺序组用显式 `group_id` 和从 0 开始的连续 `sequence`。有缺口时后项等待。前项成功后放行下一项；失败或取消时暂停后续任务，直到父代理修复、重试或取消整组。
- 并发上限可配置为 1–8 个活动子任务。排队任务没有固定总量上限，但单条请求、持久事件和模型可见读取仍受各自限制。
- 消息流在首次委派处显示批次卡，分别呈现子代理名称、模式、排队、开始、报告和终态。运行面板优先显示活动和等待任务，完成项可折叠并分页。只有已有子 Session 的任务提供打开入口。独立的“子智能体”页在抽屉内完成列表和详情导航。项目侧栏隐藏委派子 Session，并保留普通派生会话。
- Child transcript 只从子 Session 本地 item 投影生成；fork checkpoint 继续作为模型上下文，不作为子活动显示。Turn/block 的默认呈现由持久顺序和状态推导，以保持运行结束和重新打开后的一致性。

Gateway 重启不会重放待处理的自动唤醒。持久报告和任务状态仍由 App Server 保存，父代理可在后续回合调用 `task_read`。

## 所有权与跨仓契约

| 层/仓库 | 唯一职责 | 明确不做 |
| --- | --- | --- |
| Core | 保持单 Session Turn Loop 和现有 bounded tool/event 契约。 | 不持久化报告、不调度子代理、不拥有父子授权。 |
| Capabilities/Host（mini-codex） | 暴露 `task_report`、游标版 `task_read` 和 `task_control`；在 Host 边界校验工具权限，并执行具体控制动作。 | 不把整段 child transcript 注入父上下文；子代理不能再创建下级代理。 |
| App Server（mini-codex） | 持久化报告与 operation 生命周期；校验父子 lineage、operation ID、attempt 和顺序组；提供有界读写 RPC、快照和事件。 | 不让 Gateway 成为任务状态权威；不依赖 Gateway 内存状态恢复报告。 |
| SDK / JSON-RPC 边界 | 映射工具结果、游标、控制请求和通知；保留身份字段与硬限制。 | 不静默丢弃游标、身份或控制意图；不把 steer/cancel 降级为无身份布尔标志。 |
| WebStudio Gateway | 将持久状态投影为 `/children` 与 `child_operation_updated`；执行 `task_control` 并合并父唤醒。父 Turn 空闲后启动一次带 `child_wakeup` 来源的续行。 | 不复制 task ledger，不绕过 App Server 校验。待处理自动唤醒保存在进程内，Gateway 重启后不会重放。 |
| WebStudio 前端 | 消费同一投影来显示批次卡、报告、“子智能体”页和 child 打开入口；Turn 轨道显示自动续行来源；项目侧栏过滤委派 child。 | 不推导持久状态，不将 child transcript 镜像到父消息流，也不显示继承的父 checkpoint 活动。 |

子代理报告经 Capabilities/Host 写入 App Server。App Server 的持久 operation 与报告投影流向 Gateway，再供父消息流和运行面板使用。父代理的控制请求仍需经过 App Server/Host 校验；Gateway 只负责触发控制动作和呈现结果。Gateway 重启后报告仍可读取，但未处理的唤醒不会自动续发。

## 批次、风险与停止条件

### Batch 1：持久报告与读取

- Hypothesis：child 报告写入 canonical Session/operation 记录后，父代理可在刷新或
  Gateway 重启后增量读取，而无需复制 child history。Gateway 重启不会重放自动唤醒。
- Evidence：App Server 报告归属、重复 ID、attempt 校验和持久化测试通过；跨仓场景验证父会话从
  `/children` 投影读取状态。报告读取上限为每页 32 条、10 KiB。App Server 进程重启后的完整跨层恢复场景仍待添加。
- Stop：无法从 App Server 恢复报告游标，或需要在 Gateway 添加平行 ledger。

### Batch 2：父代理唤醒与任务控制

- Hypothesis：Gateway 不 steer 活动父 Turn。它等父 Turn 结算后，再为合并更新启动至多一次续行；编辑、steer、取消、重试及整组取消具有一致的操作结果。
- Evidence：Gateway/SDK 定向测试和跨仓场景验证活动父 Turn 不被打断、待处理更新合并、空闲后续行仅提交一次，以及用户 Turn 与自动续行由同一启动锁串行化。任务控制边界由 Rust 与 Gateway 测试覆盖；完整顺序组恢复仍需跨层场景。
- Stop：唤醒可能并发启动第二个父 Turn、重复执行非幂等控制，或绕过父子身份验证。

### Batch 3：顺序组与 WebStudio 投影

- Hypothesis：连续序号、缺口等待和失败暂停可由持久状态投影；消息流和运行面板
  可共用该投影展示进行中报告及全部终态。
- Evidence：前端 93 项测试通过，覆盖抽屉列表与详情入口、子活动范围、Turn/block 摘要和状态呈现；
  Rust 与 Gateway 测试覆盖 operation 投影及排队处理。完整顺序组恢复和浏览器刷新后端到端重建仍待覆盖。
- Stop：UI 必须维护不同于 App Server 的生命周期或组状态才能正确恢复。

主要风险是自动续行会增加模型调用，以及 Gateway 重启会丢失尚未送达的唤醒。报告和
operation 仍由 App Server 持久化。Gateway 将每个父 Session 的待处理子任务限制为 64 项，
每轮续行最多携带 16 项；溢出通知仅保留计数和最多 8 个示例 ID。相同 Session 的启动锁
串行化自动续行和用户 Turn。顺序组恢复与重启读取仍需跨层证据。

## 验收证据

下列验收分为已通过项与仍需增加的跨层场景。公共单测不能替代跨层证据。

已通过：

- 跨仓场景验证报告持久化、重复与过期 attempt 处理、活动父 Turn 不被 `steer` 打断、空闲后单次续行和 `/children` 回读。
- Gateway 测试验证更新队列上限、批次上限、排队和无 Session 失败项处理、同一父 Session 的启动互斥。
- App Server 测试验证 `child_wakeup` 来源的实时事件、回放、持久化和 Thread item 投影；fork child 的 item 投影不包含继承 checkpoint。
- 前端 UI 测试验证子代理抽屉入口、Turn/block 摘要、进度说明及最终回复保持独立。

仍需跨层覆盖：

1. 多个并行 child 交错报告、开始、完成和失败时，批次卡与运行面板名称、状态及顺序一致。
2. 顺序组缺号等待、前项失败或取消后的暂停、重试/调整后继续及整组取消。
3. App Server 重启后读取报告游标；Gateway 重启后从持久投影恢复可读状态，且不重放内存唤醒。
4. 浏览器刷新后重建子活动、父消息流批次卡和抽屉详情，并在活动项分页后保持位置。

## 六项变更准入

以下按 `.github/pull_request_template.md` 记录当前实现。完整顺序组和进程重启场景仍待补齐。
基线为 core + protocol `4,663 / 6,000`、Control Plane `26,786 / 30,000`、Release `39,609 / 45,000`。
单次 PR 的 Release Rust 增量上限为 1,000 行。

1. **所属层：** `task_report`、`task_read` 和 `task_control` 定义在 Capabilities，Host/App Server 校验和执行控制。App Server 持久化报告和 operation 生命周期。Gateway 合并待处理唤醒并启动续跑。Core 不承担调度或持久状态。
2. **重复职责：** 实现扩展 `delegate_task`、`task_read`、`SessionOperation`、child Session lineage/catalog、`/children` 和 `child_operation_updated`。实现没有增加 Gateway task database、平行 child history 或前端状态权威。
3. **旧概念：** 实现沿用现有 delegate/read 工具和 operation replay，并补充报告、控制与顺序组语义。任务状态仍以 operation 持久记录为准。普通 fork 不属于委派任务，侧栏仍显示普通 fork。
4. **行数预算：** 实测值如下，硬上限与增量门禁通过。

   ```text
   core+protocol: 4,663 -> 4,673 (delta +10; remain 1,327)
   control-plane: 26,786 -> 27,082 (delta +296; remain 2,918)
   release Rust:  39,609 -> 39,915 (delta +306; remain 5,085)
   ```

   `python scripts/line_budget.py --base HEAD --check-delta --json` 报告 release 增长 306 行。
   新预算决策见 `.agents/notes/implemented/process/2026-09-20-deliverable-agent-system-line-budgets.zh.md`。
5. **可见面变化：** 是。`turn/start`、`turn/event` 和 Thread item 投影增加结构化 `turnSource`；`child_wakeup` 续行提示最多 16 个子任务。现有报告与 operation 投影仍是状态权威，父 prompt 不拼接完整 child transcript。
6. **边界测试：** Rust App Server 69 项、App Server Protocol 20 项、Protocol 10 项、Capabilities 122 项测试通过；Workspace Clippy 和格式检查通过。Gateway 与 SDK 127 项、Gateway agent 18 项、跨仓场景 1 项通过。前端 93 项测试、ESLint 和构建通过；生产构建报告 657.46 kB 主 chunk 提示。未运行全工作区测试。

本变更跨 Rust runtime 与 WebStudio 多批次。六项准入问题均已回答；完整顺序组和重启场景仍待补齐：

- [x] 通过 release 增量门禁并记录所有三类行数。
- [x] 默认控制增长，并保持在仓库单次增量和硬上限内。
- [x] 确认 Session/Actor/App Server 的单一状态与控制权威。
- [x] 完成新增工具、事件、持久化和协议兼容审查。
- [x] 添加并通过一项跨层 Harness Scenario；完整状态机与恢复 Scenario/Eval 仍待补齐。
- [x] 更新两仓当前规范与变更记录；提案在顺序组和重启场景验收、变更合并前保持 `proposed`。

## 文档生命周期

本文件保持 `proposed`，直到顺序组和进程重启场景通过、变更合并后再晋级。Gateway 重启后不重放
待处理唤醒的限制已记录。稳定协议与集成说明已更新到 `mini-codex/docs/app-server.md`、
`docs/studio-integration.md` 和 `mini-agent-web/docs/child-tasks.md`、
`docs/troubleshooting.md`、`CHANGELOG.md`。新增的 `turnSource` 是有界来源枚举，不改变报告内容或读取权限，
因此本轮没有改动 `docs/privacy.md`。本提案记录尚未覆盖的验证，不替代当前产品规范。
