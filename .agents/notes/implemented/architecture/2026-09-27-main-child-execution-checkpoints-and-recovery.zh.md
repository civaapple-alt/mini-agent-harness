# Main 与 Child 的会话检查点和执行恢复

**状态：** implemented，2026-09-27

## 决策

Session checkpoint 与 execution checkpoint 各自保存不同的状态。Session checkpoint 在 Turn 结算后保存对话上下文，供后续对话与 fork 使用。Execution checkpoint 保存一个逻辑 Turn 的输入、模型上下文、下一模型步和阶段，供同一 Turn 接续。

Core 在每次模型请求前，以及整批工具完成后写入 execution checkpoint。工具批次 journal 记录批次意图、单个调用的开始和结果。已持久化的结果可供恢复复用。调用已开始但没有结果时，SessionStore 将 Turn 投影为 `needs_reconciliation`，不自动重复可能产生副作用的调用。

App Server 重启时不会自动续跑。无未决工具调用的活动 Turn 进入 `waiting_for_continue`，用户通过 `turn/resume` 显式从最新 checkpoint 接续原 Turn。恢复请求使用 Turn ID、checkpoint 序号和稳定 request ID；它不增加 Turn 或 Child operation attempt。

App Server 每 10 秒写入 executor heartbeat，并在状态读取中返回阶段、最后心跳、最后进展时间、checkpoint 序号和原因。Responses provider 在 120 秒无数据时判为采样停滞。对可识别的暂时传输与服务端错误，Responses provider 在最多 5 次或 120 秒的总窗口内退避重试；停滞流不自动重试。

Main 和 Child 各自在所属 SessionStore 中写入 journal。Gateway 与 Web Studio 读取 App Server 恢复状态；刷新和重连只恢复展示，不启动执行。Child 继续时沿用原 Session、Turn、operation 和 attempt。待核对任务保留核对入口。

## 变更准入

1. **所属层：**Core 发出有界执行边界记录；Capabilities 将 checkpoint 和工具 journal 追加到现有 SessionStore；App Server 管理恢复请求与状态；SDK、Gateway 和 Web Studio 负责调用及呈现。
2. **已有职责：**复用 Core 单 Turn Loop、App Server SessionStore、Child operation、Gateway 控制路径和 WebStudio 面板。
3. **替代内容：**原来只有 settled Session checkpoint 可用于恢复。现在 execution checkpoint 支持在同一逻辑 Turn 内显式接续；未知副作用不再盲目重放。
4. **行数预算：**Core + Protocol 实际增加 550 行，仍低于 6,000 行上限；Control Plane 为 33,344/35,000，Release 为 48,003/50,000，均通过仓库行数门禁。
5. **协议与持久化：**SessionStore 追加有界 execution journal 记录；增加 `turn/resume` 和 `turn/read.recovery`。恢复快照不通过读取接口暴露完整内部模型上下文。
6. **边界证据：**Core 测试验证使用已记录的工具结果继续且不重复调用；App Server 测试验证重开 Session 后以相同 Turn ID 接续；SDK 测试验证 `in_progress` 状态继续轮询。Web 定向恢复测试覆盖 Main 与 Child 的显式恢复路径。

## 验证

- `cargo fmt --all --check`：通过。
- 受影响 Rust 包 Clippy：通过，`mini-agent-core`、`mini-agent-capabilities`、`mini-agent-app-server-protocol` 和 `mini-agent-app-server`。
- 受影响 Rust 包测试：串行通过。App Server 77 项、协议 20 项、Capabilities 136 项、Core 46 项，共 279 项。
- `python3 scripts/line_budget.py`：通过。Core + Protocol 5,450/6,000；Control Plane 33,344/35,000；Release 48,003/50,000。
- Web 恢复路径 pytest：5 项通过。前端定向测试：80 项 Node 测试和 37 项 Vitest 测试通过。
- Web ESLint、Ruff check、Ruff format check 和 Vite production build：通过。Build 保留 Vite 的 500 KiB chunk 提示。
- `git diff --check`：通过。未调用付费模型。

## 后果

模型输出静默、服务重启或执行中断后，用户可以区分可直接继续的状态和必须先核对工具结果的状态。恢复边界位于最近一个完整模型上下文或完整工具批次之后。工具调用执行到一半、结果没有写入 journal 时，系统无法证明外部副作用是否发生，因此不会自动重放。检查点仍待恢复或核对时，App Server 会拒绝新的 `turn/start`，防止新 Turn 覆盖这份进度。
