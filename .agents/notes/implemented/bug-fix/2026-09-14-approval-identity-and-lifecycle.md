# 审批身份与生命周期收敛

Status: implemented  
Date: 2026-09-14  
Scope: App Server `ApprovalBroker`、Web Gateway `ApprovalBridge`、Web Studio 审批投影

## Decision

审批请求不再把 Provider 的 `requestId` 当作全局唯一键。

- App Server 生成带进程、Broker、局部序号和 `call_id` 的 request ID；原始
  `call_id`、Project、Thread、Turn 继续作为结构化身份随请求和结果传递。
- Gateway 用内部 entry key 保存每个等待中的 Future。首个 request ID 保持兼容，
  重复 request ID 使用内部唯一后缀；响应必须按 `requestId + call_id +` 可用的
  Project/Thread/Turn 范围唯一匹配。重复 request ID 且没有足够身份的响应 fail closed。
- Web Studio 将待审批项维护为有序队列，一次展示一项，标题显示剩余数量；审批结果
  按相同身份写回对应工具卡片。停止、运行时断开、重启和跨窗口处理都会使待审批项
  进入明确的过期/已处理状态，不能让迟到审批重新执行工具。
- `approval-evidence.jsonl` 仍然是有界审计旁路，只记录工具类型、`call_id`、
  `session_item_id`、策略、范围和哈希。完整命令通过 `session_item_id == call_id`
  关联同一 Thread 的 Session 日志；不把完整 arguments 复制进 trace。

这保持了薄 Agent Loop、厚 Control Plane 的边界：Host/Capabilities 仍是授权和
执行权威，App Server 负责审批等待与事件身份，Gateway 负责跨窗口转发，Studio 只
维护展示队列和用户操作状态。

## Change admission

1. **所属层**：主要属于 App Server 的审批事件身份和 Gateway 的 Control Plane
   转发；Capabilities 的授权结论没有移动，Web Studio 只消费并展示这些边界。
2. **重复职责**：复用了现有 `ApprovalBroker` responder、Gateway
   `ApprovalBridge` pending Future、Session 的 `call_id` 和 WebSocket/REST 审批通道；
   没有新增授权存储、第二套 Session 历史或前端授权权威。
3. **旧概念**：替换了“request ID 直接作为 Future 字典键”的假设，以及前端单个
   `pendingApproval` 的覆盖式投影。保留 request ID 作为兼容的传输/展示字段。
4. **行数预算**：以当前工作树 `HEAD` 为 base，实际为：

   ```text
   runtime:       19,637 -> 19,703 (+66)
   all Rust:      29,272 -> 29,338 (+66)
   control-plane: 20,266 -> 20,332 (+66)
   ```

   运行时、release Rust 和 Control Plane 均处于 green；没有 Cargo manifest 变化。
5. **可见面变化**：Gateway REST/WS 审批响应增加可选 `call_id`，用于重复 request ID
   的消歧；App Server 公共响应形状不变。Studio 增加多个待审批项的队列计数和审批
   结果投影。trace 仍有界且不增加完整命令、凭证或完整参数的模型/持久化输入。
6. **边界测试**：新增并运行了跨 Broker request ID 唯一性、Gateway 重复 request ID
   按 call identity 消歧、审批停止/运行时 EOF、Studio 工具卡片匹配和 API payload
   回归测试。

## Verification

Rust：

- `cargo fmt --all --check`
- `cargo clippy -p mini-agent-app-server --all-targets -- -D warnings`
- `cargo test -p mini-agent-app-server`（56 passed）
- `python scripts/line_budget.py`（runtime 19,703；release 29,338；均 green）
- `python scripts/line_budget.py --base HEAD --check-delta --json`（+66/+66，
  violations 为空）

Web：

- `uv run ruff check .`
- `uv run ruff format --check .`
- Gateway 审批/会话回归：56 passed
- `npm test`（Node 51 passed；Vitest 24 passed）
- `npm run lint`
- `npm run build`

## Consequences

同一 Turn 返回多个需要审批的工具时，用户可以逐项处理，且不会因 Provider 重用
request ID 而批准错误的工具。停止在审批等待期间会先失效所有当前 Turn 的审批，
再等待权威终态；跨浏览器处理会关闭其他窗口的 stale dock 并保留消息流证据。

未结算的 Turn 可能尚未把工具 item 写入 Session，因此对应 approval trace 暂时没有
可关联的完整命令；这是有意的 bounded evidence 边界，不应通过复制命令到 trace
来掩盖未提交状态。
