# Harness 边界与变更准入

Status: current boundary policy

## 责任分工

```text
Core ToolRouter
  → 按名称解析已注册工具
Protocol ToolHandler
  → 解析参数、描述 admission、产生协议级输入/结果
Host ToolOrchestrator
  → 编排 admission、approval、执行，并透传 typed outcome
ToolRuntime
  → 持有具体副作用及其 workspace/sandbox 配置
```

Core 只拥有可移植的 model/tool contract、显式 run loop、limits、stop
classification 和 observation events。Provider、文件、进程、approval UI、
persistence 和 terminal output 留在 Core 外。被动 observer 不改变执行。

## Thin Loop、厚 Control Plane

“薄 Agent Loop、厚 Control Plane”是责任划分，不是让某一层无限增长。行数预算只
是早期预警和交付门禁，还必须用结构性证据确认责任没有跨层泄漏。

当前 crate 对应关系如下：

| 概念 | 实际 crate / 统计分类 | 责任 |
| --- | --- | --- |
| Thin Loop / Execution Kernel | `mini-agent-core`、`mini-agent-protocol`；`execution-kernel` | portable model/tool contract、显式 turn loop、context/limit、stop classification、observation event 和公共协议 |
| Control Plane | `mini-agent-host`、`mini-agent-app-server`、`mini-agent-app-server-protocol`，加上 `mini-agent-capabilities` 中明确列出的 control-plane 文件；`host-control-plane + capability-control-plane` | admission、approval、sandbox/tool orchestration、Session/operation 生命周期、并发/恢复和对外 runtime projection |
| gateway-control | `mini-agent-app-server`、`mini-agent-app-server-protocol`；当前作为 `host-control-plane` 的子集统计 | JSON-RPC、Thread/Turn 投影、runtime/session 控制和 Gateway 边界；不拥有 Core history 或独立授权真相源 |
| Release Rust source | `mini-agent-core`、`mini-agent-protocol`、`mini-agent-capabilities`、`mini-agent-host`、`mini-agent-app-server`、`mini-agent-app-server-protocol` | 发布包完整 Rust 源码与测试；不含实验性 `mini-agent-cli` |

`mini-agent-capabilities` 中未列入 control-plane 的 provider 实现仍计入
`capability-provider`，并计入 Release Rust source。`gateway-control` 当前没有单独的
硬门禁，避免把同一批 App Server 文件重复计数；如果未来要区分 transport-edge，必须
先按文件责任拆出互斥分类，再新增统计或门禁。

结构性验收至少包括：

- Core 不新增 Scheduler、ApprovalStore、Provider、持久化或 Gateway task map；Child
  Session 由 Host/App Server 通过独立 runtime 表达。
- Host/Capabilities 是 admission、approval、执行副作用、路径边界和并发限制的权威；
  Gateway 只做协议转换、Session/runtime 投影和事件转发。
- `Protocol → Core → Capabilities → Host → App Server → CLI` 的依赖方向保持为有向无环图，
  并通过 `cargo_boundary.py` 检查。
- 涉及 Child、operation、notebook、approval、replay 或恢复的变更，必须有对应的
  bounded scenario/test evidence；通过行数门禁不能替代这些证据。
- 涉及 prompt、tool schema、loop-control、context、event 或 persistence 的变更，必须
  补充 Harness Scenario/Eval，证明模型可见面和生命周期行为没有退化。

`python scripts/line_budget.py` 默认只输出三个关键预算；`--verbose` 查看 crate、layer
和 production/test 拆分，`--json` 提供 CI 和审计所需的完整报告。

工具结果沿着主执行链保持结构化状态：Core 解析并调用 ToolRouter，Host 根据
`ToolAdmission` 编排准入与审批，Capabilities 的 `ToolRuntime` 返回带有
`ToolExecutionStatus` 的 `ToolExecutionOutcome`，最后由 Core 写入
`ToolFinished`、history 和下一轮模型输入，再由 App Server 投影为事件与 Item。
Host 不通过 `content` 的错误文本猜测 `NeedsApproval`、`Deferred` 或 `Retryable`；
`ToolError(String)` 仅是旧工具兼容边界，Legacy 工具必须自行返回明确的 outcome。
因此 `content` 是有界诊断信息，不是跨层状态协议。

Child Session 也留在这个边界之外：Core 只运行当前 Thread 的一个显式
Turn，Host/App Server 通过独立 Session/runtime 表达结构并发。创建 exact
child 时，Host 只能从父 Session 读取最近一次完整持久化 checkpoint，不得
复制或修改父 Core 的可变上下文；child 重新经过自己的 Host admission、
Approval、ToolRuntime 和 event replay。这里不引入 Core 内多租户调度器，也不
把 Gateway 的 client/task map 当成历史或授权的第二权威。

Child operation lifecycle and the Session notebook follow the same rule. The
SessionStore owns durable operation records and notebook entries; Host owns
their tool admission and App Server owns runtime/control projection. Core may
carry bounded operation correlation on a Turn, but it does not create child
Sessions, schedule work, persist notebook state, or interpret Gateway task maps.

App Server 的 `ThreadItem.ToolCall` 保留两个正交字段：`status` 表示 Item
生命周期，`outcome` 表示 Core 工具结果。`needs_approval`、`deferred` 和
`retryable` 不能被折叠成一个 `completed/failed` 布尔判断；旧 Session 没有
`outcome` 时仍按兼容规则读取。

稳定内置 prompt body 属于 crate-owned `builtin/prompts` Markdown asset 并在
编译期嵌入；Host 的 project、extension、world、workflow instruction 只能在
有界 runtime composition 中加入。App Server 只能选择 allowlisted startup
runtime 组合，不能通过公共协议暴露任意 raw system-prompt replacement。

## 六项变更准入

每个 feature、refactor、test change 或 protocol change 在实现前回答：

```text
1. Layer
   明确属于 Core、Host、Capabilities、App Server 还是 CLI，并说明为什么。
2. Duplicate responsibility
   检查是否已有 path/type 拥有同一责任；不得用新 facade 遮盖旧 owner。
3. Replace vs add
   优先移除或替换旧概念；若新增，说明为什么不能放入 host adapter。
4. Net line delta
   记录 runtime 与 release-source 有效代码行的预期和实际 delta；默认
   net-zero，或给出明确 offset。运行 `python scripts/line_budget.py`。
5. Visible surface
   记录对 model-visible input、tool schema、event、persistence 和 public
   protocol 的影响与 hard limit。
6. Boundary evidence
   指出可覆盖的既有公共边界测试和缺失的 Harness Scenario/Eval evidence。
```

PR 还必须在 `.github/pull_request_template.md` 填完六项答案并勾选六项 admission
box。机械检查只验证填写完整，架构判断仍由 review 完成。

## 已确认的边界决策

### 架构不变量与反妥协底线 (Architectural Invariants)

任何重构与实现严禁为追求局部便利、降低改动成本或规避行数红线而牺牲以下底线：

1. **正交概念严禁合并压扁**：
   全局策略（`ApprovalPolicy`：交互把关 vs 自动副驾）与单次授权生命周期（`ActionGrantScope`：本次 vs 会话 vs 项目）是正交维度，必须独立建模，严禁压扁为一个枚举。
2. **单一真理源严禁双写**：
   执行与授权权威唯一归属 Host/Capabilities 的 `ApprovalStore`；网关、SDK 与前端只维护 UI/Pending 生命周期，严禁自建影子授权缓存。
3. **安全身份结构化，严禁展示文本匹配**：
   授权比对必须使用结构化 `ActionGrantKey`（类目、归一化命令、排序去重目标路径、访问范围、版本号），严禁依赖人类可读的字符串摘要或未归一化路径。
4. **端到端契约完整无损**：
   跨层 RPC 协议必须完整透传结构化意图（`ToolApprovalResolution`），严禁降维为布尔值或静默丢失 `grant_scope`。
5. **分级安全兜底**：
   `Automatic` 模式仅自动推进明确的低风险操作；`Trusted` 模式可自动推进经过工具自身校验的普通工作区操作，但递归/强制删除、破坏性 Git、系统级命令、MCP 和工作区外 `read_image` 仍保留显式审批；高危操作必须有安全兜底，严禁盲目静默放行。
6. **行数超标必须通过重构剪枝解决**：
   触及行数上限时，唯一正确路径是删除废弃概念、清理历史死代码和冗余测试分支，严禁通过“打补丁、降低安全标准、扭曲架构”来绕过硬规则。

### Loop control

Core 在安全检查点先检查 cancel，再检查 steer。deadline 触发后，App Server
发送 interrupt，等待 `TurnFinished` 和 durable checkpoint，再返回 timeout，
不继续 drain 竞争中的 steer；普通已 settle batch 才按 steer 优先于 follow-up。
恢复 settled history 或开始下一轮时，会清理旧版本可能留下的未完成
assistant/tool group，避免把不可重放的工具调用再次交给 provider。
修改这个顺序前必须增加 deterministic race scenario。

### Approval 与 sandbox denial

拒绝必须映射成非空结构化结果，并且同时在 event、Session checkpoint 和下一轮
模型输入中可见。当前沿用 `ToolExecutionStatus::NeedsApproval`，暂不为了命名
增加公共 `PermissionDenied` 变体；下一次 approval/sandbox 改动不得退化为空结果。

### Trace

Trace 复用已有 observation events 和 `JsonlTrace`，只记录有界 metadata、counts
和 hashes，不写 raw prompt、tool arguments/results 或 Session history。CLI 的
`run --trace-jsonl PATH` 是显式、caller-owned、create-new artifact；不恢复退役
的外部 `--trace`，不隐式写入 Session。

Web Studio 的审批证据使用独立的 `approval-evidence.jsonl` sidecar，每个 Thread
单独写入自己的 Session 目录。它记录已完成的审批决策、策略/访问范围、工具与命令
首词摘要、`call_id`/`session_item_id` 关联键、规范化 action key 的 hash 和结果；请求
进入等待态时先写 `approval_requested`，最终决策再写 `approval_resolved`。它不是
Session history，也不是 `ApprovalStore`。完整命令不复制到 sidecar；读取方通过同目录
`session.jsonl` 中 `kind=item` 且 `item_id=session_item_id` 的 bounded/redacted
arguments 关联命令。多个 Thread 的项目级报告必须在读取时聚合，不得让 Gateway 或多个
runtime 共同写一个项目级文件。原始 prompt、完整 tool 参数、工具输出和密钥不得进入
该 sidecar。Trace 只能支持后续规则评审，不能自动授予新的权限。

### Compaction

当前确定性触发点仍是最大上下文的 50%，不是把 70% 预警写成新的运行时语义。
后续 scenario 记录 70% 预警、最近轮次保留和压缩前后预算；只有证据证明 50%
不合适时才改阈值。

Compaction 的辅助 user prompt 也受 `max_user_input_bytes` 限制，并使用 UTF-8
安全截断；prefix、摘要、最近 tail 和 system/tool 请求最终都必须通过同一个
`context_bytes_for` 字节上限。小配置值不能因为稳定的 compaction prompt 或多字节
字符而绕过 user/context limit。

### Plan Mode 与外部网络

Plan Mode 的 mutation admission 保持按工具分层：ApplyPatch 返回 `Deferred`，Shell
对变更命令返回 `ApprovalRequired`，MCP tool call 返回 `Deferred`；只读 Shell、
工作区/extension-root 读取和显式允许的 loopback `web_fetch` 仍可执行。当前工具
目录没有 `SpawnAgent` 公共路径，因此不以不存在的能力声称覆盖；若未来加入，必须
先定义其 child-session、路径和审批 owner，再补同一组边界证据。

`web_fetch` 在 DNS 解析后固定 origin endpoint，并要求所有解析地址与已准入的
public/loopback class 一致；redirect 只能留在同一 host 和同一 class。公共 URL
进入 Host approval，loopback 是显式本地允许目标；resolver 不能把公共域名转成
loopback、私网或 cloud metadata 地址。

### Docker

当前 Docker evidence 只证明 daemon 可达、workspace mount 和容器临时文件探针，
不等同于完整安全隔离。要增加 network、capability、privilege、read-only 或
resource restriction，必须先明确 threat model、支持平台、默认值、兼容性和
fail-closed 行为，再增加政策和跨平台边界测试。

### Provider 与 retry

单一 provider 的行为不能自动产生 provider-specific 分支。HTTP 429 保持 bounded
fail-fast；只有第二个真实 provider 或明确的 bounded retry policy 出现后，才建立
provider matrix 和 retry/backoff，且不调用付费 provider、不另起执行循环。

### Cargo dependency direction

当前 workspace 依赖保持单向 DAG：`Protocol → Core → Capabilities → Host → App
Server → CLI`，`App Server Protocol` 只依赖 `Protocol`。`App Server → Capabilities`
是已知的 review edge：App Server runtime assembly 需要 provider/session seam，但
Host 仍拥有 tool、policy、world 和 workflow composition；line gate 的路径统计不改变
Cargo 所有权。只有发现重复 authority、反向/循环依赖，或可删除而非新增胶水层的稳定
boundary seam 时，才另立 Cargo 重构提案。依赖方向由
`python scripts/cargo_boundary.py --json` 检查。

## 自动化顺序与非目标

先固定两个 line ceiling 和六项准入，再完善 bounded scenario/eval；只有存在
有界故障注入 seam 才推进跨层 MCP timeout projection；之后验证 compaction retention，
最后再处理明确政策下的 Docker 隔离和 provider matrix。

不追求 VS Code 的工具数量、扩展生态、模型数量或 UI parity；不提前把 stop hook、
plugin、scheduler、memory 或 policy framework 加入 Core；不为 benchmark 分数放宽
hard limits、approval、Session 单一权威或公共协议兼容。

## 维护规则

本文件维护当前准入和边界政策。新的边界实验先作为 `.agents/notes/` 下的日期
变更记录，确认落地后直接更新本文件；不再建立专题索引或 archive 链接。
