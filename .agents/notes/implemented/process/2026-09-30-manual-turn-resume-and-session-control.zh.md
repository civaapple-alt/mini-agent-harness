# Web Studio 手动 Turn 续接与恢复结算

- status: implemented
- date: 2026-09-30

## 问题与复现证据

用户发现手动推进时“继续当前 Turn”没有可见进展，切换为自动推进后可以继续。默认 Harness 的 `max_steps` 为 8；执行检查点保存的是下一模型步号，恢复时从该步号继续，但 App Server 沿用同一个绝对步数上限。因此第 8 步触发 `StepLimit` 后，恢复会在下一步再次立即触发 `StepLimit`。自动推进设置 `max_steps = 0`，不受该有限上限影响。

恢复父 Session 时，Gateway 在 `turn/resume` 已接受并启动恢复 Turn 后又调用 `session/control resume_settled`。App Server 的运行中 mutation admission 不允许这项状态写入，于是 Gateway 记录 `thread already has an active turn` 并把 Session 留在恢复中。

## 决策

- 只有当显式续接的检查点下一步已越过当前有限 `max_steps` 时，App Server 才将该续接 Turn 的上限向前延展一个同样大小的步数片段；`max_steps = 0` 仍表示不设步数上限。续接仍使用原 Turn、相同检查点和已有执行历史。每次后续显式续接可获得一个新的有限片段。
- 把这次配置写入 Harness 当前执行，并在 Turn 结束后沿用原配置恢复，避免只计算但未应用新上限。
- 允许活动 Turn 期间执行幂等的 `resume_settled`。该操作收敛持久 Session 控制状态并恢复既有的 Session continuation gate；重复请求不会重放 Turn 或授予工具/动作权限。
- 不新增协议字段、持久化状态或 Gateway 私有恢复账本；App Server 继续负责 Session 与 Turn 的权威状态。

## 变更准入

1. **所属层：** App Server worker 持有恢复时的步数配置；App Server Runtime Actor 负责运行中 Session 状态变更 admission。
2. **重复职责：** 复用既有 `max_steps`、执行检查点、`turn/resume` 和 `resume_settled`；不增加第二种恢复协议。
3. **旧概念：** 保留每段有界步数语义，通过已有显式续接入口提供下一段额度；无需把整个 Turn 改为无限运行。
4. **行数预算：** Core + Protocol 净增 0；Control Plane 净增 169 行；Release Rust 净增 169 行，均包含测试。当前预算为 `6101/6500`、`36326/38000`、`53312/55000`。
5. **可见面：** 修复影响 loop-control 和持久恢复路径；不更改模型可见输入、事件、协议形状或检查点格式。
6. **边界证据：** App Server JSON-RPC Mock Model Scenario 覆盖手动 StepLimit 到同一 Turn 完成；另一条公开 RPC 场景覆盖活动 Turn 中重复恢复结算。测试不调用 Provider。

## Bounded Scenario

### 手动 StepLimit 后显式续接

- **Hypothesis：** 命中有限步数上限后，用户显式续接可继续同一 Turn 一个有限步数片段；不能立即重复命中旧上限。
- **Public path：** App Server `turn/start`、`thread/read`、`turn/resume`、`turn/read`。
- **Setup：** 临时 Session、默认手动配置和确定性 Mock Model；前 8 个请求返回已知工具错误，第 9 个请求返回完成文本。
- **Stimulus：** 运行至 `step_limit`/`waiting_for_continue`，用当前 Turn ID 和检查点序号调用 `turn/resume`。
- **Trace：** 首轮恰好 8 次模型请求并落在待续接检查点；接受恢复后，同一个 Turn 再进行第 9 次模型请求并完成。
- **Settlement：** `turn/read` 返回 `completed` 和恢复后的完成文本，模型调用数为 9。
- **Failure case：** 对过期或不匹配检查点的恢复仍由既有检查点校验拒绝，不扩展步数上限；本 Scenario 不新增该拒绝分支的独立用例。
- **Command：** `cargo test -p mini-agent-app-server turn_resume_after_step_limit_gets_another_bounded_step_slice -- --nocapture`
- **Gap：** 场景使用默认 8 步配置与 Mock Model，不覆盖付费 Provider 的行为或质量。

### 活动 Turn 中恢复状态结算

- **Hypothesis：** Gateway 在恢复 Turn 启动后重复结算同一恢复请求时，App Server 应返回持久的 `running` 状态，不应以 `Busy` 拒绝。
- **Public path：** App Server `session/control` 与来源为 `session_resume` 的 `turn/start`。
- **Setup：** 临时 Session 先进入 `frozen`，随后转为 `resuming`；Mock Model 将恢复 Turn 保持活动。
- **Stimulus：** 恢复 Turn 已发出 `TurnStarted` 后，使用同一 `requestId` 重复调用 `resume_settled`。
- **Trace：** 首次恢复请求启动 Turn 并收敛状态；活动期间的重复结算仍成功且返回 `running`，然后 Mock Model 正常完成。
- **Command：** `cargo test -p mini-agent-app-server resume_settlement_is_idempotent_while_the_resumed_turn_is_active -- --nocapture`
- **Gap：** Gateway HTTP/WebSocket 前端交互未由该 Rust 场景模拟；此错误路径由 Gateway 日志和 App Server RPC 场景共同定位。

## 验证

- `cargo fmt --all`：通过。
- `cargo test -p mini-agent-app-server`：85 个库测试、1 个二进制测试通过。
- `cargo clippy -p mini-agent-app-server --all-targets -- -D warnings`：通过。
- `python3 scripts/line_budget.py`：PASS；Core + Protocol `6101/6500`，Control Plane `36326/38000`，Release `53312/55000`。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：Core + Protocol `+0`，Control Plane `+169`，Release `+169`；无 violation。
- `git diff --check`：通过。
