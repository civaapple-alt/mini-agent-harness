# 提案质量与证据指南

状态：已采用的过程指南  
日期：2026-09-04  
适用范围：`mini-codex` 及其 SDK、网关、Studio 等协作仓库

## 1. 目的

提案不是功能愿望清单，也不是实现完成后的宣传稿。它应当在动手前把四件事
说清楚：

1. 要验证的 harness 假设是什么；
2. 哪一层拥有每个状态、决策和副作用；
3. 什么可观察证据可以证明成功或推翻假设；
4. 失败时哪些数据、权限、兼容边界和预算不会被破坏。

本指南用于提案起草、分批实现、跨仓同步和状态晋级。它补充而不替代
[`AGENTS.md`](../../AGENTS.md) 与
[`pull_request_template.md`](../../.github/pull_request_template.md)；
后两者的硬性规则仍然优先。

## 使用入口：先按角色执行

这是一份工作手册。不要从头背诵；根据当前任务选择一条路径。

| 当前任务 | 先做什么 | 最低完成证据 |
| --- | --- | --- |
| 只写提案 | 填结论、证据表、所有权表和验收表 | 评审者可以复现现状并知道何时停止 |
| 单仓实现 | 复制批次卡，先删旧概念，再实现一个假设 | 受影响包测试、边界测试和预算 delta |
| Core → App Server → SDK → Web | 先填契约矩阵，再按权威层向外同步 | 每层 fixture，加一条跨层 scenario |
| 收口或移入 `implemented/` | 对照验收表逐条回填结果 | 所有标准有证据，剩余风险明确可追踪 |

### 提案作者：30 分钟得到第一版

1. **建立基线**：确认工作树状态，记录 Rust 行数和当前测试入口。

   ```text
   git status --short
   python scripts/line_budget.py
   rg -n "<symbol-or-concept>" .
   ```

   `line_budget.py` 只适用于包含该脚本的 Rust 主仓；跨仓任务要分别记录各仓
   基线。检索时可用 `--glob` 限定语言或目录，避免把生成物当成证据。

2. **先写结论**：填目标、非目标、用户流程、所有者链路和不做迁移兼容的决定。
3. **列证据**：每个问题填文件/符号、实际行为、影响、根因和反例；没有代码
   证据的问题先标为“待验证”，不能写成现状。
4. **画责任矩阵**：至少覆盖权威状态、写入者、消费者、禁止重复实现；跨仓
   变化再填 Core/Host → App Server → SDK → Web 的契约矩阵。
5. **写验收标准**：每条用“给定前置条件、执行操作、观察 trace、失败反例、
   证据命令”表达。
6. **拆批次**：每批只验证一个主要假设，优先删除旧概念，并写出预计抵消。

第一版不要求完美，但必须让评审者能回答“改什么、谁负责、如何证明、何时停”。

### 实施者：每批次照着批次卡走

开始编码前，从提案复制一张批次卡并填写：

```markdown
### Batch <N>: <one hypothesis>
- Scope: <layers, repositories, files>
- Delete/replace: <old concept or duplicate authority>
- Contract delta: <types, RPC, SDK, Web; or N/A>
- Expected budget: runtime <...>; release <...>
- Evidence: <unit / fixture / scenario / eval>
- Stop conditions: <what blocks this batch>
- Commit: <one reviewable conclusion>
```

批次结束时补上实际结果、测试输出摘要和剩余风险；如果发现假设不成立，先
记录反例并停止扩展范围，不要用更多代码掩盖失败。

### 评审者：先看四个阻断项

在细读实现前检查：

- 是否有唯一权威，还是新增了第二个状态/批准/持久化缓存；
- 是否有可失败的验收标准和至少一个反例；
- 是否有实际预算 delta，而不是只写“很小”；
- 是否明确了跨仓契约、迁移边界和未完成项。

任一项缺失时，先退回提案补证据，不进入实现评审。这样可以把评审时间花在
行为和边界上，而不是猜作者没有写出的意图。

### 完成者：用一段话交付

最终更新应能直接回答：

```text
已完成：<user-visible behavior / authority change>
未完成：<concrete gap or N/A>
证据：<commands, scenario/eval, key result>
预算：runtime <before -> after>; release <before -> after>
提交：<commit>
```

## 最小提案交付物

下表是进入评审的硬门槛。后续章节解释如何写，不能用长篇背景替代这些字段。

| 交付物 | 最低内容 | 缺失时的处理 |
| --- | --- | --- |
| 一页结论 | 目标、非目标、用户流程、所有者链路 | 保持草稿，不进入实现 |
| 证据表 | 观察、路径/符号、影响、根因、反例 | 标记待验证并补探查 |
| 所有权表 | 唯一权威、写入者、消费者、禁止重复 | 暂停设计，先消除职责冲突 |
| 验收表 | 输入、操作、trace、反例、证据 | 不得写“已实现” |
| 批次卡 | 假设、范围、删除项、预算、停止条件 | 不得扩大单批范围 |
| 六问 | PR 模板六题的具体回答 | 不能提交变更 |
| 验证记录 | fmt、lint、test、line budget 和 scenario | 状态最多为 partial |

## 一次性工作表

可以把下面的顺序贴到任务描述或 PR 描述中，逐项勾选；它比阅读整篇指南更适合
日常使用：

```text
[ ] 目标/非目标/用户流程已写
[ ] 已运行 rg 并记录关键代码证据
[ ] 已指定唯一权威和禁止重复的层
[ ] 已写至少一个会失败的验收标准和一个反例
[ ] 已检查是否可以删除旧概念或兼容路径
[ ] 已记录基线、预计 delta 和预算余量
[ ] 跨仓时已填契约矩阵和版本/字段映射
[ ] 触及 loop/context/event/persistence 时已安排 bounded scenario/eval
[ ] 每个批次有停止条件和单独提交结论
[ ] 最终状态与测试、文档和实际提交一致
```

## 2. 高质量提案的最小结构

### 2.1 一页结论

开头先给出：

- 一句话目标和明确的非目标；
- 当前用户如何操作，以及新行为与旧行为的差异；
- 所有者链路，例如 `agent core → app-server → SDK → Web Studio`；
- 关键决策、未决问题和建议顺序；
- 当前基线、预算余量和完成定义。

如果读者只读这一页，也应能判断提案是否值得进入实现。

### 2.2 证据化问题清单

每个问题都使用如下字段，不写泛化的“架构混乱”或“需要加强”：

| 字段 | 要求 |
| --- | --- |
| ID | 可在后续批次和验收标准中引用的稳定编号 |
| 观察 | 当前确实发生的行为，不是推测 |
| 代码证据 | 仓库、文件、符号或行号；必要时附命令输出 |
| 影响 | 对用户、权限、持久化、恢复或模型行为的具体风险 |
| 根因 | 所有权错误、重复状态、缺少边界或错误的默认值 |
| 反例 | 什么情况会证明这个问题判断不成立 |
| 拟删除项 | 能否删除旧概念、回退路径、缓存或重复 UI |

证据必须区分“已观察”“推断”和“待验证”。不要把设计意图写成当前事实。

### 2.3 所有权和负空间

为每个状态和动作指定唯一权威，并同时列出明确不负责的层：

| 对象 | 唯一权威 | 其他层允许做什么 | 其他层禁止做什么 |
| --- | --- | --- | --- |
| Turn loop、stop reason、事件 | Core | 消费事件、展示结果 | 自建第二个 loop 或改写结算 |
| 工具准入、批准、具体副作用 | Host/Capabilities | 传递请求、展示审批 | UI 或 SDK 私自放行 |
| Thread、Goal、Session history | App Server/SessionStore | 提供协议投影 | Web 自建 checkpoint 权威 |
| 进程连接、JSON-RPC、类型解析 | SDK | 连接、限流、解析 | 重新解释运行时语义 |
| Project 清单、页面状态 | Web/网关 | 选择项目、发起请求 | 伪造 Session 或批准状态 |

如果一个对象需要两个“方便访问”的缓存，提案必须说明为什么不能删除其中
一个，并给出失效、并发和恢复证据。优先删除重复权威，而不是增加同步器。

## 3. 控制面设计规则

### 3.1 正交维度，不制造总开关

访问模式、批准生命周期、Thread 工作流模式必须分开描述。例如：

```text
Thread mode: chat | plan | goal
Access scope: project | full_machine
Approval lifetime: per_action | current_session | current_project
```

`full_machine` 只表示路径范围，不表示全部 Allow；`Goal + full_machine +
current_project` 是一个用户场景组合，不应重新命名为 Profile、Turbo 或其他
隐含总开关。任何组合都不能绕过 Deny、Plan 锁、工具可用性和高风险确认。

### 3.2 准入顺序必须可追踪

提案应给出一条固定顺序，并为每一步提供可观察 trace：

```text
Deny → Plan lock → workspace/sandbox → approval → execution → receipt/event
```

权限扩大只能扩大对应维度；批准只能表达批准生命周期；UI 不能改变 Host
的安全结论。验收中至少要有一个“看似允许但仍被拒绝”的反例。

### 3.3 用户语言和内部状态分离

面向用户只保留能改变结果的选择。内部 `step_limit`、`max_steps`、进程锁、
超时和清理状态必须转换成可理解的状态和下一步，而不是直接把内部枚举贴在
页面上。提案要同时写：

- 用户动作：选择、暂停、恢复、批准、撤销；
- 运行时动作：继续、拒绝、等待锁释放、清理或结算；
- 可见反馈：状态、原因、是否可恢复、下一步操作。

## 4. 可证伪验收标准

### 4.1 每条标准都必须能失败

验收标准使用“给定输入 → 可观察结果”的形式，避免“体验良好”“正确支持”
等不可执行表述。推荐字段：

| 字段 | 示例 |
| --- | --- |
| ID | `AC-07` |
| 场景 | 当前 Project 中两个 Thread 使用同一批准 |
| 前置条件 | `FullMachine + current_project`，动作和 path scope 相同 |
| 操作 | Thread B 再次请求同一动作 |
| 预期 trace | 不产生第二个 UI approval request，记录复用原因 |
| 反例 | revision 或 path scope 改变后必须重新批准 |
| 证据 | 单测、协议 fixture、Harness Scenario 或离线 eval |

### 4.2 覆盖“正常”和“边界”

控制平面提案至少覆盖以下反例类别：

- Deny 优先于 FullMachine 和批准；
- Plan 可创建受控 scratch 产物，但正式源码写入仍被锁定，cleanup failure 可见；
- current-project 批准绑定 Project、workspace identity、revision、path scope
  和 action，目录变更后失效；
- 外部进程持有 Session lock 时可以读历史，但不能被第二进程抢占；
- paused Session 无锁时仍显示暂停并可恢复；
- Goal 跨 Turn 继续推进，达到运行时安全边界后有明确结算；
- SDK/App Server 无响应时超时、清理 pending request 并报告原因；
- 未知工具、未知事件、非法旧输入均 fail closed。

### 4.3 证据分层

优先使用离线证据，不以真实 Provider 调用作为默认门槛：

1. 类型和公共边界单测；
2. JSON-RPC 协议 fixture；
3. Mock Provider 的 Harness Scenario/Eval；
4. SDK、网关和 Studio 的集成测试；
5. 必要时才做人工 UI 检查或真实环境验证。

凡是触及 prompt、tool schema、loop、context、event 或 persistence，仅有公共
单元测试不充分；提案必须列出 bounded scenario，或者解释为什么该变化不影响
模型可见行为。

## 5. 六问准入的写法

`.github/pull_request_template.md` 的六问不能只填“见代码”。每次提案和每个
实现批次至少回答：

1. **所属层**：为什么由该层拥有，为什么不能放到相邻层；
2. **重复职责**：检索过哪些符号、路径、API 和状态文件；
3. **旧概念**：删除、替换了什么；若保留，失效边界是什么；
4. **预算**：runtime 与 release-source 的 before/after/delta，包含测试；
5. **可见面**：新增输入、事件、持久化和公共协议的上限与兼容边界；
6. **边界测试**：具体命令、scenario/eval、失败反例和仍缺的证据。

“增加一层方便未来扩展”不是准入理由。必须先写出当前要区分的两个有用行为、
可观察 trace，以及该层永久增加的复杂度。

## 6. 预算与批次纪律

### 6.1 先删后加

每个批次开始前建立基线，结束后重新运行：

```text
cargo fmt --all
cargo clippy -p <affected-package> --all-targets -- -D warnings
cargo test -p <affected-package>
python scripts/line_budget.py
```

跨包变更再运行 workspace clippy；不要未经明确批准在本地运行完整 workspace
测试。实验性 CLI/REPL 的行数单独报告，不用它掩盖 release/runtime 增长。

### 6.2 墙边余量处理

当 runtime 或 release-source 余量低于 5% 时进入预算警戒；低于 250 行时，
新增 Rust 默认暂停，除非同一批次提供可核验的删除抵消。提案必须记录：

```text
expected: runtime +0 / -N; release +0 / -N
actual:   runtime before -> after (delta)
          release before -> after (delta)
```

门禁为绿不代表可以继续堆代码。余量、复杂度和新增公共面都应进入完成判断。

### 6.3 一个批次一个结论

批次应按一个可审查的假设切分，例如“移除 Profile”和“Session attach”分开。
每批只提交相关文件，提交信息说明行为变化；完成后再开始下一批。若中途发现
边界错误，先提交小修复并更新证据，不把多个未验证假设捆成一个大提交。

## 7. 跨仓同步规则

当变更跨越 `agent core → app-server → SDK → Web Studio` 时，提案必须附一张
契约矩阵：

| 语义 | Core/Host 类型 | App Server RPC/Event | SDK 类型/API | Web 路由/状态/UI |
| --- | --- | --- | --- | --- |
| 访问范围 | 精确枚举和准入 | 字段名、默认值、拒绝错误 | 类型化字段 | 用户文案和选择器 |
| 批准生命周期 | Host 决策和 key | request/response envelope | 回调和响应校验 | 待批准与撤销 |
| Session | lock、checkpoint、status | list/read/attach | 解析与超时 | 历史/运行/暂停切换 |
| Goal/Plan | runtime 状态和边界 | set/get/notification | 映射 API | 顶部状态和操作 |

同步顺序应从权威层向外扩散：先稳定类型和行为，再接通 RPC，再更新 SDK，
最后更新 Web；每一层都提供最小 fixture。任何一层临时翻译未知值，都必须有
明确的拒绝或降级语义，不能静默扩展权限。

## 8. 迁移与持久化边界

提案必须明确“读取、写入、删除、迁移”四种动作。若产品决定直接切换，需明确
旧输入和旧文件全部不在新 Runtime 范围内，并写出拒绝证据；不要在实现中偷偷
加入 fallback、兼容别名或自动导入。

Session、批准和 Web UI 元数据要分别列出所有权：

- canonical Session history 只有一个写入者；
- Web 可保存 Project 清单和页面偏好，但不能复制 checkpoint 权威；
- current-project 批准的内存缓存必须可精确失效和撤销；
- 锁定、暂停、恢复和清理失败都必须可读、可审计、可恢复或明确不可恢复。

## 9. 状态晋级和诚实报告

提案状态只允许由证据推动：

```text
proposed → partially implemented → verified → implemented
                         ↘ rejected / archived
```

`implemented` 至少需要：

- 所有验收标准有对应证据或明确标记为不适用；
- 正常、边界和拒绝路径已验证；
- 文档、协议、SDK、UI 的词汇一致；
- 预算、测试、lint、fmt 结果已记录；
- 未完成事项不会隐藏在“基本完成”措辞后。

如果仍有 UI 缺口、锁竞争、cleanup pending、兼容性决定或测试 warning，状态应
保留为部分完成，并在“剩余风险”中指向具体文件和下一步，而不是提前移动到
`implemented/`。

## 10. 提案审查清单

提交提案或将其标记为完成前，逐项确认：

- [ ] 一页结论包含目标、非目标、用户流程和所有者链路；
- [ ] 每个问题有文件/符号证据、影响、根因和反例；
- [ ] 每个状态只有一个权威，重复缓存有删除或失效理由；
- [ ] 权限、批准、模式和用户文案没有被合并成隐含 Profile；
- [ ] 每条验收标准都有输入、输出、trace 和失败反例；
- [ ] 涉及模型或持久化时有 bounded Harness Scenario/Eval；
- [ ] 六问、行数预算和跨仓契约矩阵已填写；
- [ ] 迁移/兼容策略是显式选择，不是意外 fallback；
- [ ] 文档只保留一个 canonical explanation，历史记录没有被改写成 changelog；
- [ ] 结论与当前提交和测试结果一致。

## 11. 可复制的提案骨架

```markdown
# <Title>

状态：提案中
日期：YYYY-MM-DD
范围：<repositories / layers>

## 结论
- Desired change:
- Non-goals:
- User flow:
- Ownership chain:

## Harness hypothesis
<What behavior are we testing, and why does it matter?>

## Current evidence
| ID | Observation | Code evidence | Impact | Root cause | Counterexample |
| --- | --- | --- | --- | --- | --- |

## Ownership and boundaries
| Object | Authority | Allowed consumers | Forbidden duplicate |
| --- | --- | --- | --- |

## Contract matrix
<Core/Host → App Server → SDK → Web>

## Acceptance criteria
| ID | Given | When | Observable result | Falsifier | Evidence |
| --- | --- | --- | --- | --- | --- |

## Batches
1. <delete/replace old concept>
2. <stabilize authority and protocol>
3. <SDK and Web integration>
4. <scenario/eval and documentation>

## Six-question admission
<Answer all six questions from the PR template.>

## Budget and verification
expected: ...
actual: ...
<commands and results>

## Remaining risks
<Concrete files, boundaries, and next evidence.>
```
