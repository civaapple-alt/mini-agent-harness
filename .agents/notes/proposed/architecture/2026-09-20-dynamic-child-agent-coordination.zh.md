# 子代理动态协作与进展跟踪

- status: proposed
- date: 2026-09-20

## 实施记录（2026-09-20）

跨仓实现已进入工作区。本记录仍保持 `proposed`：报告持久化到父会话续行及 `/children` 回读的跨仓场景已通过，但完整生命周期、动态控制和重启恢复验收仍待补齐。

当前实现包括：

- 子 Session 可提交有界 `task_report`。App Server 按父 Turn、操作 ID 和 attempt 持久化报告；父代理可用带游标的 `task_read` 分页读取。
- `task_control` 支持编辑或取消排队任务、引导或取消运行任务、重试失败任务及取消顺序组。Gateway 执行控制并把有界结果合并到父会话唤醒；父 Turn 活跃时在安全边界处理，空闲时启动一轮续行。该结果是 Gateway 执行回执，App Server 的 operation 状态仍是权威。子代理不能创建下级代理。
- 顺序组按从 0 开始的序号等待前项。前项成功后放行下一项；前项失败或取消时暂停后续任务。
- 报告和终态会合并父会话唤醒。父 Turn 活跃时等待安全边界；父代理空闲时启动续跑。Gateway 重启后不会重放尚未处理的自动唤醒，但报告仍留在 App Server，可由父代理后续用 `task_read` 读取。
- WebStudio 消息流显示每个子任务的生命周期和报告。运行面板展示任务状态、报告及可用的子 Session 打开入口；项目侧栏过滤委派子 Session，并保留普通派生会话。

本次会话暴露了并发队列的边界缺陷：默认上限为 2 时，前两个并行任务已启动，后 3 个本应持久化为 `queued`；但 queued fork 将多行 prompt 写入 operation，通用文本校验拒绝了换行符，导致无子 Session 的失败回执，随后 `task_read` 找不到会话。Capabilities 现在对 operation prompt 单独校验：允许 LF、CR 和 tab，仍拒绝其他控制字符并保留 32 KiB 上限；`task_read` 的工具描述也明确 queued 项会自动排空，应优先跟踪运行中的子任务。该失败与同一 Turn 的 DeepSeek 10053 传输中断无关。

子会话查看器还会在加载旧页后自动滚回底部；抽屉内容与 transcript 存在嵌套滚动，命令预览浮层也可能越出抽屉。前端现在分页时保留可视位置，只让 transcript 滚动，并将浮层约束到抽屉范围。本次静态验证通过：`cargo fmt --all --check`、Capabilities Clippy、前端 ESLint/生产构建和 Release 行数增量门禁；Rust 增量为 11 行。没有运行测试套件，队列回放和抽屉分页仍需定向验收；本记录继续保持 `proposed`，不将未验证项标记为完成。

已通过的定向验证：Rust App Server 与 Capabilities 共 188 项测试（分别 67、121），受影响 Rust 包的 Clippy 通过；Gateway 与 SDK 共 97 项测试通过；前端相关 Vitest 17 项、ESLint 和构建通过。构建仍报告现有 640.46 kB chunk 提示。新增的跨仓场景使用真实 App Server，验证报告持久化、重复报告去重、过期 attempt 拒绝、父代理空闲续行触发及 `/children` 回读；父代理 `start_turn` 使用 stub，不调用模型。该场景 `1 passed`，Ruff、Python 编译检查通过。

跨仓场景尚未覆盖活跃父 Turn 的安全边界、并行报告交错、顺序组失败恢复、Gateway 重启后的待处理唤醒恢复及前端刷新一致性。Gateway 重启后自动重放待处理唤醒也尚未实现；持久报告可在后续回合读取。

按 2026-09-20 的预算调整，当前行数门禁通过。`python scripts/line_budget.py --base HEAD --check-delta --json` 测得 Release Rust 从 38,834 增至 39,598，增加 764 行；Control Plane 从 26,266 增至 26,775，增加 509 行。三个总量低于新硬上限，Release Rust 增量低于 1,000 行。

## 方案范围与目标行为

本节记录当前跨仓实现的设计范围。App Server/Host 拥有运行状态、控制权和持久化；WebStudio 呈现公共投影、合并待处理唤醒并发送控制请求；Core 不新增子代理调度或持久状态。

实现覆盖以下行为：

- 子代理提交有界进度报告。父代理用 `task_read` 的游标分页读取报告、生命周期和最终结果。完整工具记录留在子 Session。
- 父代理在安全 Turn 边界处理报告和终态。父代理空闲时可自动继续一轮。系统合并同一父 Session 的唤醒，不打断正在进行的模型请求。
- `task_control` 编辑或取消排队任务，向运行任务发送 steer 或 cooperative cancel，重试失败任务，或取消顺序组。父代理可继续用 `delegate_task` 添加任务。每项操作均校验父子关系并保持幂等。
- 顺序组用显式 `group_id` 和从 0 开始的连续 `sequence`。有缺口时后项等待。前项成功后放行下一项；失败或取消时暂停后续任务，直到父代理修复、重试或取消整组。
- 并发上限可配置为 1–8 个活动子任务。排队任务没有固定总量上限，但单条请求、持久事件和模型可见读取仍受各自限制。
- 消息流在首次委派处显示批次卡，分别呈现子代理名称、模式、排队、开始、报告和终态。运行面板优先显示活动和等待任务，完成项可折叠并分页。只有已有子 Session 的任务提供打开入口。项目侧栏隐藏委派子 Session，并保留普通派生会话。

Gateway 重启不会重放待处理的自动唤醒。持久报告和任务状态仍由 App Server 保存，父代理可在后续回合调用 `task_read`。

## 所有权与跨仓契约

| 层/仓库 | 唯一职责 | 明确不做 |
| --- | --- | --- |
| Core | 保持单 Session Turn Loop 和现有 bounded tool/event 契约。 | 不持久化报告、不调度子代理、不拥有父子授权。 |
| Capabilities/Host（mini-codex） | 暴露 `task_report`、游标版 `task_read` 和 `task_control`；在 Host 边界校验工具权限，并执行具体控制动作。 | 不把整段 child transcript 注入父上下文；子代理不能再创建下级代理。 |
| App Server（mini-codex） | 持久化报告与 operation 生命周期；校验父子 lineage、operation ID、attempt 和顺序组；提供有界读写 RPC、快照和事件。 | 不让 Gateway 成为任务状态权威；不依赖 Gateway 内存状态恢复报告。 |
| SDK / JSON-RPC 边界 | 映射工具结果、游标、控制请求和通知；保留身份字段与硬限制。 | 不静默丢弃游标、身份或控制意图；不把 steer/cancel 降级为无身份布尔标志。 |
| WebStudio Gateway | 将持久状态投影为 `/children` 与 `child_operation_updated`；执行 `task_control` 并把控制结果合并到待处理父唤醒，在父 Turn 边界发送 steer，或在父代理空闲时启动续跑。 | 不复制 task ledger，不绕过 App Server 校验。待处理自动唤醒保存在进程内，Gateway 重启后不会重放。 |
| WebStudio 前端 | 消费同一投影来显示批次卡、报告、运行面板和 child 打开入口；项目侧栏过滤委派 child。 | 不推导持久状态，不将 child transcript 镜像到父消息流。 |

子代理报告经 Capabilities/Host 写入 App Server。App Server 的持久 operation 与报告投影流向 Gateway，再供父消息流和运行面板使用。父代理的控制请求仍需经过 App Server/Host 校验；Gateway 只负责触发控制动作和呈现结果。Gateway 重启后报告仍可读取，但未处理的唤醒不会自动续发。

## 批次、风险与停止条件

### Batch 1：持久报告与读取

- Hypothesis：child 报告写入 canonical Session/operation 记录后，父代理可在刷新或
  Gateway 重启后增量读取，而无需复制 child history。Gateway 重启不会重放自动唤醒。
- Evidence：App Server 报告持久化/归属单测和跨仓场景已通过。仍需补报告大小、游标分页及 App Server 重启后的读取证据。
- Stop：无法从 App Server 恢复报告游标，或需要在 Gateway 添加平行 ledger。

### Batch 2：父代理唤醒与任务控制

- Hypothesis：Gateway 将报告和终态合并到父 Turn 的安全边界，父代理空闲时
  最多启动一个续跑请求；编辑、steer、取消、重试及整组取消具有一致的操作结果。
- Evidence：已通过 Gateway idle wake 单测、Rust 控制边界测试，以及一次跨仓 idle wake
  与重复报告场景；仍需活动 Turn、取消与重试的跨仓 Harness 场景。
- Stop：唤醒可能并发启动第二个父 Turn、重复执行非幂等控制，或绕过父子身份验证。

### Batch 3：顺序组与 WebStudio 投影

- Hypothesis：连续序号、缺口等待和失败暂停可由持久状态投影；消息流和运行面板
  可共用该投影展示进行中报告及全部终态。
- Evidence：并行/顺序混合场景、刷新重连、完成项分页、打开入口和侧栏过滤测试。
- Stop：UI 必须维护不同于 App Server 的生命周期或组状态才能正确恢复。

主要风险是报告引发的额外模型调用、父 Turn 活跃/空闲切换时的唤醒竞态、
运行任务 steer/cancel 与终态同时到达，以及队列无固定总量造成资源压力。每个
报告、通知、未完成任务快照和模型可见读取必须受现有硬限制或新设明确上限约束；
无限 backlog 不等于无限并发、无限模型上下文或无上限单次响应。报告密度、续跑
去重和活动/终态保留策略应先通过 Harness 场景验证；如果续跑造成重复执行或
无法明确界定资源上限，应停止自动续跑并保留可见报告供父代理下一轮读取。

## 验收证据

下列完整 Harness Scenario/Eval 验收仍待补齐。已有场景覆盖报告闭环、空闲唤醒和
Gateway 投影；新增场景需记录可观察 trace、失败反例和结果。公共单测不能替代跨层证据：

1. 多个并行 child 交错报告、开始、完成和失败；父消息流/运行面板均按 child
   operation identity 正确归属，重连后顺序和终态一致。
2. 父 Turn 活跃时报告只在安全边界可见；父 Turn idle 时报告合并为至多一次待
   续跑；多报告/重连/重复通知不得并发启动或重复续跑父 Turn。
3. 父代理修改/取消 queued 任务、steer/cancel running 任务、重试失败项、取消
   顺序组并追加方向；未授权 child 或错误父 Session 控制会被拒绝且留下诊断事件。
4. 顺序组缺少前序序号时不启动后项；成功后放行一项；失败/取消暂停后项；重试
   或明确调整后可继续；组取消后没有新 child 被启动。
5. Gateway 重启、父 Session 重开和分页后，报告与任务状态可从 App Server 恢复。Gateway
   不会重放待处理的自动唤醒；父代理可在后续回合调用 `task_read`。无 child Session 的失败项
   可见但不可打开；普通 fork 仍在项目列表中。
6. 有界性场景验证单条报告、一次 task_read、父 prompt 注入和通知缓存都受硬
   限制，且完整 child transcript 不会进入父消息流或隐式父 prompt。

## 六项变更准入

以下按 `.github/pull_request_template.md` 记录当前实现。已有一项跨仓 Harness 场景通过；其余生命周期与恢复场景仍待补齐。
基线为 core + protocol `4,663 / 6,000`、control-plane `26,266 / 30,000`、release `38,834 / 45,000`。
单次 PR 的 Release Rust 增量上限为 1,000 行。

1. **所属层：** `task_report`、`task_read` 和 `task_control` 定义在 Capabilities，Host/App Server 校验和执行控制。App Server 持久化报告和 operation 生命周期。Gateway 合并待处理唤醒并启动续跑。Core 不承担调度或持久状态。
2. **重复职责：** 实现扩展 `delegate_task`、`task_read`、`SessionOperation`、child Session lineage/catalog、`/children` 和 `child_operation_updated`。实现没有增加 Gateway task database、平行 child history 或前端状态权威。
3. **旧概念：** 实现沿用现有 delegate/read 工具和 operation replay，并补充报告、控制与顺序组语义。任务状态仍以 operation 持久记录为准。普通 fork 不属于委派任务，侧栏仍显示普通 fork。
4. **行数预算：** 实测值如下，硬上限与增量门禁通过。

   ```text
   core+protocol: 4,663 -> 4,663 (delta 0; remain 1,337)
   control-plane: 26,266 -> 26,775 (delta +509; remain 3,225)
   release Rust:  38,834 -> 39,598 (delta +764; remain 5,402)
   ```

   `python scripts/line_budget.py --base HEAD --check-delta --json` 报告 release 增长 764 行。
   新预算决策见 `.agents/notes/implemented/process/2026-09-20-deliverable-agent-system-line-budgets.zh.md`。
5. **可见面变化：** 是。新增 `task_report` 和 `task_control`，扩展 `task_read`，并增加有界持久报告、游标和跨层事件。父 prompt 不自动拼接完整 child transcript。跨仓场景验证了报告持久化、去重、过期 attempt 拒绝、父代理空闲续行触发和 `/children` 回读；报告限制、分页及重启恢复仍需验证。
6. **边界测试：** Rust App Server 与 Capabilities 共 188 项测试（67 和 121），受影响包 Clippy 通过。Gateway 和 SDK 共 97 项测试通过。前端相关 Vitest 17 项、ESLint 和构建通过；构建报告现有 640.46 kB chunk 提示。跨仓 smoke scenario 通过，覆盖真实 App Server 报告持久化、去重、过期 attempt 拒绝、父代理空闲续行触发和 `/children` 回读；续行模型调用使用 stub。活跃父 Turn、顺序组恢复、Gateway 重启唤醒恢复及 UI 刷新等完整场景仍待添加。Gateway 重启不重放待处理自动唤醒。未运行全工作区测试。

本变更跨 Rust runtime 与 WebStudio 多批次。以下准入项仍未全部满足：

- [x] 通过 release 增量门禁并记录所有三类行数。
- [x] 默认控制增长，并保持在仓库单次增量和硬上限内。
- [ ] 确认 Session/Actor/App Server 的单一状态与控制权威。
- [ ] 完成新增工具、事件、持久化和协议兼容审查。
- [x] 添加并通过一项跨层 Harness Scenario；完整状态机与恢复 Scenario/Eval 仍待补齐。
- [x] 更新两仓当前规范与变更记录；本提案仍待补齐完整生命周期与恢复验收后再晋级。

## 文档生命周期

本文件保持 `proposed`，直到完整生命周期与恢复场景通过并完成实现审查。Gateway 重启后不重放
待处理唤醒的限制已记录。稳定协议与集成说明已更新到 `mini-codex/docs/app-server.md`、
`docs/studio-integration.md`、`docs/privacy.md` 和
`mini-agent-web/docs/child-tasks.md`、`docs/troubleshooting.md`、`CHANGELOG.md`。本提案仍记录未完成的验证，不替代当前产品规范。
