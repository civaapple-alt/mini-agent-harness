# VS Code Coding Harness 经验与 mini-agent-harness 下一迭代准入笔记

Status: proposed

Source: [The Coding Harness Behind GitHub Copilot in VS Code](https://code.visualstudio.com/blogs/2026/05/15/agent-harnesses-github-copilot-vscode)

## 1. 结论摘要

VS Code 文章最重要的结论是：模型只是引擎，harness 才是把上下文、工具、执行循环和结果反馈组织成产品体验的系统。harness 至少负责四件事：组装模型可见上下文、声明可用工具、校验并执行工具调用、决定继续还是结束。

文章还明确区分了两个时间尺度：一次用户可见的 **turn** 可以包含很多次内部 **round**；每次 round 都重新组装最新上下文，执行工具并决定是否继续。工具调用上限、取消、stop hook、上下文压缩和持久化结果共同构成 loop-control，而不是模型自行负责这些约束。

对 mini-agent-harness 的直接启发是：下一迭代应继续投资可观察、可评估、可取消的 harness 边界，而不是单纯增加 provider、工具或配置数量。VS Code 的经验应作为设计假设和验证方法，不作为功能对齐目标。

## 2. 经验教训与本项目映射

| 文章经验 | mini-agent-harness 当前基础 | 下一迭代动作 | 保护条件 |
| :--- | :--- | :--- | :--- |
| Harness 决定模型看到什么 | Core 有 bounded context、历史、tool result 和 hard limits；Host 负责 prompt/world 组装 | 为每个高影响变更记录模型可见输入的增量、上限和来源；必要时增加脱敏的 round trace | 不允许无界上下文、单项超过既有 hard limit，或把 Host 内容未经审查直接注入 Core |
| Turn 与 round 要分开 | `Thread`/`Harness` 已有 turn 与 model step 边界；App Server 批量 drain queued turn | 在 eval 和事件分析中同时记录 user turn、turn 内 round/step、tool batch 和最终 settled result | 不改变 Core stop 分类、事件顺序或 Session 单一持久化权威 |
| 工具是动态能力面 | Capability profile、registry、MCP 和 approval 已由 Host/App Server 组合 | 评估“按 profile/provider/当前状态调整工具目录”是否有真实场景；先做 manifest/trace 证据，不先增加通用插件框架 | 工具目录、参数 schema、结果大小和权限仍有硬上限；公共协议变化必须单独评审 |
| 工具执行是 harness 责任 | Capabilities 执行工具，Host 处理 policy/persistence，App Server 负责请求与事件 | 对工具调用错误、超时、取消、重试和结果投影建立跨层场景矩阵 | 被动 observer 不改执行；非幂等外部效果不做隐式 replay |
| Loop-control 决定何时继续 | Core 有 step limit、steering、cancel；App Server 有 Actor/CAS 和 settled checkpoint | 为“继续/停止/取消/steer/timeout”建立统一生命周期证据，特别记录 active turn 到 durable state 的顺序 | 取消只能在安全边界生效；mutation 不得绕过 Actor/CAS；Session 只记录 settled 状态 |
| 上下文会增长，需要摘要 | Core 已有 bounded compaction 和 turn-atomic trimming | 将 compaction 前后模型可见内容、token/byte 预算和最近工具工作保留纳入 eval | 摘要不能重写历史、丢失最新世界状态或引入无界项 |
| 不同模型不应假设完全相同 | 当前主线是一个 Responses provider，Host 有稳定 system prompt 和 capability manifest | 只在两个真实 provider/model 行为确实分叉时引入命名的 provider policy；先用场景数据证明差异 | 不为猜测添加 provider-specific 分支、别名或第二套执行路径 |
| Benchmarks 要贴近产品工作流 | 当前有 CLI/App Server 公共路径测试，但缺少系统化 harness eval 记录 | 建立小型、可复现、容器/临时 workspace 隔离的 harness scenario 集，衡量正确率、effort、token、latency 和边界违规 | 不调用付费 provider；fixture、输出和模型可见输入必须 bounded；公共边界测试仍是最低证据 |
| Harness 改动应在合并前评估 | CI 已强制 fmt、Clippy、workspace tests 和 line budget；PR 模板已收集架构问题 | 将“影响 prompt/tool schema/loop/context/event 的变更”标记为需要 scenario/eval 证据；先人工准入，后续再考虑自动化 label | 不把 benchmark 分数替代协议、持久化和安全边界测试 |

## 3. 下一迭代建议

### 3.1 先做 Harness Scenario Evidence

先不添加新的通用 framework，建立一个最小场景集，覆盖：

1. 单文件读取/修改/测试；
2. 跨文件重构；
3. 工具失败后的恢复；
4. step limit、steer、cancel 和 timeout；
5. 多轮 follow-up 与 session resume；
6. Goal verifier 对 settled checkpoint 的判断；
7. MCP/approval/sandbox 的拒绝路径；
8. 无工具或受限 profile 下的退化行为。

每个场景至少保存：输入、允许的工具面、事件/turn trace 摘要、最终文件或 session 状态、是否越过边界，以及测试耗时。模型输出和工具结果使用现有上限，避免为了评测引入第二套运行时。

首轮只做基线，不设“模型分数达标”这一单一门槛；应同时观察 resolution、round/step 数、tool-call 数、token/byte 使用、latency、失败分类和边界违规。后续 harness 改动与基线进行同场景对比。

### 3.2 把 6 项准入问题变成验证记录

后续每个实践更新必须附下面的记录，不能只写“测试通过”：

```text
## Six-question validation

1. Layer: Core | Host | Capabilities | App Server | CLI — rationale:
2. Duplicate responsibility: existing paths/types searched:
3. Replace vs add: removed/replaced concept, or why addition is necessary:
4. Net line delta: runtime before -> after (delta); all Rust before -> after (delta):
5. Visible surface: model input / events / persistence / public protocol — bounded impact:
6. Boundary evidence: public tests, scenario/eval, Clippy, fmt, line budget:

Decision: accept | revise | defer
```

验证规则：

- 第 1、2、3 项必须给出代码路径或符号，不接受只有模块名称的回答；
- 第 4 项先写预计值，落地后补实际值，实际值以 `python scripts/line_budget.py` 为准；
- 第 5 项只要有一项为 yes，就必须说明上限、兼容性和回归证据；
- 第 6 项如果改变 prompt、tool schema、loop-control、context、event 或持久化行为，必须增加 Harness Scenario Evidence，公共单测不能单独作为充分证据；
- 任何一项无法回答时，状态只能是 `revise` 或 `defer`，不能直接进入实现。

### 3.3 评估自动化的顺序

自动化按以下顺序推进：

1. 先用 PR 模板人工填写并检查完整性；
2. 再让 CI 继续验证 fmt、Clippy、测试和两个 line ceilings；
3. 场景集稳定后，再为高影响变更增加 `requires-harness-eval` 标签或等价的 CI job；
4. 最后才考虑模型/provider 矩阵和长期趋势报告。

不要先做一个通用 benchmark platform。mini-agent-harness 当前最有价值的是验证自己的 turn、tool、context、state 和 boundary 语义。

## 4. 非目标与风险

- 不追求 VS Code 的工具数量、扩展生态、模型数量或 UI parity；
- 不把 stop hook、plugin、scheduler、memory 或 policy framework 提前加入 Core；
- 不为了取得更高 benchmark 分数放宽 hard limits、审批策略、Session 单一权威或公共协议兼容；
- 不把模型差异当作新增兼容 wrapper 的充分理由；必须先有可复现的行为差异和替代方案比较；
- 不把 scenario/eval 结果当作安全证明，安全和持久化仍需要确定性边界测试。

## 5. 下一迭代完成判定

下一迭代在以下条件全部满足后，才可把本 note 从 `proposed` 转为 `implemented`：

1. 至少 8 个 bounded harness scenarios 有稳定基线和可审计结果；
2. 至少一个影响模型可见面或 loop-control 的变更完成 6 项准入记录，并包含 scenario/eval 证据；
3. timeout、cancel、steer、follow-up、resume 的事件与 durable state 顺序没有回归；
4. runtime 和 all Rust 两个 hard ceilings 均通过，且变更没有删除受保护的 Core/Actor/CAS/Session 测试或权威；
5. README、CHANGELOG、Agent Notes 和 PR template 对实际流程保持一致。

