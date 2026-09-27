# SDK 恢复类型与跨层契约证据

**状态：** implemented，2026-09-27

## 决策

Python SDK 将 `ThreadCheckpoint.execution_recovery` 和 `TurnReadResult.recovery` 解析为
`ExecutionRecoveryInfo`。该类型提供状态、阶段、Turn ID、检查点序号、心跳、进度时间
和原因。未知状态映射为 `UNKNOWN`，完整原始对象保留在 `raw` 中。

`recommended_action` 根据 App Server 状态给调用方一个提示。它不调用 `turn/resume`，
也不重试工具。`needs_reconciliation` 提示调用方先核对工具的外部结果；未知状态提示
调用方检查。App Server 仍是 Session、Turn 和执行恢复状态的唯一来源。

Gateway 将 SDK 类型映射回现有 JSON 字段，不增加 HTTP 或 App Server 协议字段。事件
游标出现缺口时，SDK 暴露 `has_gap`；调用方可以读取 Thread checkpoint 与 bounded
ThreadItems 来校准历史。旧 SDK 收到未知事件时仍通过 `GenericEvent` 保留事件类型和
内容。

## 改动准入

1. **所属层：** SDK 与 Gateway 在 Web 仓库；execution checkpoint 和恢复状态由 App Server
   管理。Harness 只补 App Server JSON-RPC 的重启边界测试。
2. **已有职责：** 扩展现有 SDK 恢复投影、Gateway HTTP 映射、`turn/events` 回放和
   `GenericEvent`，不新增恢复存储或转发层。
3. **替代内容：** 用一个 SDK 数据类替换恢复字典。Gateway 输出仍采用原 JSON 对象。
4. **行数预算：** `line_budget.py --base HEAD^ --check-delta --json` 报告 Core + Protocol
   净增 0 行，Control Plane 净增 21 行，Release 净增 21 行。三项均低于硬上限和单 PR
   增量额度。
5. **协议与持久化：** App Server protocol v1、JSON 字段、事件和持久化均未改变。Python
   SDK 的恢复属性从字典改为公开数据类；SDK README、指南和 Unreleased changelog 记录了
   访问方式变化。
6. **边界证据：** App Server 重启测试检查 Thread 恢复字段并继续原 Turn。SDK 测试覆盖
   已知和未知恢复状态、事件缺口与未知事件。Gateway 测试覆盖 HTTP 字段映射、WebSocket
   断开和按权威 Session ID 重新附加。场景不调用 Provider。

## 验证

- `cargo fmt --all`：通过。
- `cargo clippy -p mini-agent-app-server --all-targets -- -D warnings`：通过。
- `cargo test -p mini-agent-app-server -- --test-threads=1`：通过，77 个库测试和 1 个二进制测试。
- App Server 持久 execution checkpoint 重启测试：通过。
- Python SDK 离线测试：59 项通过。一个需要本机 App Server 的实时 SDK 测试未纳入该次运行。
- Gateway 恢复契约定向测试：5 项通过。
- 修改文件的 Ruff 检查、格式检查与 `git diff --check`：通过。
- `python3 scripts/line_budget.py --base HEAD^ --check-delta --json`：通过。
- 未调用付费 Provider。

Gateway 的更大测试组仍有与本改动无关的旧 mock 签名和异步 mock 失败；本次以恢复契约
定向测试作为 Gateway 证据。

## 后果

SDK 调用方从 `recovery["status"]` 改为 `recovery.status.value`，并可读取其他有类型字段；
需要原始字段或新字段的调用方使用 `recovery.raw`。Gateway 输出字段保持原样。

恢复建议只表达检查方向。外部工具副作用是否发生，仍需调用方核实；SDK 与 Gateway
不会据建议自动继续执行。
