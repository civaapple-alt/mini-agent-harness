# 子任务 Turn 连续性与失败诊断

**状态：** implemented，2026-09-27

## 问题

`article` 项目的一轮并行子任务运行截图和主线总结显示：五项工作最终都完成，但多项需要 retry 或 resume。结合当前代码可分出几类问题：

- steer 会在 child Turn 检查点结束当前 Turn，operation 随之记为失败；后续 Session Turn 虽然完成，读取结果仍只呈现旧 attempt。
- 普通 Turn 的八步上限适用于子任务，两个任务在 `step_limit` 处停止，需要人工重试。
- 主线程读取 operation 状态时看不到对应 attempt 的准确 Turn 终态和 `operation.error`，容易把任务失败、后续 Session Turn 完成和空进度报告混为一谈。
- 模型或运行时 Turn 失败时，operation 需要保留原始错误；仅保留 Session 持久化错误会丢失首次失败原因。

队列调度本轮没有复现产品缺陷：Gateway 的并发上限与排队排空回归通过。测试夹具补齐了当前启动流程读取父 Session `session_control` 的状态，并同步现有用户停止参数；生产调度逻辑未改。

## 决策

- Child operation Turn 接受 steer 时继续运行同一个 Turn。普通 Turn 仍使用原来的检查点结算策略。
- 子任务使用至少 16 步的 Turn 上限；高于 16 的运行时设置保留，零值仍代表不限制。活动 Goal 显式设置的里程碑步数预算继续优先。该边界不自动创建后续 Turn；步数耗尽的 attempt 明确失败，由主线程决定是否重试。
- worker 将 `step_limit`、实际步数、模型/运行时错误及持久化错误写入有界 operation 诊断。
- `task_read` / `task_list` 分别投影 operation 状态、该 attempt 的 `turn_outcome` 和子 Session 最新的 `latest_session_turn`。较新的 Session Turn 不会篡改先前 attempt 的终态。
- 复用现有 App Server Session operation 和 settled Turn；不新增 Gateway 状态账本，不改队列算法或 Core 单 Turn Loop。

## 变更准入

1. **所属层：**App Server 管理 child Turn 策略和 operation 错误；Capabilities 读取持久 Session 并投影 attempt 诊断；Web 文档与 Gateway 测试保持消费边界一致。
2. **已有职责：**复用 `SessionOperation`、`turn_settled`、`task_read` / `task_list` 及 Gateway 已有排队与启动恢复路径。
3. **替代内容：**child steer 不再靠结束旧 Turn 再开新 Turn；主线程不再仅凭最新 Session Turn 推断旧 attempt 结果。
4. **行数预算：**Core + Protocol 净增 0；Control Plane 净增 230；Release Rust 净增 360。均低于 6,000、35,000、50,000 行硬上限及每次变更 1,000 行增量门禁。
5. **协议与持久化：**增加有界 `turn_outcome` / `latest_session_turn` 读取字段，未改 operation 持久化格式或事件格式。child Turn 的有效最大步数策略变化；Goal 明确预算保持原值。
6. **边界证据：**App Server mock-model 场景验证同 Turn steer 与子任务步数；Capabilities fixture 验证 attempt 和最新 Session Turn 分离；Gateway 并发队列测试验证 4 项任务在 2 个槽位下排空及启动失败可见。无付费模型调用。

## 验证

- `cargo fmt --all --check`：通过。
- Capabilities Clippy 与测试：通过，135 项。
- App Server Clippy 与串行测试：通过，76 项。一次默认并行全包测试中，Goal fixture 报 `goal state not found`；定向重跑和完整串行包测试均通过。
- Gateway 子任务队列与启动恢复定向测试：6 项通过。
- `python3 scripts/line_budget.py`：Core + Protocol 4,768/6,000；Control Plane 32,003/35,000；Release 45,908/50,000。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：通过；Control Plane `+230`，Release `+360`。
- `git diff --check`：通过。

## 后果

步数上限仍然是硬边界；16 步只是普通 child Turn 的下限，不保证每项任务都能在一次 Turn 内完成。失败原因现在可供主线程读取，是否 retry 仍由主线程或用户明确决定。排队相关的本轮证据证明现有 Gateway 路径能处理并发释放和可见启动失败，没有单独的真实卡队列 Session 日志可用于诊断历史故障。
