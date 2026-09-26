# 子任务控制与 Turn 结算恢复对齐

**状态：** implemented，2026-09-26

## 问题

Gateway 启动时会按持久化的 `pausing` / `cancelling` 状态重放 `turn/interrupt`。如果子 Turn
已经结算，App Server 会返回 `thread has no active turn`。WebStudio 的 Session 投影与 App Server
的 `task_list` / `task_read` 都只用 `turn_settled` 覆盖 `queued`、`running` 或
`awaiting_approval`，没有覆盖仍保留控制意图的 operation。因此已结算的子任务可能继续显示为处理中，
恢复逻辑也会再次尝试中断。

启动日志中的 `kill: <pid>: No such process` 来自 Session 锁的过期 PID 探测。该 PID 已退出，锁回收
会按预期继续；子进程继承了 stderr，所以探测噪声被 Gateway 转成了 `Server STDERR` 警告。

## 决策

- WebStudio 与 Capabilities 都把匹配的 `turn_settled` 作为 `pausing` / `cancelling` operation
  的权威终态来源，并清除投影中的过期控制请求。已中断的暂停请求恢复为 `paused`；已中断的停止请求
  恢复为 `cancelled`；如果 Turn 在控制生效前已经完成或失败，则保留 Turn 的真实终态。
- 暂停终态会清除旧 `turn_id`，与 SessionStore 正常写入的 `paused` operation 一致。
- Session 锁 PID 探测隐藏 `kill -0` 的 stderr；过期 PID 仍按现有流程回收。
- 此规则只在 Session 有匹配的 `turn_settled` 记录时恢复终态。缺少该记录时，投影不会猜测任务成功或
  失败；需要单独处理“Turn 不活动且结算记录缺失”的恢复状态。

## 变更准入

1. **所属层：** Capabilities 的 Child operation 投影与 Web Gateway 的 Session 投影；状态恢复属于控制面，Core Turn Loop 不变。
2. **重复职责：** 复用 `reconcile_with_settled_turn` 和 `SessionCatalog` 的既有 settled-turn 投影，没有新增状态账本。
3. **旧概念：** 用 Turn 终态替代过期的处理中控制投影；不新增任务状态或协议操作。
4. **行数预算：** Core + Protocol `4,767 -> 4,767`（`0`）；Control Plane `30,904 -> 30,906`（`+2`）；Release Rust `44,466 -> 44,548`（`+82`）。
5. **可见面：** 不增加模型输入、事件、持久化记录或公共协议字段；只将现有持久化 Turn 终态投影到已有任务状态。
6. **边界测试：** Capabilities 的 `task_read` 投影测试、Gateway 的 SessionCatalog 投影测试覆盖暂停/停止和竞态结算；没有变更 prompt、tool schema、loop-control 或事件形状，不需要额外 Scenario/Eval。

## 验证

- `cargo test -p mini-agent-capabilities`：130 passed。
- `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`：passed。
- `cargo test -p mini-agent-app-server -- --test-threads=1`：72 passed。
- `cargo fmt --all --check`、`git diff --check`：passed。
- Web `uv run pytest tests/gateway/test_session_manager.py`：133 passed；`ruff check` 与 `ruff format --check`：passed。
- `python3 scripts/line_budget.py`：passed；Core + Protocol 4,767/6,000，Control Plane 30,906/35,000，Release 44,548/50,000。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：passed；Release `+82`、Control Plane `+2`。
