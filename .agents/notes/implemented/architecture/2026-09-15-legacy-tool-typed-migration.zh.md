# Legacy 工具结构化迁移

状态：implemented
日期：2026-09-15
批次：Iteration 10，完成 `ReadFile`、`ReadImage`、`WebFetch` 的 typed admission/runtime 迁移
范围：Capabilities 工具边界、Host admission 消费和 App Server 公共链回归验证

## Decision

三个内置工具不再依赖 `ToolHandler` 默认的 `Legacy` admission。工作区内和配置的
extension-root 文件读取返回 `Allowed`；工作区外已有文件返回带规范化目标信息的
`ApprovalRequired`。`ReadImage` 保留本地读取、工作区外审批和 magic-byte 校验；
`ReadFile` 的旧直接 `execute` 入口仍保留原有路径拒绝兼容行为，Host typed path
使用 `execute_after_admission`。

`WebFetch` 在 admission 阶段先完成 URL、DNS class 和 credential 校验。loopback
是显式允许的本地目标，公共 URL 进入 Host 审批边界；执行阶段用本地
`FetchError::{Failed, Retryable}` 映射为结构化结果，超时/传输失败可重试，URL
安全、大小、编码和重定向策略错误保持失败。没有恢复 Host 全局错误文本分类器。

为了让 WebFetch 的局部错误分类跨 blocking thread 保留，Capabilities 的 blocking
runner 只增加泛型错误类型约束；不增加第二条执行路径。

## Harness hypothesis

如果每个内置工具在 side effect 前都完成 bounded typed admission，并在 side effect
后返回 typed execution outcome，那么本地路径、工作区外路径、Plan Mode、超时和
外部网络错误就能在 Host/Core 主链中保持稳定语义；诊断文本的变化不会改变 loop
control 或重新触发授权。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批行为 | 禁止行为 |
| --- | --- | --- | --- |
| 参数和准入 | Capabilities `ToolHandler` | 解析路径/URL并返回 `Allowed` 或 `ApprovalRequired` | 在执行后才猜测是否需要批准 |
| 审批顺序 | Host `ToolOrchestrator` | 消费 typed admission，创建唯一审批请求 | 通过错误文本分类工具 |
| 副作用和局部结果 | Capabilities `ToolRuntime` | `ReadFile`/`ReadImage`/`WebFetch` 实现 `execute_after_admission` | 把外部错误交给 Host 全局分类器 |
| 公共结果 | Core/App Server | 保持既有 `ToolExecutionOutcome`/Item projection | 添加第二套 Legacy outcome 协议 |
| Web/Gateway | `mini-agent-web` 消费层 | 只消费和展示结果 | 自行决定 retry、approval 或路径授权 |

## Cross-repository contract

公共 wire shape 不变：`ThreadItem.status` 仍是生命周期，`ThreadItem.outcome` 仍是
工具结果。工具内部的 `Allowed`、`ApprovalRequired`、`Deferred` 和执行后的
`Completed`、`Failed`、`Retryable` 继续由 App Server 透传；Python SDK、Gateway
和 Web Studio 不复制授权权威，也不解析诊断文本。

## Verification

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo fmt --all` | passed |
| mini-codex | `cargo test -p mini-agent-capabilities --lib` | 79 passed |
| mini-codex | `cargo test -p mini-agent-host --lib` | 34 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 57 passed |
| mini-codex | Capabilities/Host/App Server `cargo clippy --all-targets -- -D warnings` | passed |
| mini-codex | `python scripts/line_budget.py --base 8e45501 --check-delta --json` | 无 violation；runtime `+0`、release `+166`、control-plane `+61`，当前 green |
| mini-codex | `git diff --check` | passed |

## Consequences

- 内置工具的 admission 责任集中在 Capabilities，Host 只编排，不再依赖 Legacy 默认分支。
- WebFetch 的外部网络瞬态错误可以表达为 `retryable`，安全拒绝不会被误报为可重试。
- 兼容性的 `execute` 入口和公共协议均保留；没有增加自动 retry、审批缓存或执行循环。

## Remaining risks

- 本批没有真实公共网络、真实 provider 或付费 API 调用；WebFetch resolver/redirect 的安全 fixture 属于下一批 Batch 11。
- `ToolError` 仍是兼容性的字符串承载类型，结构化语义只在本地 ToolRuntime/Host 边界产生。
- Batch 11 仍需补齐 Shell、SpawnAgent、MCP 的 Plan Mode 覆盖、context compaction 硬限制和 Harness Scenario/Eval 证据。
