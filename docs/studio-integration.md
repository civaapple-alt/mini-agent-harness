# Web Studio 集成

本页定义 `mini-codex` 与 `mini-agent-web` 的跨仓库运行边界。它说明 Web Studio
如何连接 App Server，不重复 JSON-RPC 线协议，也不替代 Gateway、SDK 或前端的目录
README。

## 运行关系与权威

```text
Web Studio browser
    │ REST / WebSocket
    ▼
FastAPI Gateway
    │ Python SDK
    ▼
mini-agent-app-server
    │
    ▼
Host and Capabilities → Core
```

| 范围 | 权威 | Web 层的职责 |
| --- | --- | --- |
| Agent Loop、限制、停止分类与执行事件 | Core | 消费有界事件投影 |
| 工具准入、审批、沙箱和副作用 | Host 与 Capabilities | 提供 Project 根目录并提交审批响应 |
| Thread、Turn、Goal、Session、事件顺序和 JSON-RPC | App Server | 通过 SDK 调用，不维护第二条运行循环 |
| Session history、checkpoint 与授权 grant | App Server 和 SessionStore | 读取 canonical 投影，不建立 Web 副本 |
| Project 清单、显示标题、侧栏固定状态和界面偏好 | Gateway | 保存派生的 UI 元数据 |

Gateway、SDK 和浏览器都不能从工具文本推断授权、重试或执行结果。它们使用 App Server
返回的结构化 `outcome`、ThreadItem 和审批通知。

## 配置与 Project 绑定

Python SDK 为每个 App Server 子进程构造环境。显式 `env` 覆盖进程环境，进程环境
覆盖从 `cwd`、父目录、Web workspace 和 `~/.mini-agent` 读取到的 `.env` 值。SDK
不会修改父进程环境。

Gateway 为每个 Project/Thread 注入以下运行时绑定。不要把这些值写入共享 `.env`：

| 变量 | 用途 |
| --- | --- |
| `MINI_AGENT_PROJECT_ID` | Project 范围与审批匹配的身份 |
| `MINI_AGENT_EXTRA_READ_ROOTS` | 关联的只读根目录 |
| `MINI_AGENT_EXTRA_WRITE_ROOTS` | 关联的可写根目录 |
| `MINI_AGENT_SESSION_READ_ROOTS` | 当前 Thread 的 Gateway 附件根，只读 |
| `MINI_AGENT_SESSION_MODE`、`MINI_AGENT_SESSION_ID`、`MINI_AGENT_THREAD_ID` | 新建或恢复 Session 的身份 |

Session directory 不是工作区根目录。模型通过逻辑 `session_capabilities` 了解 Plan、
Goal、Notebook 和当前 Turn 附件；它不能把 `session.jsonl`、审批证据或其他
SessionStore sidecar 当作普通工作区文件读取。

## Project、Thread 与历史

所有 Web 请求都带 `project_id`。REST 请求在 query 或 payload 中传递，WebSocket
连接使用 `?project_id=...`，每个 `turn`、`steer`、`interrupt`、审批响应和 `ping`
也重复携带 Project 身份。Gateway 先按 Project 过滤运行时广播，因此不同 Project
可以各自拥有名为 `default` 的 Thread。

浏览器在 Project 或 Thread 变化时取消旧请求、清空旧投影，并以 request epoch 拒绝
晚到响应。该保护只避免 UI 串线，不能取代 App Server 对 Session 和运行时身份的校验。

Gateway 提供的跨层读取路径如下：

| 路径 | 用途 |
| --- | --- |
| `/api/threads`、`/api/threads/{thread_id}` | Thread 列表与 canonical history |
| `/api/threads/{thread_id}/items` | 有界 ThreadItem 投影 |
| `/api/threads/{thread_id}/events` | 有界 `turn/event` 重放 |
| `/api/threads/{thread_id}/context-manifest` | 读取 Session-owned 的有界 Context 来源元数据 |
| `/api/threads/{thread_id}/runtime/status` | 非阻塞运行时状态 |
| `/api/threads/{thread_id}/attach` | 恢复可写 Session，或报告外部锁 |
| `/api/threads/{thread_id}/children` | Child Session 的控制与观察投影 |
| `/api/threads/project/{project_id}/sessions/doctor` | 手动检查项目 Session 日志 |
| `/api/threads/project/{project_id}/sessions/{session_id}/doctor/repair` | 备份并修复一个不完整尾记录 |
| `/api/workflows/files` | 列出当前 Project 文件与 Session 计划产物 |
| `/api/workflows/file/content` | 读取受控 Project 文件或 Session 计划产物 |
| `/api/threads/{thread_id}/notebook` | Session-owned Notebook 投影 |

读取历史不会 attach 或修改 Session。若另一个 App Server 持有 Session lock，Gateway
只返回只读历史或锁定信息，绝不启动第二个 writer。

### 检查项目 Session 日志

用户从项目菜单手动发起检查。Gateway 只根据已登记的 `project_id` 找到项目主工作区，
然后调用 App Server 的本地维护命令。浏览器不能提交 workspace 路径。该命令直接复用
Capabilities 的 `load_records` 解析器，不加载目标 Session 的 Agent runtime。

报告将检查状态、历史完整性和恢复可用性分开显示。活动锁对应
`locked_unverified`，不能当作损坏。单次扫描最多读取 8,192 个目录项、检查 4,096 个
Session 目录，并返回至多 256 条 finding。报告不含 prompt、工具参数、工具结果或绝对路径。

只有有效 settled checkpoint 后的未完成日志尾部可以修复。Web Studio 要求用户二次确认；
修复端取得独占锁并重新检查，再将原始日志保存到
`~/.mini-agent/recovery-backups/`，然后截去未完成尾部。序号断档、`recovery_gap`、无效完整
记录和缺失 checkpoint 不会自动修复。修复后 Gateway 返回新报告，Studio 重新扫描项目。

## Plan Mode 与计划查看

Host 的逻辑文件别名 `plan.md` 指向当前 Session 的 Plan artifact。Web Studio 的
`/api/workflows/files` 将该 Session-owned 文件列为 `plan/plan.md`，并让计划查看器优先
显示它。项目根目录的 `plan.md` 是独立工作区文件，不与 Session 计划合并。计划查看器
通过 Gateway 的受控文件接口读取，不扫描物理 Session 目录。

Turn 结算后，SidePanel 使用新的 Turn 结果重新加载计划文件清单和所选文件内容。若 UI
仍显示旧内容，先确认当前 Project/Thread，再刷新计划页；不要用绝对 Session 路径替代
逻辑别名或公开接口。

## Turn、事件与重连

Web Studio 的实时控制使用 `/ws/agent`：

```json
{"action":"turn","project_id":"project-1","threadId":"thread-1","mode":"start","prompt":"inspect the workspace"}
{"action":"steer","project_id":"project-1","threadId":"thread-1","turnId":"turn-1","text":"focus on the failing test"}
{"action":"interrupt","project_id":"project-1","threadId":"thread-1","turnId":"turn-1"}
```

Gateway 将这些请求映射为 App Server 的 `turn/start`、`turn/steer` 和
`turn/interrupt`。对未知工具结果的人工核对通过
`POST /api/threads/{thread_id}/turns/{turn_id}/reconcile` 映射为 App Server 的
`turn/reconcile`。`turn/interrupt` 的成功响应只表示运行时已接纳取消请求；浏览器
必须等到 `turn_finished` 或读取 `turn/read` 的终态，才能把 Turn 视为结算。

`turn/event` 的 `sequence` 属于每个 Thread 的 Core 事件流。它与 ActionResult 的
`actionSequence` 不同，不能混用。WebSocket 重连后，浏览器用最后的 sequence 调用
事件重放。V2 在 Session 中跨 App Server 重启保留最多 512 条有界生命周期摘要；
摘要包含 Thread/Turn/Item 身份、事件类型、工具调用标识和 Context 来源指纹，不包含文本增量、
prompt、工具参数、工具正文或 Context 正文。Live `turn/event` 仍携带原有实时内容。
重放摘要只推进游标和标记待对账活动，不得当作聊天内容。出现 `hasGap=true` 或重连后有摘要时，
Studio 从 `thread/read` 和 `thread/items/list` 的 canonical 投影恢复持久内容，再读取 Runtime 状态。

ToolCall 的稳定 item identity 是模型 `callId`。浏览器可以据此合并模型、工具开始、
工具完成与重放投影；`ThreadItem.status` 是生命周期，`ThreadItem.outcome` 是结构化
工具结果，必须分开处理。

### Session 活动、重启恢复与 UI 一致性标准

Web Studio 将活动分成持久活动和实时状态。持久活动来自 App Server 的 Thread checkpoint
和 `ThreadItem` 投影；实时状态来自 `runtime/status`、有序 `turn/event` 和生命周期通知。
实时事件更新对应的活动项，不能替代 Session 历史。Gateway 广播和浏览器本地状态也不能成为
第二份历史或授权来源。

#### 活动身份与状态

- 用 `(projectId, threadId)` 标识 Web 活动范围。同名 Thread 可以属于不同 Project；切换范围时，
  取消旧请求、清理旧 Session 投影，并拒绝旧请求和旧事件的迟到结果。
- 用 `turnId` 归属 Turn 活动，用稳定 item identity 合并同一活动的开始、完成、历史和回放版本。
  ToolCall 使用 `callId`。不要用工具输出文本推断状态或结果。
- `ThreadItem.status` 表示活动项的生命周期，`ThreadItem.outcome` 表示结构化执行结果。
  `turn/event.sequence` 表示 Thread 事件顺序；`actionSequence`、`stateRevision` 和
  `runtimeGeneration` 各有自己的用途，不能互相比较或替代。
- 将连接状态和运行生命周期分开显示。断线表示当前状态尚未确认，不表示 Turn 已停止或 Session
  已空闲。`turn/interrupt` 的成功响应只表示取消请求已接纳；只有 `turn_finished` 或恢复得到的
  权威 Session/runtime 状态能确认 Turn 结算。`run_finished` 和 `run_failed` 是运行诊断，不能代替
  `turn_finished` 的 Turn 结算边界。
- 审批卡以 `requestId`、Project、Thread 和 Turn 关联。恢复时重新读取 pending approvals；收到
  `approval/resolved` 后关闭对应请求，不能把旧审批卡当作仍有效的授权。

#### 首次打开与断线恢复

首次打开 Session、切换 Session 和恢复连接时，按以下顺序对账：

1. 首次打开或切换 Session 时，先 attach 当前 Session。若 App Server 报告 Session 被其他进程锁定，
   进入只读观察状态；继续读取 canonical 历史，不启动第二个 writer。
2. WebSocket 重连时保留最后一份完整活动投影，并显示“正在恢复”。连接恢复本身不代表 Turn 已停止；
   不重复提交输入。
3. 对同一运行代次，从最后一个已接受的 `sequence` 回放事件，并读取 `runtime/status`、工作流状态和
   待审批请求。运行代次变化、回放返回 `hasGap` 或回放失败时，丢弃旧代次的事件游标并完整对账。
4. 完整对账时读取 `thread/read`、分页的 `thread/items/list`、`runtime/status`、工作流状态和待审批请求。
   这些结果恢复历史活动、当前 Turn、Session 设置和可执行操作。若尚未 attach，则先尝试 attach。
5. 同步期间到达的 WebSocket 事件先缓冲，再按 `sequence` 排序应用。用稳定 item identity 合并重复投影，
   丢弃已应用事件。事件缓存用于补齐短暂断线，不是持久历史。
6. 所有读取和回放都确认属于当前 Project、Thread 和请求代次后，才把界面标记为在线并解除同步状态。

浏览器与 Gateway 的 WebSocket 断开时，已接纳的 Gateway Turn stream 可以继续运行。重连后应观察并
对账该 Turn，不得因连接重建而重新提交相同输入。Gateway 或 App Server 进程重启后，未结算的普通
Turn 不会自动从中断点继续；Session checkpoint 为新 Turn 或 fork 提供已结算上下文，Execution checkpoint 只供
操作者显式恢复同一个 Turn。若工具已开始但结果未知，Studio 要求操作者选择“已完成并提交有界结果”或
“确认未执行”；`turnId`、checkpoint 序号、`toolCallId` 和稳定 `requestId` 绑定该决定，重复请求幂等，过期检查点被拒绝。
核对不会自动重放工具。界面在所有未知调用核对完毕前不提供继续或新 Turn 执行。Goal 和 Child operation 按其各自的
持久生命周期恢复；客户端不得自行重跑 Goal 输入或重复启动已有 Child Turn。

#### 实时活动的方向提示和交互

状态栏始终给出当前 Project/Session 范围、Turn 生命周期、连接状态、最近运行阶段和下一步可执行操作。
连接恢复中时明确显示“正在恢复”或“连接中断”，同时保留最后的活动上下文。不要把未知状态显示为
“空闲”或“已完成”。审批、停止确认、计划审阅、只读锁和失败状态都提供对应的下一步操作或原因。

对话时间线按 Session 活动顺序呈现 Turn 和活动项。当前 Turn 有清晰标记；Session Turn 导航可定位到
历史 Turn，并提供输入摘要、来源、结果摘要和有界运行指标。Child 的父级视图展示 operation 摘要，
Child transcript 留在自己的 Session 活动时间线。

实时更新不能打断用户查看旧活动。用户停留在最新活动附近时，时间线可以跟随新增内容；用户滚动查看
历史时，保持其滚动位置并显示“有新活动”入口。该入口跳到当前 Turn 的最新活动，历史 Turn 选择则
跳到所选 Turn。工具调用和结果使用可折叠的结构化卡片；推理等大段细节默认折叠，避免实时流淹没
当前步骤和下一步操作。

## 审批与执行设置

敏感工具调用从 Host/App Server 以 `approval/request` 发到 SDK。Gateway 将它按
Project 广播给浏览器，浏览器通过 REST 或 WebSocket 提交 `approval/respond`。首个
通过 Project、Thread、Turn 和 request 身份校验的响应生效，随后 `approval/resolved`
关闭其他浏览器上的待审批卡。

浏览器和 Gateway 只保存 pending/UI 状态。授权 grant 属于 Host/Capabilities 的
运行时内存，并按完整 `ActionGrantKey` 匹配。Project、workspace revision 或 policy
变化，以及显式撤销，都会使旧 grant 失效。

`project` 与 `full_machine` 是路径范围；`interactive`、`automatic` 与 `trusted`
是审批策略。Trusted 自动准入通过 URL 校验的公网 `web_fetch`；Interactive 仍要求
确认。`full_machine` 不等于 allow-all。Deny、工具可用性、Plan 模式的源文件修改锁和
其他高风险确认仍由 App Server/Host 执行。

## Fork、Child Session 与 Notebook

`thread/fork` 是进程内的逻辑 Thread 分叉。`session/fork` 从最近一次已结算的
checkpoint 建立独立持久化 Session。精确 fork 可以在来源 Turn 运行时使用最近的完整
checkpoint；compact fork 要求来源 Turn 空闲。两者都不复制进行中的输入、工具调用或
审批等待。

Child 运行在独立 App Server runtime 中。`delegate_task` 的 `execution_mode` 必须
是 `parallel` 或 `sequential`；顺序任务还需要 `group_id`。Gateway 只把 operation
的有限状态投影给父 Thread，Child transcript 始终留在 Child Session。
委派使用父 Session 内唯一的 `child_key`。Capabilities 根据父 Session ID 和该 key
生成 canonical `child_thread_id`；同名标题不会共享 Thread。Gateway 只在成功的
`delegate_task` 结果中取得 canonical ID 后才物化子 Session。结果只携带身份和调度元数据，
不回显提示词；Gateway 先按父 Turn 与工具调用 ID 持久化 `tool_started` 参数，再与成功结果配对，
这样最大 32 KiB 的提示词不会被重复放进模型可见工具输出。

Child 可用 `task_report` 发送有界进展；Gateway 通过 App Server `child/task`
写入 Child Session，再刷新 `/children` 和父 Turn 批次卡。父 Thread 活跃时，报告和
终态只更新持久状态与批次卡，不调用 `turn/steer`。父 Turn 结束后，Gateway 合并待处理更新，
启动一轮带 `turnSource: "child_wakeup"` 的续行。父 Turn 空闲时到达的更新也启动一轮续行；续行期间的新更新留到下一轮空闲边界。
该来源随 Turn 事件和 Session item 投影提供给客户端，因此 UI 可在 Turn 轨道标记“子代理更新”，
并隐藏续行输入，不把它显示为用户消息。
Gateway 每个父 Session 最多缓存 64 个不同子任务的最新状态；每轮最多提交 16 个更新。超限更新
合并为数量和最多 8 个示例 ID。Gateway 使用每个 Session 共用的启动锁串行化用户 Turn 和自动续行。

父代理可以通过 `task_control` 修改/取消排队项、引导或取消运行项、重试失败项、取消顺序组，或用
`delegate_task` 追加新方向。顺序组缺号时等待，前序失败或取消时暂停后续项。运行面板展示相同的持久
状态和最近报告；只有已创建 Session 的任务可打开。Gateway 事件是刷新提示，Session operation/report
仍是状态依据。若终态 operation 快照未写入，读取方会用同一 Child Session 中匹配的
`turn_settled` 和最终 assistant item 恢复终态与结果；`reports` 仍只代表显式 `task_report` 进展，
不会代替最终回复。手动 steer 只在客户端收到 `steer_ack` 后提示一次。

`task_control.assign` 根据 App Server 持久投影中的实时状态自动路由：运行中（包括已报告进展但仍继续执行）
或等待审批的 child 使用带稳定 `requestId` 的 `turn/steer`；已完成 child 使用 `child/task` 的
`queue_follow_up`，在同一 Thread 上启动一个新 Turn。Follow-up 使用相同 `operationId`、递增 attempt，
并记录 `attemptKind: "follow_up"`。提示最多 32 KiB；并发容量或顺序组暂不可用时保持 queued，由持久队列
在可运行后启动。重复请求 ID 返回同一后续 attempt。失败、取消或步数受限仍调用现有 retry，沿用原提示词。
App Server 在提交 child steer 前先持久化 request ID reservation。若恢复时只有未结算 reservation，`turn/steer`
返回 `pending`；Gateway 不自动重发该 ID，并会在父会话唤醒中注明结果未确定。父代理先读取权威子 Turn 和 operation 状态，再决定后续动作。该指令可能已提交，也可能未提交。若子任务在 reservation 前已完成，App Server 返回 `not_submitted`，Gateway 可读取新状态并用相同 request ID 建立 follow-up。
每个父会话只显示一张 child 卡，轮次分别标记初次执行、重试和 follow-up；子详情显示该 Session 自身的
本地 Turns，不显示父 checkpoint 的历史活动。

`thread/items/list` 是子查看器的活动来源。Fork 的父 checkpoint 继续提供模型上下文，但子 Session 的
item 投影不回退显示该 checkpoint 的消息。旧 child 没有本地活动项时，客户端显示空态。父 checkpoint
来源信息单独展示，不混入 child 活动时间线。

Gateway 也会把 `task_control` 的执行结果合并到父会话唤醒中。活动父 Turn 不会被打断；
Gateway 等 Turn 结束后再启动续行。父 Turn 空闲时到达的更新会启动一轮续行。该结果说明 Gateway 控制调用
是否应用、失败或部分完成，不代替 App Server 中的 operation 状态。待处理唤醒保存在
Gateway 内存中，Gateway 重启后不会重放；持久报告和任务状态仍可从 App Server 读取。队列溢出摘要
不替代 operation/report 投影，后续处理仍以 App Server 状态为准。

Notebook 属于当前 Session。Gateway 通过 App Server 读写它，不缓存第二份内容。Child
可以读取 Host 校验的父级快照，但只能修改自己的 Notebook。

## 维护边界

修改 JSON-RPC 方法、DTO、事件或错误码时，更新 [`app-server.md`](app-server.md)。
修改 SDK 行为时，更新 `mini-agent-web/sdk/python/` 下的文档。修改 Gateway 路由或
Web Studio 组件时，更新其各自目录 README。不要用 Gateway 缓存、浏览器状态或 Session
sidecar 修补缺失的运行时投影。
