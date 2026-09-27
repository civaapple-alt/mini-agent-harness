# 主子 Agent 自然交接与轻量复核

- status: implemented
- date: 2026-09-27
- implementation: 2026-09-27

## 决策

主子 Agent 沿用现有子任务工具和 Session，以自然语言完成交接。主 Agent 在委派提示中说明目标、预期交付和关键约束，不要求固定模板。子 Agent 只在有实质进展或遇到阻塞时用 `task_report` 汇报；最终回答简要交代结果、依据、未完成项和不确定处。

父任务在收到报告或终态唤醒后读取有界状态和当前 attempt 结果。运行中的任务尚无最终回答，不应被视为结果缺口。任务结算后，父 Agent 对照原始目标轻量复核：有明确缺口时，通过 `task_control.assign` 在同一个子 Session 追问；结果足够时，向用户总结已交付内容和仍存在的不确定性。

operation 状态只表达任务执行结果，不代表回答质量已验证。完整 child transcript 仍保留在子 Session；Gateway 唤醒只携带有界任务标识和操作指引，不复制报告正文或历史。Web Studio 沿用现有子任务列表、结果预览和子 Session 详情入口，不增加新的工作台或状态类型。

## 所有权与边界

- Capabilities 通过已有 `delegate_task`、`task_report`、`task_read` 和 `task_control` 工具说明引导父子 Agent 的交接行为；未增加参数或工具。
- Gateway 在已有父唤醒流程中提示主 Agent 对照委派目标复核、按需追问或总结；它不判断结果质量，也不保存任务状态。
- App Server operation 与 Session 仍是执行状态、attempt 身份和 child 历史的唯一权威。
- Web Studio 继续显示现有投影，不改变页面布局。

## 六项变更准入

1. **所属层：** Capabilities 负责模型可见的工具说明，Gateway 负责既有续行提示；App Server 持有任务状态。Core、SDK 和 Web Studio 状态逻辑不变。
2. **重复职责：** 复用现有委派、报告、读取、控制和自动唤醒路径，没有增加新 facade、任务账本或检查状态。
3. **替换旧概念：** 更新既有工具说明和 Gateway 唤醒措辞，保留 operation 生命周期、工具参数、父子 Session 和控制路由。
4. **行数预算：** 相对 `HEAD`，Core + Protocol 净增 0 行，Control Plane 净增 0 行，Release Rust 净增 31 行。当前分别为 `5,450/6,000`、`33,536/35,000`、`48,998/50,000`，行数门禁与 Release 增量门禁通过。
5. **可见面变化：** 工具描述和父唤醒 prompt 增加有界行为指引；未改变 tool 参数、事件、持久化格式、JSON-RPC 或 SDK 类型。
6. **边界证据：** Capabilities 单测覆盖工具说明；Gateway 单测和真实 App Server 跨仓 Smoke 覆盖唤醒 prompt、同一 child Session 的 follow-up、持久化 attempt 和恢复调度。

## 验证

- `cargo test -p mini-agent-capabilities`：140 项通过。
- `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`：通过。
- `cargo build -p mini-agent-app-server`：通过。
- Gateway 定向测试：1 项通过；使用上述本地 App Server 的跨仓 Smoke：2 项通过，无模型请求。
- Ruff check 和 format check 通过；两仓 `git diff --check` 通过。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json` 通过，Release 净增 31 行。

## 剩余限制

离线场景验证的是指引进入父 Session 的路径、状态边界和同一子 Session 的追问路由；它不证明某个真实 Provider 一定按指引行事。本次未调用付费模型，也未运行全工作区或完整 Web 测试矩阵。
