# 长 Turn 纠偏与 Session 恢复可信性

## 问题

长时间运行的 Turn 中，Web Studio 曾将纠偏请求超时显示成失败，但 App Server 可能已经接受并注入；思考活动也可能被错误分段。停止请求只代表取消已受理，用户仍可能看不到明确终态。Gateway 收到 Ctrl+C 时，SDK 子进程还需要经由正常生命周期关闭，避免与 Gateway 一起被信号打断。

Session 恢复还必须保留工具调用之前的历史活动，并让副作用未确认的调用保持可核对、不可自动重放。

## 边界与状态

- Core 继续拥有模型/工具边界上的 Turn 控制和执行检查点；取消模型请求时丢弃正在等待的请求 Future，Host 可取消工具仍使用既有的取消令牌。
- Capabilities 的 `SessionExecutionJournal` 是纠偏和执行恢复的持久来源。Gateway、SDK 和 Studio 只传输或投影状态。
- 纠偏状态使用 `accepted`、`applied`、`unapplied`：App Server 先持久化再确认受理；只有纠偏进入后续模型上下文并写入检查点才标记为已应用；停止先到时将仍待应用的请求标为未应用。
- 纠偏请求 ID 长度不超过 192 字节，正文不超过 30 KiB；每个 Turn 最多 16 个待应用请求，历史保留最多 64 项。相同 ID 和正文幂等返回已有状态；同 ID 的不同 Turn 或正文会被拒绝。
- `turn/read` 提供持久化纠偏状态。重启后只在用户显式 `turn/resume` 时重新排入仍为 `accepted` 的纠偏；事件回放不携带纠偏正文。
- 停止状态只有在终态事件或权威运行状态读取确认后才清除。Gateway 收到取消响应不当作 Turn 已停止；状态不可读时 Studio 保留“结果未确认”和手动刷新入口。
- 对结果未知的工具调用，恢复历史通过 `turn/read` 与执行检查点补充；用户核对后还需显式续跑同一 Turn。不会因 Gateway 或 App Server 重启而重放工具。

## 跨层实现

Harness 在 Core 内增加模型请求的协作取消，并把纠偏请求 ID 附着在待处理 `TurnInput` 上，避免重复队列封装。Capabilities 持久化有限的纠偏记录及检查点应用 ID。App Server 在运行中的命令处理器中完成持久受理、去重、状态读取和停止竞态处理；恢复时重排尚未应用的请求。

Python SDK 的 `steer_turn` 接收 `request_id`；Gateway REST 和 WebSocket 把 ID 传给 `turn/steer.requestId`，`turn/read` 则透传恢复状态。Studio 分开显示受理与应用状态；收到无确认错误后先查询同一 Turn 的状态，不自动重发。停止期间等待权威结算，无法读取时提供刷新入口。

SDK 在 Unix 上将 App Server 放入独立 Session，在 Windows 上创建独立进程组。Gateway lifespan 通过 SDK 生命周期关闭并回收子进程。

## 验证证据

- Rust 受影响包：`mini-agent-app-server` 103 项、其二进制 1 项、`mini-agent-app-server-protocol` 23 项、`mini-agent-capabilities` 176 项、`mini-agent-core` 64 项通过。
- 对应 Rust 包 Clippy (`--all-targets -- -D warnings`) 与 `cargo fmt --all` 通过。
- 行数门禁单测 17 项通过。Core + Protocol 硬上限同步提升至 7,500；当前计数 `7042/7500`，Control Plane `43153/45000`，Release `61895/65000`，均通过。
- Python SDK Ruff 与测试通过，SDK 102 项；Web Ruff、格式检查通过；Gateway steer/recovery 相关测试 38 项通过。
- 前端 Node 单测 103 项、Vitest 222 项通过；ESLint 与 Vite 构建通过。构建仍报告已有的 bundle 超过 500 KiB 提示；Vitest 部分 UI 测试打印对未启动 `localhost:3000` 的预期连接拒绝，但没有测试失败。
- 隔离 Gateway SIGINT 冒烟通过：使用临时 HOME、临时 Web 状态目录、随机端口和本地 App Server 子进程；`/health` 返回成功，Gateway 收到 SIGINT 后完成应用 shutdown，并回收了 PID `46128`。未调用真实供应商。

## 尚未验证

没有对浏览器中已打开的真实 Session 注入停止、断线或重启；未运行真人用户对照走查。受影响状态通过确定性 Rust、Gateway 和前端测试覆盖，进程退出另由隔离 Gateway 冒烟确认。
