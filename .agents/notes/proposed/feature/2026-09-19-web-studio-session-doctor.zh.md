# WebStudio Session Doctor

状态：提案
日期：2026-09-19
适用仓库：`mini-codex`、`mini-agent-web`

## 结论

WebStudio 将提供项目级的手动 Session 检查。用户从项目菜单发起扫描后，Gateway 调用 App Server 的一次性维护入口。维护入口直接使用 Capabilities 的 Session 解析器，不启动模型或恢复目标 Session。报告列出主日志的完整性、恢复状态和处理建议。

扫描保持只读。报告只将未完成的日志尾部列为可修复项。用户明确选择修复后，系统重新校验该日志，在排除并发写入后先保存备份，再截去未完成的尾部。序号断档和 `recovery_gap` 表示已有内容缺失，不能自动补造。

本提案不增加面向用户的 `mini-agent doctor` 命令，不在项目加载或打开 Session 时自动扫描，也不检查索引、sidecar、附件、Notebook 或 plan 文件。

## 当前问题与证据

| 观察 | 代码证据 | 影响 |
| --- | --- | --- |
| Capabilities 在恢复时严格解析主日志，并要求记录序号连续。 | `crates/mini-agent-capabilities/src/session/storage.rs` 的 `load_records`；`crates/mini-agent-capabilities/src/session.rs` 的 `SessionStore::resume` | 序号断档、完整记录损坏或缺少 settled checkpoint 会在恢复时失败。 |
| 恢复路径会在取得 Session 锁后截掉未以换行结尾的尾部。 | `SessionStore::resume` 使用 `LoadedRecords::valid_bytes` 调用 `set_len`。 | 现在只有运行时打开 Session 时才会发现并处理这类尾部。用户无法先看报告或备份。 |
| Gateway 的 Session catalog 是展示投影，不检查日志序号。 | `mini-agent-web/server/session_catalog.py` 的 `_read_session_records`、`_read_session`、`list_sessions` | 列表可能展示可投影但不能恢复的 Session。坏日志问题会延后到 SDK attach 或 App Server 恢复时暴露。 |
| 常规 App Server 初始化会打开 Session 并构建 Host runtime。 | `crates/mini-agent-app-server/src/bin/mini-agent-app-server.rs` 的启动回调调用 `open_session`，随后构建 `HostRuntimeFactory`。 | 损坏的默认 Session 可能阻断 RPC 初始化，所以 doctor 不能依赖同一条 Session attach 路径。 |
| 已恢复日志可包含 `recovery_gap` 标记，而加载器会接受 header 后的未知记录类型。 | `load_records` 对 header 后未知 kind 的兼容分支；本次修复日志使用的 `recovery_gap` kind | 日志可以继续加载，但仍缺少历史记录。Doctor 必须区分“可以恢复”和“历史完整”。 |

## 目标和非目标

目标：

- 用户可以从 WebStudio 项目菜单扫描该项目下所有已落盘 Session 的 `session.jsonl`。
- 报告指出具体 Session、问题类别、记录位置和下一步建议，不复制 prompt、工具参数或工具结果。
- 用户可以明确修复没有并发写入、且只包含未完成尾部的问题。
- 修复前保留可还原的原始日志；修复失败时不截断日志。

非目标：

- 不扩展实验性 `mini-agent` CLI。
- 不在打开 Session 前自动扫描，也不在后台周期性扫描。
- 不由 Gateway 在 Python 中重新实现 Rust 日志校验规则。
- 不自动修复序号断档、`recovery_gap`、损坏的完整记录或缺少 checkpoint。
- 不校验 `thread_index.json`、JSON sidecar、附件、Notebook、plan 或附件引用。
- 不新增模型可见工具、事件或会话 schema 版本。

## 所有权和数据流

| 对象 | 唯一所有者 | 其他层的职责 |
| --- | --- | --- |
| Session 主日志解析、问题分类和修复 | Capabilities | 复用现有 `load_records` 规则；不暴露任意路径写入。 |
| 一次性诊断进程 | App Server 可执行文件 | 提供不启动 Session、Host 或 provider runtime 的维护入口。 |
| 项目路径和 HTTP 请求 | Gateway | 根据 `project_id` 查项目主工作区，启动维护入口并转发有界 JSON 报告。 |
| 扫描入口和结果展示 | WebStudio | 从项目菜单手动发起扫描，展示报告并提供显式修复动作。 |

```text
WebStudio 项目菜单
  → Gateway 项目级 doctor API
  → mini-agent-app-server doctor（cwd 绑定项目主工作区）
  → Capabilities Session 日志检查
  → 结构化报告返回 WebStudio
```

Gateway 不能接受浏览器传入的绝对路径。它只接受已登记的 `project_id` 和通过验证的 `session_id`。Rust 维护命令从项目主工作区的当前目录确定 workspace。扫描器从 `session_directory(workspace)` 枚举 Session 目录，并逐个读取 `session.jsonl`。

## 接口草案

### App Server 维护入口

```text
mini-agent-app-server doctor --json
mini-agent-app-server doctor repair --session-id <id> --json
```

该入口是 Gateway 调用的内部维护模式，不出现在 `mini-agent` CLI 的用户命令文档中。App Server 在解析到 `doctor` 后直接调用 Capabilities 并退出，不加载 `RuntimeConfig`、SessionStore writer、模型、provider 或 Host runtime。stdout 只输出 JSON，stderr 用于进程级错误。扫描即使发现问题也以成功进程状态返回报告；无法完成扫描或修复时返回非零状态和有界错误。

### Gateway 路由

| 方法和路径 | 行为 |
| --- | --- |
| `POST /api/threads/project/{project_id}/sessions/doctor` | 手动扫描项目主工作区的 Session 日志。请求不接收 workspace 路径。 |
| `POST /api/threads/project/{project_id}/sessions/{session_id}/doctor/repair` | 请求 Capabilities 重新检查并修复该 Session 的未完成尾部。请求不接收文件路径或任意修复偏移。 |

Gateway 使用现有 `MINI_AGENT_APP_SERVER_PATH` 指向的可执行文件，将 `cwd` 设置为项目的 `primary_path`。Gateway 不缓存诊断结论，也不修改 Session 文件。

### 报告字段

报告包含 `schema_version`、`scanned_sessions`、各状态计数、`findings_truncated` 和至多 256 条问题记录。每条问题记录包含 `session_id`、稳定的 `issue_code`、状态、恢复可用性、修复可用性、记录行号或字节偏移、适用时的序号，以及一条建议。报告不包含日志正文或绝对路径。

## 问题和修复规则

| 问题 | 报告状态 | 建议或修复 |
| --- | --- | --- |
| 无问题且日志可恢复 | `healthy` | 无需操作。 |
| `recovery_gap` | `history_incomplete` | 报告 `missing_seq`。允许继续恢复时明确显示这一点；提示恢复原始备份或创建新 Session。不能重建缺失内容。 |
| 完整行 JSON 损坏、序号不连续、header/schema 不合法或 checkpoint 无效 | `corrupt` | 报告行号、偏移或 expected/found；建议恢复备份或创建新 Session，不插入占位记录。 |
| 文件尾部存在未完整写入的 JSONL 记录，前缀通过完整解析且含 settled checkpoint | `repairable_tail` | 默认只报告。用户确认后取得独占锁、重读并重验日志，备份原文件，再截断到最后一条完整记录并同步文件。 |
| 存在 `session.lock` | `locked_unverified` | 不把并发写入判成损坏，不允许修复。扫描报告标记该 Session 未检查，因为只读 doctor 不判断锁是否过期，也不清理或重领锁。 |
| 无法读取、超限或 Session 目录结构异常 | `unreadable` | 报告受限原因和检查路径所需的下一步；不修改数据。 |

未知记录类型在 header 后仍按现有兼容规则接受；`recovery_gap` 是单独识别的已知数据缺口。这样不会因未来新增 kind 而误报损坏。

修复备份放在 `~/.mini-agent/recovery-backups/<workspace-key>/<session-id>/` 下，文件名带 UTC 时间。若无法创建并同步备份，修复立即失败。修复操作在取得锁后重新执行日志校验；如果报告已过期、问题已消失或问题类别不再是可修复尾部，操作不写入日志。成功后返回备份相对路径和新的诊断结果。

## WebStudio 流程

1. 用户打开项目行的菜单，选择“检查 Session 数据”。
2. WebStudio 显示正在检查状态；Gateway 返回结果后展示总数和 Session 问题列表。
3. 用户展开问题查看行号、序号、恢复状态和建议。
4. 只有 `repairable_tail` 显示“备份并修复”。WebStudio 在执行前说明将截去未完成尾部并保留备份，再要求用户确认。
5. 修复成功后 WebStudio 重新扫描该项目，并显示新的报告和备份位置。

项目扫描是手动的，不会因为展开项目、刷新 Session 列表或 attach 而隐式运行。正常 Session 列表仍按现有 catalog 展示。

## 验收标准

- **AC-1 完整日志。** 给定含连续序号、有效 header 和 settled checkpoint 的日志，执行项目扫描后报告该 Session 可恢复且无数据完整性问题；日志字节保持不变。
- **AC-2 序号断档。** 给定 expected sequence 为 130、实际记录为 131 的日志，报告 `expected_seq=130`、`found_seq=131` 和对应位置；不得报告为健康或可自动修复。
- **AC-3 已标记缺口。** 给定包含 `recovery_gap` 的可加载日志，报告历史不完整和 `missing_seq`；保持可恢复性字段与完整性状态分离。
- **AC-4 尾部截断。** 给定有效前缀和未以换行结束的尾记录，扫描不修改文件。用户明确确认后，系统先备份，再截去尾部；备份字节等于修复前原始日志。
- **AC-5 并发保护。** 给定存在 `session.lock` 的 Session，扫描标记为 `locked_unverified`；修复被拒绝且日志不变。
- **AC-6 Provider 独立。** 给定没有 provider key 的环境，App Server doctor 仍可完成检查；trace 证明它没有加载 Host/provider runtime 或打开 Session writer。
- **AC-7 范围限制。** 给定浏览器传入未登记项目或任意文件路径，Gateway 拒绝请求；有效项目请求只能扫描其主工作区日志。
- **AC-8 有界隐私。** 给定损坏记录含 prompt 或工具数据，报告不返回这些正文；超过 256 条 finding 时返回截断标记和完整计数。

## 实施批次

### Batch 1：共享的 Session 诊断结果

- Scope：Capabilities Session storage。
- Replace：将当前 parser 的字符串失败结果改为结构化问题，同时让恢复和 doctor 共用同一套验证，不新增第二个 parser。
- Contract delta：新增 workspace 级只读检查类型和显式尾部修复操作；不改 JSONL schema。
- Delete：评估将单一调用方 `runtime_actor` 使用的 `SessionStore::read_settled_checkpoint` 读文件逻辑迁移到共享读取结果，随后删除旧的一次性读取接缝。此项是预期的预算抵消候选，必须在实现前用调用图和实际 delta 确认。
- Evidence：Capabilities 单测覆盖上述记录类型、锁状态、修复重验和备份失败；保留一个 `recovery_gap` 回归样例。
- Stop：如果没有真实可验证的抵消项，先缩小实现或明确预算例外，不进入跨仓接线。

### Batch 2：App Server 维护入口和 Gateway API

- Scope：App Server 可执行文件、Gateway 路由和结构化响应。
- Replace：复用现有 App Server 可执行文件路径，不新建 Gateway 日志 parser 或 Gateway Session 写入器。
- Contract delta：新增内部 doctor/repair 子命令和项目级 REST 请求；不新增 JSON-RPC 或 SDK 协议方法。
- Evidence：确认无 provider key 也能运行；Gateway 测试覆盖项目绑定、进程失败和修复拒绝。
- Stop：如果维护模式仍会初始化正常 runtime 或尝试打开目标 Session，停止并调整启动分支。

### Batch 3：WebStudio 报告与显式修复

- Scope：项目菜单、报告面板、修复确认和排查文档。
- Replace：以项目级报告替代用户查看 Gateway 日志或等 attach 报错后再定位主日志问题的手工流程。
- Contract delta：使用 Batch 2 的 REST 响应；不在前端缓存报告作为 Session 权威状态。
- Evidence：UI 测试覆盖手动扫描、问题展开、确认修复、失败反馈和修复后重扫。
- Stop：任何失败响应都不能清空当前对话、执行修复或隐藏已有问题报告。

## 证据计划

Capabilities 与 App Server 测试使用临时 workspace 和本地 JSONL fixture，不调用模型 provider。Gateway 和 WebStudio 使用 stub 维护进程，验证进程参数、项目路径绑定和用户反馈。增加一条 bounded Harness Scenario：一个损坏 Session 在正常 runtime 初始化前通过维护入口报告；一条明确修复路径生成原始备份并只截去不完整尾部。

实现后运行受影响 Rust package 的 fmt、clippy 和测试，运行 Gateway/UI 聚焦测试，并运行 `python scripts/line_budget.py` 与增量检查。完整 workspace 测试不属于本提案的本地默认验证。

## 六问准入

1. **归属：** Session 日志检查和修复属于 Capabilities；无 runtime 维护入口属于 App Server；项目路由属于 Gateway；报告显示和用户确认属于 WebStudio。Core 不变。
2. **责任复用：** `load_records` 已拥有日志语法、序号、header、item 和 checkpoint 验证。Gateway `session_catalog` 继续负责列表投影，不承担完整性判定。
3. **旧概念：** 不增加第二个 parser。共享读取结果应取代 `read_settled_checkpoint` 的一次性读取实现，并在同一批次迁移唯一调用方后删除旧入口。
4. **预算：** 当前基线为 Core + Protocol 4,663 行、Control Plane 26,208 行、release source 38,612 行。目标是 Core + Protocol 净增为零、release source 净增为零。`read_settled_checkpoint` 的迁移是预算抵消候选，必须在实施前核实有效行数。若抵消不足，先缩小范围或记录经评审接受的抵消，不超过当前 +300 行增量门槛。
5. **可见面：** 不增加模型可见输入、事件或 Session schema。新增 Gateway REST 路由和内部 App Server 维护调用。显式修复新增备份文件；报告不暴露日志正文。
6. **边界证据：** Capabilities 和 App Server fixture、Gateway 路由测试、WebStudio 交互测试，以及一条 bounded persistence scenario。完整 workspace 测试需单独批准。

## 风险和停止条件

- 日志可能在报告后改变。修复必须取得独占锁并重新读取、重新校验；检查结果过期时拒绝写入。
- 进程锁存在时无法仅凭只读扫描判定它是否过期。Doctor 将其报告为进行中或不确定，不尝试重领。
- 序号断档和 `recovery_gap` 无法从现有文件重建原始内容。建议恢复人工备份或创建新 Session。
- 维护子命令缺失、执行失败或返回格式无效时，Gateway 应展示具体诊断不可用原因，不应回退到 Python 自行解析。
- 报告状态只证明本次读取到的主日志，不证明索引、附件或其他 sidecar 完整。
- 只有满足备份、锁和重验条件的末尾截断可以进入自动修复。其他修复需另立提案。

实现完成并通过证据后，将本文从 `proposed/feature/` 移到 `implemented/feature/`，改写为实际决策和验证结果。
