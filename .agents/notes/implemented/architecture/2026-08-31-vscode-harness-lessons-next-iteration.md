# VS Code Coding Harness 经验与 mini-agent-harness 下一迭代准入笔记

Status: implemented

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

本批 8/8 通过。该第一版 baseline 已证明现有公共路径可承载基础 harness evidence；当时尚未包含跨文件重构、CLI 公共路径的工具失败恢复、MCP/approval/sandbox 拒绝和独立的 model/provider 对比，不将后续补充结果倒填为第一版基线。阶段 2/3 后续批次已分别补上部分边界证据，详见下文。

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
| Fault Injection Provider | 首版已实现，限定为测试设施 | `mini-agent-core` 的 `FaultInjectionModel` 覆盖缺少必要工具参数、部分流失败和 `Retryable` 工具结果；`mini-agent-capabilities` 的 Responses 解析测试覆盖畸形 JSON 和缺字段。HTTP 429 provider 适配语义仍单独待补；不增加生产 provider、不调用付费服务、不另起执行循环。 |
| 超过 5 秒的场景移出日常 CI | 部分接受 | 5 秒是 CI 调度策略，不是运行时语义；只有经确认的确定性慢场景才允许显式 `#[ignore]`，并提供定时或手动命令。不能按一次机器墙钟测量自动改变门禁。 |
| Compaction 在 70% 触发 | 暂不改变当前行为 | 当前实现和 `docs/limits.md` 的确定性触发点是最大上下文的 50%；下一场景在 70% 记录预警、最近 3 轮保留和压缩前后预算，只有证据证明 50% 不合适时才改阈值。 |
| 权限失败返回 `permission_denied`，不能是空结果 | 已用现有结构化状态验证 | 当前保留 `ToolExecutionStatus::NeedsApproval`，App Server 公共场景已断言事件、Session checkpoint 和下一轮模型输入均包含非空拒绝 reason；下一次审批/沙箱变更不得把该契约退化为空结果，再单独评估是否需要公共 `PermissionDenied` 变体。 |
| timeout 与 steer 并发时记录确定性优先级 | 接受 | 当前顺序明确为：Core 安全检查点先检查 cancel，再检查 steer；deadline 触发后 App Server 发送 interrupt、等待 `TurnFinished` 和 durable checkpoint，然后返回 timeout，不继续 drain 排队的 steer；普通已 settle batch 才按 steer 优先于 follow-up。修改该顺序前必须增加并发 race scenario。 |

当前 baseline 已有稳定的本地 JSONL Trace artifact；README 中的快捷命令仍只捕获
测试输出和预算快照，尚未自动把 `JsonlTrace` 接入 CLI 基线报告。Trace 只用于证明
事件/输入摘要变化，不将 mock provider 的通过结果等同于真实模型质量。Stage 3
审计暂缓 CLI 自动 Trace：需要先明确 artifact 生命周期、session 单一权威和用户可见
CLI 选项，再以独立变更重新评估。

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

## 5. 当前实现状态与后续缺口

第一版 bounded harness scenario 基线已经实现并晋级为本项目的当前准入证据。以下条件均已满足；未覆盖的场景作为下一批 backlog 继续跟踪：

1. 已有 8 个 bounded harness scenarios 的稳定基线和可审计结果；
2. `goal timeout lifecycle` 已完成 6 项准入记录，并包含 scenario/eval 证据；
3. timeout、cancel、steer、follow-up、resume 的事件与 durable state 顺序已通过回归；
4. test-only FaultInjectionModel 和 Responses parser fault cases 已覆盖缺字段、畸形 JSON、
   部分流和 retryable tool result；CLI public-path scenario 已验证未知工具失败后的下一轮恢复，
   App Server public scenario 已验证 NeedsApproval 拒绝在事件、checkpoint 和下一轮模型输入中
   保持非空；cross-file refactor scenario 已验证两个文件的读取、编辑和最终落盘；且没有改变生产执行路径；
5. runtime 和 all Rust 两个 hard ceilings 均通过，且本批没有删除受保护的 Core/Actor/CAS/Session 测试或权威；
6. README、CHANGELOG、Agent Notes、`AGENTS.md` 和 PR template 已与实际流程一致。

后续仍需补充 CLI 自动接入 Trace 报告、HTTP 429 provider 适配、MCP/sandbox 拒绝和独立
model/provider 对比场景；CLI public-path 的未知工具恢复、cross-file refactor 和 App Server 的
NeedsApproval 拒绝已覆盖，但更完整的工具失败/超时/重试矩阵仍是证据缺口，
不是当前实现的已覆盖能力。
