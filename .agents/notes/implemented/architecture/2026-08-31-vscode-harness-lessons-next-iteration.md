# Harness 经验、框架对比与下一迭代准入笔记

Status: implemented

Source: [The Coding Harness Behind GitHub Copilot in VS Code](https://code.visualstudio.com/blogs/2026/05/15/agent-harnesses-github-copilot-vscode)。本文同时合并原 2026-08-31 成熟度分析 note 的框架分层、Turn 流程、Codex 原生对照和成熟度结论。

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

## 附录 A：mini-agent-harness 与 Codex 原生框架对照

### A.1 定位与分层

共同的最小闭环是：

```text
用户输入 → 构造上下文 → 模型生成 → 判断工具调用
         → 执行工具并写回结果 → 再次请求模型 → 最终回复或停止
```

mini-agent-harness 的定位是小型、显式、可替换且可观察的 Agent 执行内核；
Codex 原生框架的定位是生产级、持久化、异步、事件驱动的 Agent Runtime。
前者适合验证 Harness 行为，后者还要整合 Session、Task、Turn、Item、沙箱、
MCP、审批、压缩、恢复、分叉和 App Server 生命周期。简化表达为：

```text
mini-agent-harness = model + bounded tool loop
Codex native        = durable Session/Task/Turn/Item + tool/event runtime
```

当前 mini-codex 的边界为：

```text
CLI / 客户端
    ↓
App Server（Actor、CAS/revision、事件与管理控制面）
    ↓
Host（runtime/profile/workflow 组合）
    ↓
Capabilities（provider、workspace、process、sandbox、MCP、approval）
    ↓
Core（Thread、Harness、Turn/Step、limits、control）
    ↓
Protocol（消息、工具、事件、停止原因和限制契约）
```

| 概念 | mini-agent-harness | Codex 原生框架 |
| --- | --- | --- |
| 会话 | `Thread`、`SessionState` 与 settled checkpoint | 持久化 `CodexThread`、`Session` 与 rollout |
| 工作单元 | `Thread::run_turn`；Turn 内有多个 Core Step | `RegularTask` 驱动的持久化 Turn |
| 一次模型步骤 | `Model::respond`，随后执行有界工具 batch | Responses stream、多个 output Item 与异步工具任务 |
| 工具路由 | Capabilities 执行，Host/App Server 组合 policy、MCP 和 approval | `ToolRouter`、沙箱、审批、MCP 与 StepContext 协同 |
| 上下文 | bounded `Message` 列表、压缩和 UTF-8 截断 | Turn/Step context、response item、rollout 和模型相关压缩 |
| 事件 | 稳定的 `Event`/`EventEnvelope` | Turn/Item lifecycle、delta、diff、MCP 和 raw response 事件 |
| 持久化 | settled checkpoint、Session JSONL、Result Store handle | thread store、rollout、response item 与恢复/分叉 |
| 控制 | safe checkpoint 上的 cancel、steer、follow-up | cancellation token、interrupt、mailbox 和 task 生命周期 |

### A.2 mini 的 Turn/Step 流程

一次用户可见 Turn 包含一个或多个模型 Step。每个 Step 都先检查控制状态，
再组装 bounded context，请求模型，验证响应，必要时执行完整工具 batch，最后
把工具结果写回下一轮上下文：

```text
turn/start
  ↓
App Server Actor 排队并分配 identity/revision
  ↓
Thread.begin_turn → Harness.run_with_control_mode
  ↓
追加 User Message，检查/压缩 context
  ↓
Model.respond
  ├─ 无工具调用 → 生成最终回复并结束
  └─ 有工具调用 → 校验数量 → 完整执行 bounded batch
                         ↓
                    追加 Tool Message / result handle
                         ↓
                    回到 context 组装和下一次 Model.respond
  ↓
TurnFinished → settled checkpoint / Session 持久化
  ↓
消费 queued steer/follow-up，必要时启动后续 Turn
```

关键不变量：响应和工具调用先验证，失败时不执行副作用；工具结果在进入模型
上下文前截断；连续重复 batch 只产生 bounded warning；cancel 和 steer 只在模型
步骤之间或完整工具 batch 后观察，不把工具副作用截断在中间状态。

### A.3 Codex 原生 Turn 流程

原生 Codex 的外部 Turn 通常经过以下持久化与异步协调路径：

```text
JSON-RPC turn/start
  ↓
turn processor → CodexThread::start_or_steer_turn
  ↓
Session::start_task → RegularTask
  ↓
TurnContext / hooks / MCP / AGENTS.md / skills / history
  ↓
Responses API stream
  ├─ 输出 Item/delta 并持久化
  ├─ ToolRouter 创建异步工具任务
  └─ stream 完成后 drain 工具 futures
  ↓
需要继续？
  ├─ 是：压缩或处理 pending input，再次 sampling
  └─ 否：执行 stop hooks，flush rollout，发送 turn-completed
```

因此 mini 的 Step 更容易逐项观察和测试；原生 Codex 的 Step 更像 provider
stream、Item 持久化与多个工具 future 的协调过程。两者共享“模型不是控制器”
这一原则，但原生框架承担了更多持久化和异步生命周期语义。

### A.4 Steering、Cancel 与外部 Turn 边界

mini Core 支持 `StopAtCheckpoint` 和 `ContinueSameTurn`，当前 App Server 主路径
主要采用前者：

```text
当前模型步骤完成 → 检查 cancel/steer → 结束当前 Core Turn
                  → 持久化 checkpoint → 取 queued steer/follow-up
                  → 必要时启动新的 Core Turn
```

所以 mini 的默认外部语义更接近“安全停止当前工作单元，再开始下一个”；一次
App Server `turn/start` 仍可能因 queued input 连续 settle 多个 Core Turn。

Codex 原生通常把 `turn/steer` 放入 Session input queue/mailbox，在同一个外部
Turn 内继续下一次 sampling：

```text
Turn 1: sampling → tool execution → steering input → sampling → Turn completed
```

这不是谁更“正确”，而是边界选择不同：mini 优先让 checkpoint 和测试边界显式，
原生 Codex 优先保持长任务的连续工作语义。任何变更都必须记录其对事件顺序、
持久化和取消确定性的影响，不能只比较最终文本。

### A.5 成熟度结论

成熟度应按“聚焦型 coding agent 的可靠运行基础设施”判断，而不是按工具数量
或模型数量判断：

| 维度 | mini-agent-harness 当前优势 | Codex 原生框架的优势与代价 |
| --- | --- | --- |
| 执行内核 | Loop、Step、limits、stop 分类和安全检查点显式，容易做确定性 fixture | 生命周期覆盖更完整，但 Task、Item、stream 和异步工具协调更复杂 |
| 状态与恢复 | App Server Actor/CAS、Session settled checkpoint 和单一路径边界清楚 | 持久化粒度、恢复/分叉和长任务能力更丰富，状态面更大 |
| 能力面 | Provider、workspace、sandbox、MCP 和 approval 在 Core 外组合 | 工具、hooks、skills、MCP、沙箱和环境集成更成熟，但兼容性矩阵更宽 |
| 可验证性 | bounded 输入/输出、被动事件和本地 mock 场景便于隔离验证 | 更接近生产工作流，需要更大的跨平台、真实 provider 和长期运行证据 |
| 主要风险 | 容易因追求小而漏掉真实 provider、平台和安全策略证据 | 容易因功能面扩大而增加隐式状态、异步竞态和上下文成本 |

结论是：mini 不应复制原生 Codex 的全部对象或工具生态；应继续保持
`CLI → App Server → Host → Core` 主路径，用 bounded scenario 验证每次改变对
Turn、Tool、Context、State 和 Boundary 的影响。只有在真实场景和证据都成立时，
才扩大 provider、retry 或 Docker policy，而不是用抽象数量代替成熟度。

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

#### 第一版 bounded scenario baseline（2026-08-31）

第一版复用了现有 CLI 公共集成测试，没有引入第二套 harness。每个测试都在临时 workspace 中运行，并使用本地 TCP mock provider；测试断言请求内容、工具目录、事件/输出或 durable session 状态。以下耗时是 Windows 本地单测命令的墙钟时间，包含 Cargo 增量检查/启动开销，只用于发现异常，不作为模型质量指标。

| 场景 | 现有公共测试 | 输入与允许面 | 可审计结果 | 耗时 |
| :--- | :--- | :--- | :--- | ---: |
| 上下文组装与受限 skill 摘要 | `ask_reads_stdin_and_keeps_machine_output_clean` | `summarize this repository`；允许 ask profile、world、workspace skill 摘要 | mock request 含 user/world/instructions，未泄漏完整 skill body；JSON 输出保持机器可读 | 4,184 ms |
| 无工具 profile 退化 | `ask_no_tools_uses_model_only_scope_without_extension_tools` | `explain the scope`；tools 必须为空，skill 不加载 | request `tools=[]`，capability profile 为 `ask-no-tools`，disabled 原因可见 | 527 ms |
| durable session resume | `durable_session_resumes_settled_history_after_restart` | `first question` → 重启 → `second question`；仅 settled session 可恢复 | 第二次 request 同时包含旧问题、旧答案和新问题，session turn-1/turn-2 均落盘 | 884 ms |
| Goal tool turn 与 verifier | `goal_mode_runs_a_tool_turn_and_verifies_the_settled_history` | `/goal Verify the release`；primary 可用工具，verifier 工具为空 | 7 个 mock request、verifier 输入含 tool evidence，goal 状态为 `converged` | 1,750 ms |
| timeout → interrupt → failed | `goal_mode_timeout_is_deterministic_and_keeps_repl_alive` | `/goal timeout fixture`；1 秒 deadline，mock provider 延迟 1.5 秒 | timeout 后 turn settled，goal state 为 `failed`，REPL 正常退出；无 `Busy` 状态残留 | 1,926 ms |
| restart recovery | `running_goal_is_paused_when_a_session_restarts` | 运行中的 Goal 被进程终止后 resume；不重放活动 turn | resume 后状态变为 `user_paused`，并提示重新发起 Goal | 760 ms |
| steer 安全检查点 | `steer_interrupts_a_running_turn_at_a_checkpoint` | active turn 中提交 `/steer focus on the actual bug` | 第一 turn 保存 `steered`，第二 request 使用新消息，session 记录 settled steered turn | 642 ms |
| follow-up 排队 | `follow_up_is_queued_until_the_running_turn_finishes` | active turn 中提交 `follow-up request`；只允许 bounded queue | 第一 request 结束后才发第二 request，输出包含 follow-up answer | 550 ms |

执行命令为：

```text
cargo test -p mini-agent-cli --test interactive <scenario> -- --exact
```

本批 8/8 通过。该第一版 baseline 已证明现有公共路径可承载基础 harness evidence；当时尚未包含跨文件重构、CLI 公共路径的工具失败恢复、MCP/approval/sandbox 拒绝和独立的 model/provider 对比，不将后续补充结果倒填为第一版基线。阶段 2/3 后续批次已分别补上部分边界证据，详见下文；当前仍未覆盖的项目以 Note 末尾清单为准。

同日回归也通过：`cargo test -p mini-agent-app-server` 为 28/28，
`cargo test -p mini-agent-cli --test interactive -- --test-threads=1` 为
12/12。App Server 测试覆盖 running turn 中的 cancel、steer、follow-up、
Actor/CAS、settled checkpoint 和 bounded JSONL Trace；CLI 场景覆盖真实
前端到 App Server 的公共路径。阶段 2 追加的结构化权限拒绝场景使 App Server
当前测试为 29/29。

#### 评审意见吸收与取舍（2026-08-31）

外部审查建议增加可导出的 Trace、故障注入 Provider、长场景调度、量化
Compaction、显式权限拒绝结果和 timeout/steer 并发证据。以下取舍作为下一
迭代的前置准入项；它们不会把尚未实现的能力写成当前基线。

| 评审建议 | 当前判断 | 下一迭代准入条件 |
| :--- | :--- | :--- |
| 每个 Round 导出 JSONL Trace 并记录哈希 | 已实现本地诊断 API；CLI 自动接入暂缓 | `mini_agent_app_server::JsonlTrace` 复用 App Server 事件，记录 `trace_id`、`turn_id`、`round_index`、事件类型、完整 bounded model-input 哈希、工具 manifest 哈希、字节计数和 payload 哈希；原始 prompt、工具参数和结果不写入。Stage 3 暂不自动写入 session 目录，也不重新引入已退役的外部 `--trace` 开关，避免扩大持久化和公共 CLI 语义。 |
| Fault Injection Provider | 首版已实现，限定为测试设施 | `mini-agent-core` 的 `FaultInjectionModel` 覆盖缺少必要工具参数、部分流失败和 `Retryable` 工具结果；`mini-agent-capabilities` 的 Responses 解析测试覆盖畸形 JSON、缺字段和 HTTP 429 的有界 API 错误映射。未增加隐式重试；provider 级 retry/backoff 策略仍单独待补，不增加生产 provider、不调用付费服务、不另起执行循环。 |
| 超过 5 秒的场景移出日常 CI | 部分接受 | 5 秒是 CI 调度策略，不是运行时语义；只有经确认的确定性慢场景才允许显式 `#[ignore]`，并提供定时或手动命令。不能按一次机器墙钟测量自动改变门禁。 |
| Compaction 在 70% 触发 | 暂不改变当前行为 | 当前实现和 `docs/limits.md` 的确定性触发点是最大上下文的 50%；下一场景在 70% 记录预警、最近 3 轮保留和压缩前后预算，只有证据证明 50% 不合适时才改阈值。 |
| 权限失败返回 `permission_denied`，不能是空结果 | 已用现有结构化状态验证 | 当前保留 `ToolExecutionStatus::NeedsApproval`，App Server 公共场景已断言事件、Session checkpoint 和下一轮模型输入均包含非空拒绝 reason；下一次审批/沙箱变更不得把该契约退化为空结果，再单独评估是否需要公共 `PermissionDenied` 变体。 |
| timeout 与 steer 并发时记录确定性优先级 | 接受 | 当前顺序明确为：Core 安全检查点先检查 cancel，再检查 steer；deadline 触发后 App Server 发送 interrupt、等待 `TurnFinished` 和 durable checkpoint，然后返回 timeout，不继续 drain 排队的 steer；普通已 settle batch 才按 steer 优先于 follow-up。修改该顺序前必须增加并发 race scenario。 |

当前 baseline 已有稳定的本地 JSONL Trace artifact；README 中的快捷命令仍只捕获
测试输出和预算快照，不会隐式生成 Trace。CLI 现在支持显式的 `ask --trace-jsonl PATH`
导出；Trace 只用于证明事件/输入摘要变化，不将 mock provider 的通过结果等同于真实
模型质量。

#### Stage 3 本轮实践的六项验证：CLI Trace contract audit

```text
1. Layer: CLI + App Server local boundary
   rationale: JsonlTrace 已属于 App Server 的本地诊断能力；CLI 只负责显式选择
   artifact sink，不复制事件、turn 或 Session 逻辑。
2. Duplicate responsibility:
   searched JsonlTrace/TraceRecord、CLI RunObserver/ChannelObserver、Session JSONL、
   offline trace replay 和历史 --trace 入口。现有事件 sink 可复用，但没有 CLI
   artifact lifecycle 或总量上限的公共契约。
3. Replace vs add:
   复用 JsonlTrace、现有 App Server turn API 和事件 sink；未来最多增加一个显式
   opt-in 的 CLI sink 配置，不增加第二条执行循环，不把 trace 写入 Session，也不
   恢复隐式 session trace 或未定义语义的旧 --trace 入口。
4. Net line delta:
   audit actual: runtime 16,216 -> 16,216 (+0); all Rust 29,537 -> 29,537 (+0).
   implementation estimate remains unapproved until the contract and offset are accepted.
5. Visible surface:
   accepted contract candidate is one explicitly requested artifact, initially limited
   to one-shot CLI paths; it must use a caller-provided path, refuse accidental overwrite,
   cap each record at 8 KiB and the complete artifact at a fixed total limit, and write
   only event metadata, counts, and hashes. Raw prompt, tool arguments/results, Session
   history, model input, public events, and JSON-RPC protocol remain unchanged.
6. Boundary evidence:
   existing App Server trace tests prove redaction and per-record bounds, but no CLI
   public-path test proves path ownership, total-size failure, or settled-session
   behavior. Add that CLI scenario before implementation is accepted.

Decision: defer implementation; accept the bounded contract direction for the next
design review.
```

At the time of this audit, automatic Trace export remained deferred; the existing
caller-owned `JsonlTrace` was the only supported diagnostic path.

The preceding audit is retained as the design-review checkpoint. Its deferred
automatic-export decision was superseded only for the explicit one-shot option below;
implicit Session or baseline-file export remains out of scope.

#### Stage 3 本轮实践的六项验证：bounded CLI Trace export

```text
1. Layer: CLI + App Server local boundary
   rationale: `ask` owns the explicit artifact path and combines the existing
   `RunObserver` with App Server `JsonlTrace`; Core and the event loop remain unchanged.
2. Duplicate responsibility:
   searched `mini-agent-cli/src/ask.rs`, `args.rs`, App Server `JsonlTrace`,
   `LocalAppServerClient::run_turn`, and Session JSONL. No other CLI-owned trace
   writer or event loop exists.
3. Replace vs add:
   reuse `JsonlTrace` and the existing observer path; add one passive composite
   observer and one `--trace-jsonl PATH` option. Do not restore the retired
   `--trace`, write into Session, create parent directories, or overwrite files.
4. Net line delta:
   expected: runtime +~30; all Rust +~270
   actual: runtime 16,216 -> 16,243 (+27); all Rust 29,560 -> 29,815 (+255).
5. Visible surface:
   adds one opt-in `ask` CLI argument and a caller-owned file artifact only. Each
   JSONL record is capped at 8 KiB and the artifact at 256 KiB; records contain
   event metadata, counts, and hashes, not prompt/tool payloads. No model input,
   event type, Session schema, or JSON-RPC field changed. Existing files and trace
   write/finalization failures fail the command.
6. Boundary evidence:
   `cargo test -p mini-agent-app-server` (31 passed),
   `cargo test -p mini-agent-cli --test interactive -- --test-threads=1`
   (15 passed), including success/redaction, total-limit, and overwrite-refusal
   scenarios; both affected-package Clippy checks, `cargo fmt --all`, and
   `python scripts/line_budget.py` pass.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：CLI public MCP timeout seam re-audit

```text
1. Layer: CLI public path over Capabilities/App Server
   rationale: the requested evidence must start from `mini-agent ask` and preserve
   the existing MCP tool result projection through the canonical runtime path.
2. Duplicate responsibility:
   searched `mini-agent-capabilities/src/mcp.rs`, CLI interactive MCP coverage,
   App Server MCP timeout projection, and the existing `McpTool::execute` wait.
   Capabilities has a 50ms `cfg(test)` protocol seam, but the CLI dependency is
   compiled without that test configuration and uses the 118-second production
   bound (with a separate 120-second reply wait).
3. Replace vs add:
   no bounded cross-layer seam currently exists. Do not add a production timeout
   setting, test-only runtime flag, alternate CLI transport, or second MCP loop
   solely to make this scenario fast; keep the existing capability/App Server
   evidence and defer the CLI projection.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,243 / 20,000; all Rust 29,815 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, CLI option, or public protocol change.
   A future seam must state timeout ownership, bounded failure text, cancellation,
   process cleanup, and compatibility before adding any surface.
6. Boundary evidence:
   existing Capabilities timeout and App Server public projection tests pass, while
   CLI interactive 15/15 has no fast MCP-timeout path. A CLI scenario is not accepted
   until a bounded seam can exercise the actual public transport without a 118-second
   wait or an external process kill.

Decision: defer
```

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

该记录已用于本轮 `goal timeout lifecycle` 实践，实际结果为 `accept`；它同时说明了为什么要在公共边界测试之外单独保留 bounded scenario evidence。

验证规则：

- 第 1、2、3 项必须给出代码路径或符号，不接受只有模块名称的回答；
- 第 4 项先写预计值，落地后补实际值，实际值以 `python scripts/line_budget.py` 为准；
- 第 5 项只要有一项为 yes，就必须说明上限、兼容性和回归证据；
- 第 6 项如果改变 prompt、tool schema、loop-control、context、event 或持久化行为，必须增加 Harness Scenario Evidence，公共单测不能单独作为充分证据；
- 任何一项无法回答时，状态只能是 `revise` 或 `defer`，不能直接进入实现。

#### 本轮实践的六项验证：goal timeout lifecycle

```text
1. Layer: App Server（CLI 仅负责调用）
   rationale: deadline-aware turn drain 属于 LocalAppServerClient 的服务边界；CLI 不直接操作 Thread、Session 或 workflow store。
2. Duplicate responsibility:
   searched LocalAppServerClient::run_turn_batch, LocalAppServerClient::interrupt,
   AppServer::turn_cancel_for, worker::handle_running_command,
   repl_worker::fail_active_goal；没有另一个已存在的 timeout-and-settle 路径。
3. Replace vs add:
   用 LocalAppServerClient::run_turn_batch_until 取代 CLI 外层可丢弃 future 的
   tokio::time::timeout；普通 run_turn_batch 复用同一 helper，没有新增第二套 turn loop。
4. Net line delta:
   expected: runtime +~50; all Rust +~50
   actual: runtime 15,236 -> 15,286 (+50); all Rust 28,255 -> 28,306 (+51)
5. Visible surface:
   no new model input, event type, persistence schema, or public protocol field;
   existing turn/interrupt is used, TurnFinished/checkpoint ordering is preserved,
   and only the existing goal state changes from running to failed after settlement.
   Concurrent behavior is deterministic: Core checks cancel before steer at a safe
   checkpoint; a deadline drains the interrupted turn and returns timeout without
   consuming a queued steer; an ordinary settled batch consumes steer before follow-up.
6. Boundary evidence:
   cargo test -p mini-agent-app-server (23 passed)
   cargo test -p mini-agent-cli --test interactive (11 passed)
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### 本轮实践的六项验证：bounded JSONL Trace

```text
1. Layer: Core + Protocol + App Server
   rationale: Core 在 ModelStarted 生成输入和工具 manifest 摘要；Protocol 只承载
   Rust 内部诊断字段并通过 serde(skip) 保持 wire shape；App Server 提供本地 JSONL sink。
2. Duplicate responsibility:
   searched EventSink/EventEnvelope、App Server LocalAppServerClient、Host RunObserver、
   session.jsonl 和历史 --trace/trace replay 路径；没有现行的 bounded round exporter。
3. Replace vs add:
   复用现有 EventEnvelope/EventSink 和 App Server turn drain；新增 JsonlTrace 只是
   被动事件 sink，不新增 event loop、Session authority、外部 --trace 开关或第二条执行路径。
4. Net line delta:
   expected: runtime +~380; all Rust +~380
   actual: runtime 15,286 -> 15,660 (+374); all Rust 28,306 -> 28,680 (+374)
5. Visible surface:
   no JSON-RPC wire shape or model-visible input changed. Existing ModelStarted carries
   Rust-only input_bytes/input_hash/tool_manifest_hash with serde(skip). The local trace
   exposes only bounded hashes/counts and event metadata; each record is capped at 8 KiB,
   and raw prompt/tool/result payloads are omitted. JsonlTrace/TraceRecord are a local
   App Server Rust API, not a public protocol method.
6. Boundary evidence:
   cargo test -p mini-agent-app-server (28 passed)
   cargo test -p mini-agent-core (28 passed)
   cargo test -p mini-agent-protocol (7 passed)
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### Stage 3 历史审计：CLI automatic Trace（实现前）

```text
1. Layer: CLI + App Server boundary audit
   rationale: the existing `JsonlTrace` is an App Server local diagnostic sink;
   automatic CLI use would decide artifact ownership and lifecycle, not add a new
   Core observation mechanism.
2. Duplicate responsibility:
   searched `mini-agent-cli/src/ask.rs`, `repl_worker/prompt.rs`, the App Server
   `LocalAppServerClient`/`JsonlTrace`, session JSONL storage, and the baseline report
   recipe. No existing CLI-owned bounded trace file or stable lifecycle contract exists.
3. Replace vs add:
   keep the existing caller-provided `JsonlTrace` and manual baseline recipe. Do not
   write an implicit file into the Session directory and do not restore the retired
   external `--trace` option; either would add ownership or public CLI semantics before
   the artifact contract is settled.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, or protocol change. The deferred choice
   preserves the bounded, redacted local trace and keeps Session as the sole durable
   history authority.
6. Boundary evidence:
   App Server `local_client_exports_bounded_redacted_trace` and the existing Trace tests
   prove the sink contract; the README baseline recipe proves CLI output and budget
   capture but intentionally does not claim automatic Trace export. A future CLI trace
   change must specify artifact path, retention, redaction, failure handling, and opt-in
   semantics before implementation.

Decision: defer（已由上方 bounded CLI Trace export 实践记录取代）
```

#### Stage 3 本轮实践的六项验证：bounded HTTP 429 classification

```text
1. Layer: Capabilities
   rationale: `crates/mini-agent-capabilities/src/openai/mod.rs::post_json` 是 Responses
   provider 的 HTTP 状态边界；本轮只验证 adapter 对 429 的有界错误分类，不改变 Core、
   Host、App Server 或 CLI 的执行语义。
2. Duplicate responsibility:
   searched `post_json`, `OpenAiModel::respond`、Responses SSE tests 和现有 provider
   error tests；没有现有测试验证 429 status、bounded error body 和 no implicit retry
   的组合契约。
3. Replace vs add:
   复用现有 `post_json`、`OpenAiError::Api` 和 `MAX_ERROR_BODY_BYTES`；只增加一个
   test-only loopback HTTP fixture，避免新增 retry loop、provider wrapper 或第二套请求路径。
4. Net line delta:
   expected: runtime +0; all Rust +~40
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,288 -> 29,327 (+39)
   （Capabilities unit-test net +39，包含 fmt 调整）。
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. HTTP 429 remains
   a bounded `OpenAiError::Api { status: 429, message }`; the body is capped by the existing
   4 KiB limit, and the adapter does not retry implicitly. Provider-specific backoff remains
   an explicit future policy decision.
6. Boundary evidence:
   cargo test -p mini-agent-capabilities (63 passed)
   cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### Stage 3 本轮实践的六项验证：HTTP 429 retry/backoff policy audit

```text
1. Layer: Capabilities provider boundary
   rationale: `post_json` owns the HTTP status classification. A retry policy would
   also affect model request count, latency, cost, and turn settlement, so it must be
   decided at the provider boundary before changing Core or App Server.
2. Duplicate responsibility:
   searched `post_json`, `OpenAiModel::respond`, Core model-error handling, and the
   existing 429 loopback test. There is no existing retry scheduler, backoff helper, or
   `Retry-After` contract to reuse.
3. Replace vs add:
   retain the existing bounded `OpenAiError::Api` fail-fast path. Do not add an implicit
   retry loop, provider wrapper, or configuration field until the retry count, jitter,
   `Retry-After` cap, cancellation behavior, and user-visible semantics are specified.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. One 429 remains
   one bounded model failure; this avoids hidden extra provider requests and preserves
   deterministic turn failure under the current contract.
6. Boundary evidence:
   `maps_http_429_to_bounded_api_error_without_retrying` passes in the Capabilities
   provider tests, with the existing bounded body assertion and no second request.
   `cargo test -p mini-agent-capabilities` and Clippy evidence are recorded above;
   `python scripts/line_budget.py` remains under both ceilings. Retry/backoff behavior
   is explicitly deferred pending a separate policy decision.

Decision: accept current fail-fast policy; defer retry/backoff implementation
```

#### Stage 3 本轮实践的六项验证：bounded cross-file refactor

```text
1. Layer: CLI（通过现有 App Server/Host/Core 主路径验证）
   rationale: 场景从 `mini-agent ask --json --auto-approve` 进入，连续读取并编辑两个
   workspace 文件，验证 CLI 公共入口承载跨文件上下文与 settled 文件结果，不调用私有
   Core/Host API。
2. Duplicate responsibility:
   searched `crates/mini-agent-cli/tests/interactive.rs` 的现有 ask、read/write、
   approval 和 unknown-tool 场景，以及 Capabilities workspace tool tests；没有现有
   公共场景同时验证两个文件的读取结果进入后续模型 round 并完成两个精确编辑。
3. Replace vs add:
   复用现有临时 workspace、TCP Mock Provider、SSE function-call helper 和真实
   `read_file`/`edit_file` 工具；只增加一个 bounded scenario，不增加生产重构器、文件
   批处理 API、备用 CLI 路径或第二套 Harness loop。
4. Net line delta:
   expected: runtime +0; all Rust +~100
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,203 -> 29,288 (+85)
   （CLI integration test net +85）。
5. Visible surface:
   no production model input, event type, persistence schema, or public protocol change;
   the provider receives only the existing bounded tool results and the scenario asserts the
   two final file contents. `--auto-approve` is an existing test invocation option; no new
   permission behavior is introduced.
6. Boundary evidence:
   cargo test -p mini-agent-cli --test interactive -- --test-threads=1 (13 passed)
   cargo clippy -p mini-agent-cli --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### Stage 3 本轮实践的六项验证：explicit MCP/sandbox refusal

```text
1. Layer: Capabilities
   rationale: `mcp::load` 是 MCP server admission 边界，`workspace::Shell::execute`
   是 shell policy 与 ProcessSandbox 之间的边界；本轮只验证拒绝结果和副作用，不改变
   Core、Host、App Server 或 CLI 的执行路径。
2. Duplicate responsibility:
   searched `mcp_tests::approval_denial_prevents_server_start_and_data_creation`,
   `workspace_tests::read_image_outside_workspace_can_be_denied`, the CLI non-TTY shell
   scenario, and `Shell::execute_outcome`; MCP 的拒绝断言原先只有模糊文本匹配，sandbox
   前的 shell 拒绝没有直接断言 structured status 和无副作用。
3. Replace vs add:
   保留现有 `ApprovalController`、`ToolExecutionOutcome`、MCP diagnostics 和 sandbox
   construction；只收紧 MCP 的精确诊断断言，并增加一个 test-only shell marker fixture，
   不新增 `PermissionDenied` 状态、审批路径或第二套 sandbox 执行器。
4. Net line delta:
   expected: runtime +0; all Rust +~40
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,327 -> 29,360 (+33)
   （Capabilities unit-test net +33）。
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. A denied MCP
   connection produces no exposed tool, no loaded server, no plugin-data directory, and one
   explicit bounded diagnostic. A denied shell produces the existing structured `Failed`
   outcome with a non-empty reason before `run_shell`/ProcessSandbox is reached; Docker
   availability and container-isolation semantics remain separate evidence gaps.
6. Boundary evidence:
   cargo test -p mini-agent-capabilities (64 passed)
   cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### Stage 3 初次审计结论（daemon unavailable）：Docker sandbox availability/isolation

```text
1. Layer: Capabilities
   rationale: `workspace::run_shell` owns Docker preflight and process execution; this audit
   checks the existing sandbox boundary without changing its runtime contract.
2. Duplicate responsibility:
   searched `workspace_tests::docker_sandbox_checks_availability_or_reports_clear_error` and
   `workspace::shell::run_shell`; there is no deterministic cross-platform Docker daemon or
   container-isolation fixture in the repository.
3. Replace vs add:
   保留现有 Docker smoke test，不添加依赖本机 daemon、镜像缓存或宿主机文件布局的测试，
   也不增加新的 sandbox abstraction。`docker --version` 只能证明 CLI 存在。
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The existing smoke
   test accepts either the explicit preflight-unavailable error or a bounded command result;
   a missing Docker API can appear as an exit-1 stderr result, so this is not isolation proof.
6. Boundary evidence:
   `docker --version` reports Docker 29.6.1 on this Windows host, but `docker info` cannot
   connect to `dockerDesktopLinuxEngine` because the daemon pipe is absent. The existing
   `workspace::tests::docker_sandbox_checks_availability_or_reports_clear_error` test passed
   through its clear-error branch. `python scripts/line_budget.py` passed. Cross-platform
   daemon availability and container isolation were deferred at that time until a controlled
   test seam or CI capability became available.

Decision: defer
```

#### Stage 3 复审：Docker runtime mount and ephemeral filesystem evidence

```text
1. Layer: Capabilities
   rationale: `workspace::run_shell` owns Docker command construction and process execution;
   the new scenario exercises that real boundary after a daemon became available.
2. Duplicate responsibility:
   searched `workspace::shell::run_shell`, `ProcessSandbox`, and the existing Docker
   availability test. The existing smoke test checks command/preflight behavior; the new
   probe adds only the missing runtime mount and container-filesystem observation.
3. Replace vs add:
   keep the existing availability test and add one bounded probe in the same test module;
   do not add a second sandbox abstraction, Docker daemon wrapper, or environment-specific
   fixture. The probe skips only through the existing clear unavailable error when a host
   cannot provide Docker.
4. Net line delta:
   expected: runtime +0; all Rust +~25 test lines
   actual: runtime 16,216 -> 16,216 (+0); all Rust 29,537 -> 29,560 (+23).
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The probe proves
   the selected workspace is mounted at `/workspace`, writes are visible in that workspace,
   and a `/tmp` container file is not written into the workspace. It does not claim network,
   Linux capability, privilege, or resource isolation because the production `docker run`
   command does not set those policies.
6. Boundary evidence:
   `docker --version` and `docker info` report Docker Desktop Linux/server 29.6.1 with
   `runc`, seccomp, and cgroupns; `docker image inspect alpine` succeeds. The new
   `workspace::tests::docker_sandbox_mounts_workspace_and_keeps_container_tmp_ephemeral`
   test passes, and `cargo test -p mini-agent-capabilities` is 65/65. Clippy, fmt, and
   `python scripts/line_budget.py` pass. Stronger isolation remains a separate policy item.

Decision: accept partial runtime evidence; defer stronger isolation policy.
```

#### Stage 3 本轮实践的六项验证：Docker daemon preflight correction

```text
1. Layer: Capabilities
   rationale: `workspace::run_shell` owns the Docker availability preflight before
   constructing the container command.
2. Duplicate responsibility:
   searched the existing `docker` check and `run_sandboxed_command`; no second
   daemon probe or wrapper was introduced.
3. Replace vs add:
   replace the CLI-only `docker --version` check with `docker info --format
   {{.ServerVersion}}`, so the existing fail-closed diagnostic verifies the daemon
   rather than merely the executable. The container command and sandbox policy
   remain unchanged.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 -> 16,216 (+0); all Rust 29,560 -> 29,560 (+0).
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. An
   unavailable Docker daemon now fails before spawning `docker run` with the
   existing actionable error; native sandbox behavior is unchanged.
6. Boundary evidence:
   `cargo test -p mini-agent-capabilities` is 65/65, including the real Docker
   mount probe; `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`,
   `cargo fmt --all`, and `python scripts/line_budget.py` pass. `docker info` succeeds
   on this host. Stronger network/capability/resource isolation remains deferred.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：model/provider comparison audit

```text
1. Layer: Capabilities + Host composition boundary
   rationale: model construction is selected by Capabilities and assembled by Host;
   a comparison must distinguish provider transport behavior from Core turn behavior.
2. Duplicate responsibility:
   searched `CapabilityRegistry`, `build_model`, `ModelProviderFactory`, the external
   model-provider example, and the existing CLI/App Server mock-provider scenarios.
   The registry has one built-in OpenAI-compatible provider; the factory seam does not
   provide a second behavioral implementation or a comparable quality dataset.
3. Replace vs add:
   do not add a provider-specific branch, compatibility wrapper, benchmark platform, or
   paid-provider test. Keep the existing provider-selection seam and require a concrete
   reproducible behavior fork before adding a named provider policy.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. Mock provider
   results remain harness-path evidence only and are not promoted to model-quality claims.
6. Boundary evidence:
   existing Host/Capabilities registry and factory tests, the bounded CLI/App Server mock
   scenarios, `cargo test -p mini-agent-host`, and the no-paid-provider release rule show
   composition coverage but not cross-provider behavior. A future comparison must define
   fixed scenarios, model IDs, request/response capture, cost/latency bounds, and privacy
   handling before it becomes a CI or release gate.

Decision: defer
```

#### Stage 3 本轮实践的六项验证：MCP call denial

```text
1. Layer: Capabilities
   rationale: `mcp::McpTool::execute` 是已加载 MCP tool 的调用授权边界；本轮沿用
   `loads_and_calls_stdio_server_through_rmcp` 的真实 RMCP/stdio 组装，只验证调用前拒绝。
2. Duplicate responsibility:
   searched the existing MCP load/call fixture and CLI/App Server approval scenarios；原有
   MCP fixture只验证成功调用，连接拒绝与通用敏感工具拒绝不能证明真实 `McpTool` 调用门禁。
3. Replace vs add:
   复用同一 stdio server、tool 和 `ApprovalController`，连接完成后切换为 Interactive
   拒绝；不复制 fixture，不新增 MCP proxy、重试循环或公共拒绝类型。
4. Net line delta:
   expected: runtime +0; all Rust +~15
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,360 -> 29,369 (+9)
   （Capabilities unit-test net +9）。
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The existing
   `McpTool::execute_outcome` maps the denied call to structured `Failed` with a non-empty
   reason before it sends `ServerCommand::Call`; successful MCP calls remain covered by the
   same fixture.
6. Boundary evidence:
   cargo test -p mini-agent-capabilities (64 passed)
   cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### 阶段 2 本轮实践的六项验证：structured approval denial

```text
1. Layer: App Server（通过现有 Core Thread/Harness 主路径验证）
   rationale: 场景调用公开的 `AppServer::turn_start_for`、事件订阅和
   `thread_read_for`；App Server 负责把 Core 的结构化拒绝事件与 settled checkpoint
   交给客户端，不新增 Host 或 CLI 私有旁路。
2. Duplicate responsibility:
   searched Core 的 ApprovalTool/structured outcome test、Capabilities 的 approval
   callback tests、CLI 的 non-interactive denial scenario 和 App Server 的
   ApprovalBroker test；没有现有 App Server 场景同时断言 ToolFinished、Session 和
   后续模型输入都保留非空拒绝 reason。
3. Replace vs add:
   复用现有 `ToolExecutionStatus::NeedsApproval`、Event/Session 投影和 App Server
   worker；仅增加一个 test-only model/tool 场景，不新增 `PermissionDenied` 枚举、事件、
   persistence schema、审批 callback 或第二套执行路径。
4. Net line delta:
   expected: runtime +~140; all Rust +~140
   actual: runtime 15,928 -> 16,074 (+146); all Rust 29,057 -> 29,203 (+146)
   （App Server unit-test net +146）。
5. Visible surface:
   no production model input, public protocol, event type, or persistence schema changed;
   the existing `NeedsApproval` status and bounded non-empty reason are visible only through
   existing ToolFinished, Message::Tool, and the next model request. The test also proves the
   settled answer completes after the denial instead of treating it as an absent tool result.
6. Boundary evidence:
   cargo test -p mini-agent-app-server (29 passed)
   cargo clippy -p mini-agent-app-server --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### 阶段 2 本轮实践的六项验证：CLI public-path tool recovery

```text
1. Layer: CLI（通过现有 App Server/Host/Core 主路径验证）
   rationale: 新场景从 `mini-agent ask --json` 进入，不调用 Core 或 Host 私有接口；
   Mock Provider 返回未知工具调用，验证 CLI 公共入口能把 Core 的 bounded tool failure
   送回下一轮模型输入并得到 settled answer。
2. Duplicate responsibility:
   searched `crates/mini-agent-cli/tests/interactive.rs` 的现有 ask、tool、approval
   场景，以及 Core 的 unknown-tool recovery test；没有现有 CLI 公共路径场景同时断言
   未知工具结果进入下一轮 provider request。
3. Replace vs add:
   复用既有 `mini_agent`、临时 workspace、TCP Mock Provider 和 SSE helper；只增加一个
   public-path scenario，并将固定 shell response helper 委托给通用 function-call helper，
   不增加生产恢复逻辑、备用 CLI 路径或第二套 Harness loop。
4. Net line delta:
   expected: runtime +0; all Rust +~80
   actual: runtime 15,928 -> 15,928 (+0); all Rust 28,992 -> 29,057 (+65)
   （CLI integration test net +65）。
5. Visible surface:
   no production model input, event type, persistence schema, or public protocol change;
   the test only verifies the existing bounded `unknown tool: missing_fixture` Tool result
   is projected into the next provider request before the final answer is settled.
6. Boundary evidence:
   cargo test -p mini-agent-cli --test interactive -- --test-threads=1 (12 passed)
   cargo clippy -p mini-agent-cli --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### 本轮实践的六项验证：test-only fault injection

```text
1. Layer: Core + Capabilities
   rationale: Core 的 FaultInjectionModel 验证现有 Model/ModelEventSink、Tool 和
   Harness 边界；Capabilities 的 Responses 测试验证原始 provider 事件解析。没有修改
   Protocol、Host、App Server 或 CLI 生产路径。
2. Duplicate responsibility:
   searched mini-agent-core/src/harness_tests.rs 的 ScriptedModel/RecordingModel、
   mini-agent-capabilities/src/openai/responses.rs 的 Accumulator/apply/parse_tool_call；
   现有 double 只提供正常响应，现有解析测试没有覆盖脏 function-call 事件，没有可复用
   的故障序列设施。
3. Replace vs add:
   保留正常测试 double，新增隔离的 fault_injection_tests.rs 和一个可排队的
   FaultInjectionModel；它只复用既有 Model、ModelEventSink、Tool 和 Harness loop，
   不新增生产 provider、重试策略或第二套执行循环。
4. Net line delta:
   expected: runtime +~320; all Rust +~320
   actual: runtime 15,660 -> 15,928 (+268); all Rust 28,680 -> 28,992 (+312)
   （Core test-only +268，Capabilities parser tests +44）。
5. Visible surface:
   no production model input, event type, persistence schema, or public protocol change;
   partial text is emitted only through the existing ModelEventSink and is followed by the
   existing RunFailed(Model). Malformed provider JSON is rejected at the existing adapter
   boundary; missing tool arguments remain a bounded existing Tool result visible to the
   next model round.
6. Boundary evidence:
   cargo test -p mini-agent-core (31 passed)
   cargo test -p mini-agent-capabilities (62 passed)
   cargo clippy -p mini-agent-core --all-targets -- -D warnings
   cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

### 3.3 Tool failure/timeout/retry evidence matrix

本轮审计将已有证据与未覆盖语义分开，避免把单层 unit test 或一次 smoke test
误写成完整 Harness Scenario：

| Fault class | Existing evidence | Status | Remaining gap |
| :--- | :--- | :--- | :--- |
| 缺少必要工具参数 | Core `missing_required_tool_argument_is_projected_for_model_recovery` | covered | 无；继续保持下一轮可见的 bounded Tool result |
| 未知工具恢复 | CLI `ask_recovers_from_unknown_tool_on_public_path` | covered | 无；更广泛 provider 脏参数仍未穷举 |
| 模型部分流失败 | Core `partial_model_stream_is_failed_without_fabricating_completion` | covered | 无；真实 provider 截断仍需独立矩阵 |
| Retryable 工具结果 | Core `retryable_tool_result_is_preserved_until_model_recovers` | covered | 无隐式重试；策略层仍 deferred |
| HTTP 429 | Capabilities `maps_http_429_to_bounded_api_error_without_retrying` | covered: bounded fail-fast default | provider-specific retry/backoff policy remains deferred |
| shell timeout | Capabilities `shell_process_has_a_timeout` | covered at capability boundary | CLI/App Server 公共路径尚未单独覆盖 |
| turn/goal timeout | App Server/CLI goal timeout scenario | covered at public path | 与 tool timeout 的组合矩阵未覆盖 |
| approval/MCP refusal | App Server `NeedsApproval`、Capabilities MCP connection/call denial | covered | 无；公共 event projection 仍沿用既有边界 |
| MCP call timeout | Capabilities `loads_and_calls_stdio_server_through_rmcp` with a controlled slow call; App Server `projects_mcp_timeout_through_public_app_server` | covered at capability and App Server public boundary | CLI public command with actual MCP transport is deferred: the bounded test seam is not available across the dependency boundary |
| MCP circuit breaker | Capabilities `circuit_breaker_trips_after_failures_and_recovers` | unit-only | 真实 MCP call failure 到 model round 的公共路径未覆盖 |
| Docker sandbox | availability smoke test + runtime mount/ephemeral filesystem probe | covered: host runtime path | network/capability/resource isolation still needs an explicit policy and cross-platform evidence |

#### Stage 3 本轮实践的六项验证：MCP call timeout

```text
1. Layer: Capabilities
   rationale: `mcp::run_server` 的 `PROTOCOL_CALL_TIMEOUT` 是 MCP provider call 的
   bounded execution boundary；测试沿用真实 `McpTool`/RMCP/stdio 组装，不改变 App Server
   或 CLI 的公共协议。
2. Duplicate responsibility:
   searched the existing stdio MCP success/refusal fixture, circuit-breaker unit test, and
   `McpTool::execute`;原先只有固定 118 秒生产常量，没有可在测试中快速证明 timeout result
   的实际 provider call。
3. Replace vs add:
   复用已有 stdio fixture，增加一个受控 slow call；仅用 `cfg(test)` 将等待缩短为 50ms，
   生产仍为 118 秒，不新增 retry loop、timeout wrapper 或第二套 MCP client。
4. Net line delta:
   expected: runtime +0; all Rust +~15
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,369 -> 29,378 (+9)
   （Capabilities production/test-only timeout seam and unit fixture net +9）。
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The existing
   `McpTool::execute_outcome` returns structured `Failed` with the bounded reason
   `MCP tool call timed out`; production timeout behavior remains unchanged. App Server
   projection is covered separately, while CLI transport projection remains deferred.
6. Boundary evidence:
   cargo test -p mini-agent-capabilities (64 passed)
   cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
   cargo fmt --all
   python scripts/line_budget.py

Decision: accept
```

#### Stage 3 本轮实践的六项验证：App Server MCP timeout projection

```text
1. Layer: App Server
   rationale: the App Server worker owns public event fan-out, checkpoint settlement,
   and the next model round; it must preserve the structured failure produced by the
   Capabilities MCP boundary without adding another timeout policy.
2. Duplicate responsibility:
   searched the existing `NeedsApproval` public projection test, Core tool-batch
   execution, and Capabilities `McpTool` timeout evidence; no App Server test previously
   asserted a timeout result across events, checkpoint, and the next model request.
3. Replace vs add:
   reuse the existing `ToolExecutionOutcome` and App Server worker path; add only a
   deterministic timeout-shaped fixture and public boundary scenario. No production
   timeout wrapper, retry loop, protocol field, or second MCP path is introduced.
4. Net line delta:
   expected: runtime +~142; all Rust +~142
   actual: runtime +142; all Rust +142 in the changed Rust test file. Current budget
   snapshot is runtime 16,216 / 20,000 and all Rust 29,537 / 30,000.
5. Visible surface:
   no model input, event, persistence schema, or public protocol shape is added. The
   scenario verifies the existing `Failed` status and bounded `MCP tool call timed out`
   reason survive `ToolFinished`, the durable checkpoint, and the next model round.
6. Boundary evidence:
   `cargo test -p mini-agent-app-server` (30 passed); `cargo clippy -p
   mini-agent-app-server --all-targets -- -D warnings`; `cargo fmt --all`; and
   `python scripts/line_budget.py` all pass. The actual RMCP timeout source remains
   covered separately by the Capabilities test; CLI transport projection remains open.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：CLI MCP timeout projection audit

```text
1. Layer: CLI + Capabilities boundary audit
   rationale: the requested evidence must run through CLI configuration, Host assembly,
   the real MCP stdio tool, and the existing App Server turn path; no model or Core
   behavior needs to change.
2. Duplicate responsibility:
   searched the CLI interactive MCP/configuration path, Host `prepare_harness` assembly,
   Capabilities `PROTOCOL_CALL_TIMEOUT`, and the App Server public projection scenario.
   Capabilities already has the real RMCP timeout and App Server already has the public
   outcome projection; only their CLI process-level composition is missing.
3. Replace vs add:
   no implementation is added in this audit. A CLI fixture cannot reuse the Capabilities
   unit-only 50ms `cfg(test)` constant because dependencies are compiled without that cfg;
   adding a production timeout option or a feature solely for this scenario would expand
   configuration/API surface without a settled user requirement.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; no Rust files changed.
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The decision
   keeps the 118-second production MCP call bound and avoids a test-only configuration
   contract leaking into the CLI.
6. Boundary evidence:
   Capabilities `loads_and_calls_stdio_server_through_rmcp` proves the real bounded timeout;
   App Server `projects_mcp_timeout_through_public_app_server` proves event/checkpoint/next-
   round projection; `cargo test -p mini-agent-app-server` (30 passed), its Clippy check,
   `cargo fmt --all`, and `python scripts/line_budget.py` pass. A real CLI transport test is
   deferred until a bounded, justified injection seam exists.

Decision: defer
```

#### Stage 3 本轮实践的六项验证：failure matrix audit

```text
1. Layer: Core + Capabilities + App Server + CLI
   rationale: 这是对既有跨层证据的 inventory，不引入新的执行逻辑；每个 fault class
   仍由拥有其语义的边界负责。
2. Duplicate responsibility:
   searched the existing Core fault-injection tests, Responses parser/429 tests, MCP and
   workspace tests, App Server approval/timeout tests, and CLI interactive scenarios；矩阵
   标出了 unit-only 与 public-path evidence，未把它们重复实现为新 fixture。
3. Replace vs add:
   只整理和分类现有测试与已记录结果，不新增生产代码、测试 double、重试策略或第二套
   Harness loop；缺口保留为后续独立准入项。
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,369 -> 29,369 (+0)
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The audit explicitly
   preserves the distinction between bounded adapter errors, structured tool outcomes, public
   turn settlement, and unproven provider/container behavior.
6. Boundary evidence:
   existing recorded Core/App Server/CLI evidence in this note; current
   `cargo test -p mini-agent-capabilities` (64 passed), the Docker smoke test, and
   `python scripts/line_budget.py` all pass. No full workspace test was added or run.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：Compaction trigger and retention evidence

```text
1. Layer: Core
   rationale: compaction threshold, turn-atomic trimming, and recent-tail retention
   are Core context semantics; no Host, CLI, provider, or persistence path changes.
2. Duplicate responsibility:
   searched `Harness::prepare_context`, `compact_context`, `split_prefix_tail`,
   `trim_prefix_to_fit`, `docs/limits.md`, and the existing Core harness tests.
   The implementation already has one 50% trigger and one last-two-groups retention
   rule; this batch adds no second policy.
3. Replace vs add:
   strengthen the existing `compacts_context_and_continues_the_tool_loop` scenario
   with a measurable 70%-of-test-budget precondition, while retaining the existing
   `split_prefix_tail_keeps_last_two_assistant_groups` behavioral assertion. No
   runtime threshold, context structure, or compaction path is added.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 -> 16,216 (+0); all Rust 29,537 -> 29,537 (+0).
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The test
   records that compaction occurs after the context reaches at least 70% of its
   bounded 2,000-byte fixture budget; production remains triggered at 50%.
6. Boundary evidence:
   `cargo test -p mini-agent-core` (31 passed), `cargo clippy -p mini-agent-core
   --all-targets -- -D warnings`, `cargo fmt --all`, and `python scripts/line_budget.py`
   pass. The existing Core test also proves the compaction result is smaller and
   the latest world-state/tool work remains available; no public path changed.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：PR admission mechanical check

```text
1. Layer: N/A（repository process/CI）
   rationale: `scripts/check_pr_admission.py` 只验证 PR 描述的准入记录是否完整，
   不进入 Core、Host、Capabilities、App Server 或 CLI runtime。
2. Duplicate responsibility:
   searched `.github/pull_request_template.md`, `AGENTS.md` and `.github/workflows/ci.yml`；
   原有模板要求人工填写，但没有检查 placeholder、六题回答和六个确认框的自动门禁。
3. Replace vs add:
   保留现有模板和 reviewer 判断，新增一个无副作用的 Python completion check 与单测，
   不新增 Rust helper、runtime hook 或第二套质量门禁。
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,074 -> 16,074 (+0); all Rust 29,378 -> 29,378 (+0)
5. Visible surface:
   no model input, event, persistence schema, or public protocol change. The pull-request job
   reads only `github.event.pull_request.body` and validates completion; content quality remains
   a review responsibility. Push, dispatch, and local runtime paths are unchanged.
6. Boundary evidence:
   `python -m unittest scripts/test_pr_admission.py scripts/test_line_budget.py
   scripts/test_package_release.py` (13 passed); CI also runs the new check only for
   `pull_request` events. `git diff --check` and the Rust budget remain clean.

Decision: accept
```

#### Stage 3 本轮实践的六项验证：PR admission confirmation scope

```text
1. Layer: N/A（repository process/CI）
   rationale: the change tightens the existing PR description validator and does not
   enter any Core, Host, Capabilities, App Server, or CLI runtime path.
2. Duplicate responsibility:
   searched `scripts/check_pr_admission.py`, its maintenance tests, the PR template,
   and the workflow job. The previous checker counted checked boxes in the whole PR
   body, so unrelated checklists could satisfy an unchecked admission box.
3. Replace vs add:
   replace the global checkbox count with a bounded scan of the existing `准入确认`
   section that matches each of the six stable confirmation labels, and add one
   regression case. No second validator, Rust helper, or new gate is introduced.
4. Net line delta:
   expected: runtime +0; all Rust +0
   actual: runtime 16,216 / 20,000; all Rust 29,537 / 30,000; Python-only change.
5. Visible surface:
   no model input, event, persistence, public protocol, or CLI behavior changes. The
   CI admission decision becomes stricter only when the six designated boxes are not
   checked; unrelated PR checklists no longer affect it.
6. Boundary evidence:
   `python -m unittest scripts/test_pr_admission.py scripts/test_line_budget.py
   scripts/test_package_release.py` (15 passed), `git diff --check`, and
   `python scripts/line_budget.py` pass. The new regression proves an unrelated
   checked box cannot mask an unchecked admission confirmation.

Decision: accept
```

#### 阶段 1 本轮实践的六项验证：REPL 核心能力边界

```text
1. Layer: CLI（通过现有 App Server/Host/Core 主路径执行）
   rationale: 本批只收缩 `mini-agent-cli` 的交互适配层；Turn、工具、审批、
   run control 和 Session 仍通过 App Server 公共路径运行，Plan/Goal 的状态机仍由
   App Server workflow API 所有。
2. Duplicate responsibility:
   searched `repl.rs`, `repl_worker.rs`, `repl_worker/prompt.rs`, the App Server
   workflow client methods, and Studio/SDK-facing workflow contracts. REPL 原先重复
   编排 Plan prompt、Goal verifier、timeout/advance 和 restart pause；这些不应再由
   终端参考客户端复制。
3. Replace vs add:
   删除 REPL 的 `/plan`、`/goal` 解析、WorkerCommand 分支和 Goal verifier loop；
   不新增替代 CLI 命令，不删除 App Server workflow API、持久化状态或 Core 边界。
4. Net line delta:
   expected: runtime +0; all Rust -500 to -800
   actual: runtime 16,336 -> 16,336 (+0); all Rust 29,908 -> 29,152 (-756)
   （以 `python scripts/line_budget.py` 为准；CLI production/unit/integration
   合计从 4,039 降至 3,283）。
5. Visible surface:
   REPL slash-command surface intentionally shrinks: `/plan` and `/goal` no longer
   start local workflows and are reported as unknown local commands. No model input、
   event、Session schema 或 public App Server protocol changed；App Server clients
   retain the complete Plan/Goal workflow surface.
6. Boundary evidence:
   `cargo test -p mini-agent-cli` (14 unit + 12 interactive integration passed),
   `cargo fmt --all`, `cargo clippy -p mini-agent-cli --all-targets -- -D warnings`,
   `git diff --check`, and `python scripts/line_budget.py`. The three removed tests
   were CLI-only Goal orchestration tests; App Server/Core workflow and boundary tests
   remain protected.

Decision: accept
```

This batch establishes the product split for the next iteration: Studio is the
full user interaction surface, while the REPL demonstrates the harness core
and its stable public path. The follow-up World/MCP/extension presentation
reduction is recorded below; it keeps the world snapshot and capability state
authoritative in Host/App Server.

#### 阶段 1 本轮实践的六项验证：移除 REPL 管理面板

```text
1. Layer: CLI（通过现有 App Server/Host/Core 主路径执行）
   rationale: 本批只收缩 REPL 的展示和管理适配；Host 继续发现扩展并构造
   WorldState，App Server 继续提供 `world/*` 和 `mcp/*` 公共管理方法。
2. Duplicate responsibility:
   searched `repl.rs`, `repl_worker.rs`, `HostRuntime` world construction,
   App Server `world/state`, `world/refresh`, `mcp/status`, `mcp/retry`, and
   Studio/SDK-facing management contracts. REPL 原先重复展示 world/extension
   摘要并复制 MCP retry 入口，完整管理职责已经存在于 App Server。
3. Replace vs add:
   删除启动扩展摘要、world/mcp welcome/help 文案、`/world`、`/world refresh`、
   `/mcp` 解析和对应 WorkerCommand 分支；不删除 Host context 注入、MCP 加载，
   也不新增 CLI 替代入口。
4. Net line delta:
   expected: runtime +0; all Rust -100 to -250
   actual: runtime 16,336 -> 16,336 (+0); all Rust 29,152 -> 28,976 (-176)
   （CLI 合计从 3,283 降至 3,107）。
5. Visible surface:
   REPL 管理命令和启动摘要减少；初始 model context、工具目录、事件、Session
   schema 和 App Server public protocol 不变。需要刷新 world 或重试 MCP 时，
   由 App Server client（Studio/SDK）调用对应管理方法。
6. Boundary evidence:
   `cargo test -p mini-agent-cli` (14 unit + 12 interactive integration passed),
   `cargo clippy -p mini-agent-cli --all-targets -- -D warnings`, `cargo fmt --all`,
   `git diff --check`, and `python scripts/line_budget.py` all pass. The existing
   public CLI scenarios still cover world context construction, extensions, session
   resume, and tool execution; Core/Host/App Server management tests remain intact.

Decision: accept
```

#### 阶段 3 审计基线：Core/Host 的 ToolRouter → ToolOrchestrator 审批准入链路

以下是 `505c24f` 文档审计批次时的代码路径基线；本批随后只增加执行委托切片，
不改变审批策略。审计结论是：当时
mini-agent 已有审批能力，但还没有 Codex 式独立的 `ToolOrchestrator`。实际
路径如下：

```text
model response
  → Core Harness 校验有界 tool calls
  → Core ToolRouter 查找并调用 execute_outcome
  → Capabilities 具体工具自行完成 policy / approval / sandbox 前置检查
  → Host ClassifiedTool 对 legacy error 文本做 outcome 分类
  → Core 截断结果，写入 ToolFinished 和 Session Message::Tool
  → provider 将 Message::Tool 投影为 function_call_output
  → App Server 在 turn settle 后 checkpoint、持久化并发布 TurnFinished
```

关键发现：

- `mini-agent-core::ToolRouter` 目前只是工具目录、名称查找和 dispatch，不能
  统一承载审批、permission/sandbox 选择、`call_id` 关联或执行前 admission。
- Shell、Process、MCP、Edit/Write 等内建工具在各自 `execute` 内部调用
  `ApprovalController`；这些敏感路径当前都在副作用前检查，拒绝也能回写为
  非空的 Tool result。Read-only 工具按职责不请求审批。
- `mini-agent-host::ClassifiedTool` 是兼容性桥接：从 `user denied:`、Plan Mode、
  timeout 等错误文本推断 `NeedsApproval`、`Deferred`、`Retryable`。它发生在
  工具返回之后，不是一个强制的执行前准入器；没有实现该约定的外部工具可能绕过
  统一审批语义。
- Protocol 已有 `ToolExecutionOutcome` 和 `NeedsApproval` 等结构化状态，但
  Responses 的 `function_call_output` 当前只把 content 发给模型，状态仍主要
  保存在内部 event/session 结构中；Host 的 approval broker 也只是通知/应答
  transport，不负责工具路由和执行。
- App Server 的 Actor、checkpoint 和 Session 持久化边界没有发现绕过：工具结果
  在 Core turn 中进入 `Message::Tool`，settled turn 后才进入 runtime/session
  持久化。当前缺口是 approval callback 同步阻塞 turn worker，以及 JSON-RPC
  审批通知虽有 `turnId` 字段但 transport 目前填 `None`，Broker 没有 `callId`。

本次按六项准入问题记录：

```text
1. Layer:
   Core + Host + Capabilities + App Server 边界审计；不把 CLI 或 Studio 的交互
   适配层当作新的执行层。
2. Duplicate responsibility:
   已检查 ToolRouter、ToolExecutionOutcome、ClassifiedTool、ApprovalController、
   ApprovalBroker、Host harness builder、Core batch executor、App Server worker
   和 Responses function_call_output 投影。当前没有第二个 ToolOrchestrator；重复
   责任主要表现为各 Capability 工具分别编排 approval，以及 Host 的事后字符串分类。
3. Replace vs add:
   本批不添加中央 orchestrator，也不把审批搬出各工具。先以现有 Protocol outcome、
   Host/App Server broker 和 Core Session 回写作为证据；未来若引入统一 admission，
   必须先定义替换边界、审批 correlation 和外部工具兼容策略，不能再叠加一层包装。
4. Net line delta:
   expected +0 runtime / +0 all Rust；actual +0 / +0（本批仅审计与文档）。当前
   runtime 为 16,336 / 20,000，全 Rust 为 28,976 / 30,000，暂不为安全语义增加
   未抵扣的 Rust plumbing。
5. Visible surface:
   本批不改变模型输入、工具 schema、事件、Session 或公共协议。审计确认现有
   function_call_output 仍保持兼容；待补的可见面是 approval request/resolution
   与 turn/call 的可关联性，以及是否要让模型看到结构化 status，而非只看到 content。
6. Boundary evidence:
   已复核 Core ToolRouter/ToolBatch、Capabilities workspace/shell/process/MCP
   审批测试、Host outcome 分类、App Server broker/JSON-RPC 通知、Actor settle/
   checkpoint/session 路径和现有 public `NeedsApproval` fixture。当前缺少真正的
   built-in sensitive tool 从 model sampling 经 App Server 发起审批、收到拒绝、
   确认无副作用并完成下一轮 model input 的 public-path scenario。
```

判定：**审计接受，统一 ToolOrchestrator 实现 deferred**。下一步只准在不突破
line ceiling 且有明确抵扣时补一条真实内建工具的 App Server 审批 scenario：请求、
应答、无副作用、`ToolFinished`/`Message::Tool` 非空拒绝结果、checkpoint 和下一轮
`function_call_output` 必须完整可观测。随后再单独评审 typed admission、异步审批
等待和 `turnId/callId` correlation；在这些契约未确定前，不新增通用路由或 sandbox
包装层。

#### 阶段 3 首个改造批次：ToolRouter → ToolExecutionDelegate → ToolOrchestrator

本批把 Codex 原生关系映射为一个最小的 Host 注入边界：Router 负责 resolve，
Host 的 Orchestrator 负责执行委托和 legacy outcome 分类；尚未迁移具体 Capability
的 approval/sandbox 逻辑。因此这不是完整的统一审批实现，而是可逐步替换旧
`Tool::execute_outcome` 的执行生命周期 seam。

```text
Core Harness
  → ToolRouter：查找 tool，并保留 call_id/name/arguments
  → ToolExecutionDelegate：跨 Core/Host 的执行委托契约
  → Host ToolOrchestrator：当前兼容调用 legacy Tool::execute_outcome
  → Capability Tool：当前仍负责自身参数校验和 approval/sandbox
  → Core：写入 ToolFinished / Message::Tool
```

六项准入验证：

```text
1. Layer:
   Protocol 定义可移植的 ToolExecutionDelegate；Core 只保存并调用委托；Host
   装配 ToolOrchestrator。Capabilities、App Server 和 CLI 公共行为不变。
2. Duplicate responsibility:
   搜索 ToolRouter、ToolRegistry、Tool::execute_outcome、Host classify_tools、
   ApprovalController 和 App Server broker。删除旧的 ClassifiedTool wrapper，
   将 outcome 分类收拢到 Host ToolOrchestrator；没有增加第二个 Router 或 Session
   写入路径。
3. Replace vs add:
   用 Router 的 injected delegate 替换 Host `classify_tools` 包装路径；保留
   `Tool::execute_outcome` 作为迁移期 legacy runtime。审批和 sandbox 暂不搬迁，
   避免当前批次同时产生两套 admission。
4. Net line delta:
   expected: runtime 小幅净增且低于剩余预算；actual: runtime
   16,336 → 16,411 (+75)，all Rust 28,976 → 29,051 (+75)。当前余量分别为
   3,589 和 949 行；后续 approval migration 必须净零或提供抵扣。
5. Visible surface:
   不改变模型输入、tool schema、event、Session 或 wire protocol。新增的是
   Rust in-process `ToolExecutionDelegate`/`ToolRouter::with_executor` 注入边界；
   `call_id` 已能穿过 Router 到 delegate，但 approval 的 turn/call correlation
   仍未完成。
6. Boundary evidence:
   `cargo test -p mini-agent-protocol`（7 passed）、`cargo test -p mini-agent-core`
   （32 passed）、`cargo test -p mini-agent-host`（40 passed）、受影响三 crate
   的 Clippy、`cargo fmt --all`、`git diff --check` 和 `python scripts/line_budget.py`
   均通过。缺口仍是实际内建敏感工具的 App Server 审批公共场景。
```

判定：**首个执行委托切片接受**。下一批不直接扩展通用 Orchestrator；只选择一个
内建敏感工具，先定义 typed admission 和副作用前审批证据，再迁移其 approval 调用，
同时删除对应 legacy 分类/路径，确保没有双重审批。

#### 阶段 3 第二个改造批次：Shell typed admission

本批选择 Shell 作为第一个真正迁移的内建敏感工具。Shell 的命令长度和格式校验、
Plan Mode 检查以及 approval action 仍由 Shell 提供；审批决策从 Shell 的主模型执行
路径移到 Host `ToolOrchestrator`，批准后才调用 `execute_after_admission`。直接调用
旧 `execute` 仍保留原审批行为，作为兼容安全路径；模型经 Host 装配的主路径不再
调用 Shell 内部的 `approve`，因此没有双重审批。

```text
model tool call
  → ToolRouter resolve
  → ToolOrchestrator: Shell::admission(request)
  → ApprovalController::approve(action)
  → Shell::execute_after_admission(request)
  → Core ToolFinished / Message::Tool
```

六项准入验证：

```text
1. Layer:
   Protocol 增加可移植的 ToolAdmission 与 Tool lifecycle hooks；Capabilities 的
   Shell 提供 typed admission；Host Orchestrator 执行审批；Core/App Server 只
   复用既有 delegate、事件和 Session 边界。
2. Duplicate responsibility:
   检查 Shell、Workspace::approve、ApprovalController、Host Orchestrator 和
   ToolExecutionDelegate。Shell 的主路径审批调用被移除并由 Orchestrator 接管；
   没有增加第二个 Router、ApprovalController 或 Session 写入路径。
3. Replace vs add:
   用 `ToolAdmission::ApprovalRequired` 替换 Shell 在主执行路径中的直接审批；
   保留 `Tool::execute` 的兼容安全行为，并让其他工具继续返回 `Legacy`，避免在
   尚未迁移的工具上猜测 policy 或引入第二套执行协议。
4. Net line delta:
   expected: runtime +150 以内、all Rust +220 以内；actual: runtime
   16,411 → 16,556 (+145)，all Rust 29,051 → 29,258 (+207)。当前余量分别为
   3,444 和 742 行；下一批必须净零或提供明确抵扣。
5. Visible surface:
   不改变 model input、tool schema、wire protocol、event 或 Session schema；
   Shell 的拒绝文本保持非空且兼容。新增的是内部 typed admission hook；approval
   request 仍是同步 callback，`requestId`/`turnId`/`callId` correlation 仍待公共
   App Server 场景确认。
6. Boundary evidence:
   `cargo test -p mini-agent-protocol`（7 passed）、`cargo test -p mini-agent-core`
   （32 passed）、`cargo test -p mini-agent-capabilities`（66 passed）、
   `cargo test -p mini-agent-host`（42 passed）、`cargo test -p mini-agent-app-server`
   （32 passed）、跨包 Clippy、`cargo fmt --all`、`git diff --check` 和
   `python scripts/line_budget.py` 均通过。Host fake admitted-tool 测试证明批准
   先于执行且拒绝无副作用；Capabilities 测试证明 Shell admission action 和旧
   直接拒绝行为。真实模型 → App Server → Shell approval 公共场景仍是缺口。
```

判定：**Shell typed admission 接受**。下一步先补真实 App Server 公共 Shell approval
scenario，并补齐 approval 与 `requestId`/`turnId`/`callId` 的关联；在该证据和预算
抵扣明确前，不迁移 Edit/Write、Process 或 MCP，也不把 `Legacy` 默认改成隐式 allow。

### 3.4 评估自动化的顺序

自动化和下一迭代按以下顺序推进：

1. 冻结两个 line ceilings，要求每批先完成六项准入回答和固定确认项；
   没有净零计划或明确抵扣，不开始新增 Rust 功能。
2. CLI Trace 的有界契约已实现：`ask --trace-jsonl PATH` 显式创建新 artifact，
   使用脱敏的单轮/总量上限和 CLI 场景证据；baseline 命令不隐式生成 Trace。
3. 只有存在有界故障注入 seam 时，才推进 CLI 公共 MCP timeout projection；
   否则保留 Capabilities/App Server 证据，并将 CLI transport 缺口标为 deferred。
4. 可量化的 compaction trigger 场景已记录，验证最近轮次保留；只有后续证据证明
   50% 不合适时才改变 context 行为。
5. Docker daemon 已可达，普通 profile 与 candidate strict profile 的 workspace mount
   和 container-only 临时文件场景已在当前 Docker Desktop Linux host 记录；strict
   profile 还观察到 read-only root、零有效 capability、无 route 和 bounded cgroup。
   下一步不是直接加 Docker flags，而是先确定 threat model、支持的平台、默认值/opt-out、
   兼容性和 fail-closed 行为；只有显式策略及跨平台边界证据接受后，才增加更强的
   network/capability/privilege/read-only/resource isolation。
6. 第二 provider 或明确的 bounded retry policy 出现后，再做 provider 矩阵和
   retry/backoff；不把付费 provider CI 设为默认门禁。

每项控制在几百行以内，执行受影响测试、Clippy、格式化和
`python scripts/line_budget.py`，同步 note/README/CHANGELOG 后单独提交。缺少
有界 seam 或可复现证据时，停止并 deferred，不增加猜测性的 plumbing。

不要先做一个通用 benchmark platform。mini-agent-harness 当前最有价值的是验证自己的 turn、tool、context、state 和 boundary 语义。

### 3.5 Docker 更强隔离的政策决策门

当前已完成的 Docker 证据包括 daemon 可达、普通 profile 的 workspace 挂载、strict
profile 的 workspace 挂载，以及容器临时文件不落入宿主 workspace；当前 host 的 strict
probe 还观察到网络、capability、只读文件系统和 cgroup 限制行为。它们仍不是跨平台
安全证明，也不覆盖 image provenance、daemon isolation 或真实项目构建兼容性。为避免
把一次运行时探针误写成安全证明，后续 Docker 变更必须先完成以下六项准入记录：

```text
1. Layer: Capabilities runtime boundary; Docker command construction remains in workspace::run_shell.
2. Duplicate responsibility: inspect the existing docker info preflight, docker run command,
   and ProcessSandbox; do not add a second daemon wrapper or sandbox abstraction.
3. Replace vs add: define the required policy before adding flags; replace no existing
   behavior unless the policy identifies a redundant default or compatibility branch.
4. Net line delta: keep the implementation net-zero or name an explicit offset; record
   runtime and all-Rust before/after values.
5. Visible surface: record workspace, process, failure-result, and any public CLI/config
   compatibility changes; model context and protocol changes are not implied by isolation.
6. Boundary evidence: cover daemon unavailable, supported host platforms, each selected
   restriction, and fail-closed behavior with bounded Capabilities/public-path scenarios.
```

Decision: defer implementation until the policy is explicitly accepted. The current
Docker contract and README/SECURITY wording intentionally make no stronger isolation claim.

## 4. 非目标与风险

- 不追求 VS Code 的工具数量、扩展生态、模型数量或 UI parity；
- 不把 stop hook、plugin、scheduler、memory 或 policy framework 提前加入 Core；
- 不为了取得更高 benchmark 分数放宽 hard limits、审批策略、Session 单一权威或公共协议兼容；
- 不把模型差异当作新增兼容 wrapper 的充分理由；必须先有可复现的行为差异和替代方案比较；
- 不把 scenario/eval 结果当作安全证明，安全和持久化仍需要确定性边界测试。

## 5. 当前实现状态与后续缺口

第一版 bounded harness scenario 基线已经实现并晋级为本项目的当前准入证据。以下条件均已满足；未覆盖的场景作为下一批 backlog 继续跟踪：

1. 已有 8 个 bounded harness scenarios 的稳定基线和可审计结果；
2. `goal timeout lifecycle` 已完成 6 项准入记录，并包含 scenario/eval 证据；
3. timeout、cancel、steer、follow-up、resume 的事件与 durable state 顺序已通过回归；
4. test-only FaultInjectionModel 和 Responses parser fault cases 已覆盖缺字段、畸形 JSON、
   部分流和 retryable tool result；CLI public-path scenario 已验证未知工具失败后的下一轮恢复，
   App Server public scenario 已验证 NeedsApproval 拒绝和 MCP timeout failure 在事件、checkpoint
   和下一轮模型输入中保持非空；cross-file refactor scenario 已验证两个文件的读取、编辑和最终落盘；且没有改变生产执行路径；
   CLI `ask --trace-jsonl PATH` 已验证 bounded redaction、总量上限和 create-new ownership；
5. runtime 和 all Rust 两个 hard ceilings 均通过，且本批没有删除受保护的 Core/Actor/CAS/Session 测试或权威；
6. README、CHANGELOG、Agent Notes、`AGENTS.md` 和 PR template 已与实际流程一致。

后续仍需补充 CLI public MCP transport timeout projection、
HTTP 429 provider-specific retry/backoff 合同、Docker sandbox 的更强 network/capability/resource isolation 和独立 model/provider
对比场景；CLI public-path 的未知工具恢复、cross-file refactor、MCP connection/call refusal、
sandbox 前置拒绝以及 App Server 的 NeedsApproval/MCP timeout projection 已覆盖，但更完整的
工具失败/超时/重试矩阵仍是证据缺口，不是当前实现的已覆盖能力。
