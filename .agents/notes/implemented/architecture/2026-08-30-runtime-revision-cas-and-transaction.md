# Runtime Revision, CAS, and Transaction Boundary

Status: implemented

## Decision

阶段三把运行时的“当前版本”统一为 `RuntimeRevision`。它由
`RuntimeActorState` 持有，所有成功的运行时状态变更都在 actor 内递增；
App Server 对外只镜像这个值，用来给下一次请求生成 expected revision。

运行时请求携带它创建时观察到的 revision。只有 mutation 做 CAS 校验：
如果 expected revision 和 actor 当前 revision 不一致，直接返回
`RevisionConflict`，不会执行副作用；查询不需要 revision token。

ActionEnvelope 仍记录 worker 接纳命令时的 base revision。它描述服务器当时
看到的状态版本，和请求携带的 expected revision 各司其职：前者用于审计和
排序，后者用于拒绝过期写入。

## Session 与 Thread 的原子边界

`AppendContext` 是当前最重要的跨对象更新，现按以下顺序在 actor 中执行：

1. 保存 Thread 的旧 checkpoint；
2. 更新 live Thread；
3. 将新 checkpoint 写入 Session；
4. 持久化失败时恢复旧 Thread checkpoint；
5. 持久化成功后，调用方才看到成功结果。

`SessionStore::append_records` 还会在写入、flush 或 sync 失败时截断本次追加
的内容并恢复内存计数。新建 Session 立即写入空 checkpoint，因此在第一次
上下文更新前进程退出时，仍然可以恢复为一个有效的空 Thread。

这关闭了原先“先更新 Thread、再由 CLI 另行持久化 context”的明显分裂边界。
World 的 context 更新也复用同一事务路径，只有 Session 成功后才替换内存
World。

## 保留的边界

一轮 Core turn 现在由 worker 在持有 Thread 的同一个执行动作中完成结算：
先取得 settled result 和 checkpoint，再调用 Runtime Actor 内部的 Session
持久化适配，最后释放 `TurnFinished` 事件。CLI 不再调用 `record_turn` 或
`record_batch`，也不再决定持久化时机。每轮只把相对 turn 开始时新增的消息
写入 item records，checkpoint 仍保存完整上下文。

如果 Session 写入失败，`turn/read` 会返回原始 turn 结果及持久化错误；完成
事件仍然会被释放，避免客户端永久等待。当前仍没有跨进程事务日志：进程在
文件系统 sync 之后退出属于成功，文件系统自身故障由 SessionStore 的追加
回滚和尾部恢复策略处理。

## Verification

- `runtime_mutations_reject_stale_revision_tokens` 验证并发 mutation 中只有
  一个旧 token 能成功。
- `running_goal_is_paused_when_a_session_restarts` 验证新 Session 的初始
  checkpoint 和恢复路径。
- `cargo check --workspace --all-targets` 和 workspace Clippy 已通过。
