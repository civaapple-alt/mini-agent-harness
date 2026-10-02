# Session V2 恢复、来源清单与执行边界

状态：partially implemented（恢复、Context Manifest 和事件回放已实现并验证；Docker 运行验证待补）

日期：2026-10-03

范围：`mini-agent-harness` 的 Capabilities、App Server、Protocol 与文档；`mini-agent-web` 的 Python SDK、Gateway、Web Studio 与文档。

## 结论

重启不能自动重放工具副作用。Session checkpoint 用于新 Turn 和 fork；Execution checkpoint 用于操作者显式恢复原 Turn。结果未知的工具调用先由操作者核对，App Server 持久化核对结果并阻止过期或重复处置。

SessionStore 继续持有 Thread、Turn、Session journal 和恢复状态。App Server 提供核对和恢复 RPC。SDK、Gateway 与 Studio 传递并呈现该状态，不各自保存恢复账本。Session 另存有界 Context Manifest 与事件摘要，记录来源和生命周期引用，不复制模型正文或工具输出。

本次实现覆盖了恢复协议、Manifest、事件重放和 Native Shell 契约。Docker daemon 在当前环境不可用，实际 Docker 启停和进程清理场景还没有验证，因此本记录保留在 `proposed/`。

## 状态所有权

| 状态或副作用 | 权威 | 其他层职责 |
| --- | --- | --- |
| Turn loop、事件和对话写回 | Core | App Server 消费并持久化 Session 投影；客户端订阅和展示 |
| 工具准入、批准和执行 | Host、Capabilities | App Server 传递请求；Web 只呈现批准或核对操作 |
| Thread、Session checkpoint、Execution checkpoint 和 journal | App Server、SessionStore | SDK、Gateway 与 Studio 读取、提交有界请求，不复制账本 |
| Context Manifest 与事件回放摘要 | SessionStore | App Server 暴露有界读取；Web 展示来源并按 canonical Thread/Item 对账 |
| Shell 进程及 sandbox | Capabilities | App Server 控制生命周期；Native 与 Docker 实现遵守同一执行契约 |

## 已落地决策

### 重启恢复需要显式处置

App Server protocol version 和 Session schema 都使用 V2。V1 客户端在握手时被拒绝，V1 Session 以明确的 schema 错误拒绝打开，不自动迁移或改写旧文件。升级前先用旧版检查、导出或归档需要保留的 Session。

`turn/reconcile` 接受 `turnId`、`checkpointSeq`、`toolCallId`、`requestId`、处置和有界证据摘要。`completed` 必须带有界结构化结果；`not_executed` 表示操作者确认副作用没有发生。相同请求幂等，过期 checkpoint 和冲突请求 ID 被拒绝。存在未核对调用时，App Server 阻止 `turn/resume` 和覆盖该 checkpoint 的新 Turn。核对不会启动工具或恢复 Turn。

恢复会复用已持久化的工具结果和原 Turn 身份。对“工具已启动但结果未知”的调用，App Server 不会自动重试。操作者选择 `not_executed` 后，仍需显式恢复。

### Context 来源留在 Session 元数据中

Session-owned Context Manifest 记录来源 ID、名称、类型、版本指纹、路径、适用范围、权限依据、注入原因、字节数和复用情况。它不存来源正文或密钥，也不作为模型输入。Manifest 最多保留 512 条，文件上限为 1 MiB，每次追加最多 32 条，每个元数据字段最多 512 字节。

Studio 主对话和 Child Session 详情均从 App Server 读取 Manifest。Gateway 不创建第二份来源账本。

### 事件重放只保存有界摘要

Session 持久化最多 512 条、总计不超过 1 MiB 的事件摘要。摘要包含事件类型、Thread/Turn/Item 身份、工具调用身份和 Context 来源指纹。`itemId` 用于从 canonical Thread/Item 投影读取结果；重放文件不复制 delta、prompt、工具参数、工具输出或 Context 正文。

`turn/events` 的 `afterSequence` 为排他游标。若保留窗口不足，客户端从 `thread/read` 和 `thread/items/list` 对账。断线恢复只恢复投影，不重新提交用户输入。

### Shell 契约只用于本机开发

Native 和 Docker 共用工作目录、workspace 挂载边界、120 秒超时、8 MiB 合并输出上限和进程树清理要求。Native timeout 或取消会终止进程树。Docker 按容器名停止并确认容器已退出。Docker daemon 不可用、容器启动失败或清理无法确认时，操作失败关闭，不回退到 Native。

该契约提供本机开发隔离，不承诺抵御恶意代码。当前环境中的 `docker info` 无法连接 Docker socket，因此 Docker 的运行时清理证据待补。

## 跨仓接口

| 语义 | Harness | Web |
| --- | --- | --- |
| 协议协商 | `PROTOCOL_VERSION = 2`；新增 `turn/reconcile`、`session/context_manifest` 和有界事件回放 | Python SDK 使用 V2 类型化 API；Gateway 转发 App Server 状态 |
| 恢复状态 | Session execution journal 是唯一权威 | 主对话和 Child Session 展示核对表单，不维护影子状态 |
| 来源审计 | SessionStore 写入 Manifest | 主对话与 Child Session 查看来源元数据 |
| 事件缺口 | SessionStore 保留有界摘要，Thread/Item 为 canonical 投影 | Gateway 暴露游标结果；Studio 遇到缺口时读取 canonical 投影 |
| 旧格式 | 拒绝 V1 客户端和 Session，不自动迁移 | SDK 握手不接受 V1；用户先使用旧版导出或归档 Session |

## 验证记录

### Harness

- `cargo fmt --all` 通过。
- `mini-agent-core`、`mini-agent-app-server-protocol`、`mini-agent-capabilities` 和 `mini-agent-app-server` 定向测试分别通过 56、22、170 和 94 项库测试；App Server binary 测试 1 项通过。
- 上述四个包的 `--all-targets` Clippy 检查通过，参数为 `-D warnings`。
- `python3 scripts/cargo_boundary.py --json` 通过；报告保留既有 App Server 到 Capabilities 的 review edge。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json` 无违规：Core + Protocol 为 6,490 / 7,000，Control Plane 为 40,530 / 45,000，Release Rust 为 58,504 / 65,000。相对基线分别增加 95、1,281 和 1,903 行。Release 增量超过 1,000 行评审参考值，但未超过硬上限。
- App Server 崩溃恢复测试覆盖结果已持久化与工具已启动但结果未知两种情况。重复核对保持幂等，旧 checkpoint 被拒绝，恢复不重跑已结算副作用。Manifest 与事件环也覆盖重新打开 Session 后读取和容量限制。
- Shell 定向场景覆盖 Native 取消、超时、输出上限和进程清理。Docker runtime 场景未运行。

### Web

- `npm run lint -- --no-fix` 通过。
- `npm run test:ui` 通过，33 个测试文件共 195 项。测试覆盖主对话及 Child Session 的手动核对呈现。
- 变更涉及的 11 个 Python 文件通过 Ruff 检查和格式检查。SDK/Gateway 定向 pytest、V2 App Server 重启 SDK 测试，以及离线 Cookbook demo 06、07 均通过。
- UI 测试进程退出码为 0；Happy DOM teardown 输出未完成 fetch 的 `AbortError` 诊断。没有运行全工作区测试、付费 Provider 调用或手动浏览器 smoke。
- 两仓 `git diff --check` 通过。

## 准入问题

1. **所属层：**恢复账本由 SessionStore 持久化，核对 RPC 由 App Server 管理；SDK、Gateway 和 Web 只调用、投影该契约。Shell 副作用属于 Capabilities。Core 不增加恢复策略。
2. **既有职责：**SessionStore 已拥有 Session journal；本次扩展该 journal 及其有界 sidecar。App Server 负责协议 admission。检索确认 Gateway 和 Studio 没有现成的恢复账本，也不新增一份。
3. **旧概念：**保留 Session checkpoint 与 Execution checkpoint 两种不同含义。拒绝旧协议和旧 Session，不加自动迁移、静默 fallback 或旧字段别名。
4. **预算：**Core + Protocol 为 6,395 → 6,490（+95）；Control Plane 为 39,249 → 40,530（+1,281）；Release Rust 为 56,601 → 58,504（+1,903）。硬限制均通过，Release 增量高于建议参考值。
5. **可见面：**公共协议升为 V2，Session schema 升为 V2。Manifest 上限为 512 条和 1 MiB；事件摘要上限相同。来源正文、prompt、delta、工具参数和工具输出不写入这两份元数据，也不扩展模型输入。
6. **边界测试：**已运行上述受影响 Rust 包测试、Clippy、格式检查、Web SDK/Gateway/UI 定向测试和离线 scenario。Docker daemon 未运行，Docker 启停和清理路径仍缺真实运行证据。

## 收口条件

在可用的 Docker daemon 上运行 Native 与 Docker 共用场景，确认超时、取消、进程树清理、输出上限和失败关闭。补齐证据后再将本记录移入 `implemented/architecture/`。提 PR 时应考虑按恢复协议、Context/事件元数据、Shell 契约拆分评审批次，以降低单个变更的 Release Rust 行数。
