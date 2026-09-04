# Harness 边界与变更准入

Status: current boundary policy

## 责任分工

```text
Core ToolRouter
  → 按名称解析已注册工具
Protocol ToolHandler
  → 解析参数、描述 admission、产生协议级输入/结果
Host ToolOrchestrator
  → 编排 admission、approval、执行和结果投影
ToolRuntime
  → 持有具体副作用及其 workspace/sandbox 配置
```

Core 只拥有可移植的 model/tool contract、显式 run loop、limits、stop
classification 和 observation events。Provider、文件、进程、approval UI、
persistence 和 terminal output 留在 Core 外。被动 observer 不改变执行。

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
   记录 runtime 与 release-source 的预期和实际 delta；默认 net-zero，或给出
   明确 offset。运行 `python scripts/line_budget.py`。
5. Visible surface
   记录对 model-visible input、tool schema、event、persistence 和 public
   protocol 的影响与 hard limit。
6. Boundary evidence
   指出可覆盖的既有公共边界测试和缺失的 Harness Scenario/Eval evidence。
```

PR 还必须在 `.github/pull_request_template.md` 填完六项答案并勾选六项 admission
box。机械检查只验证填写完整，架构判断仍由 review 完成。

## 已确认的边界决策

### Loop control

Core 在安全检查点先检查 cancel，再检查 steer。deadline 触发后，App Server
发送 interrupt，等待 `TurnFinished` 和 durable checkpoint，再返回 timeout，
不继续 drain 竞争中的 steer；普通已 settle batch 才按 steer 优先于 follow-up。
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

### Compaction

当前确定性触发点仍是最大上下文的 50%，不是把 70% 预警写成新的运行时语义。
后续 scenario 记录 70% 预警、最近轮次保留和压缩前后预算；只有证据证明 50%
不合适时才改阈值。

### Docker

当前 Docker evidence 只证明 daemon 可达、workspace mount 和容器临时文件探针，
不等同于完整安全隔离。要增加 network、capability、privilege、read-only 或
resource restriction，必须先明确 threat model、支持平台、默认值、兼容性和
fail-closed 行为，再增加政策和跨平台边界测试。

### Provider 与 retry

单一 provider 的行为不能自动产生 provider-specific 分支。HTTP 429 保持 bounded
fail-fast；只有第二个真实 provider 或明确的 bounded retry policy 出现后，才建立
provider matrix 和 retry/backoff，且不调用付费 provider、不另起执行循环。

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
