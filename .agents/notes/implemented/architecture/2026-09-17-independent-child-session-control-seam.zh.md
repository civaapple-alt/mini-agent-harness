# 独立 Child Session 控制接缝

- status: implemented
- date: 2026-09-17

## 决策

Child Session 不改变 `mini-agent-core` 的职责。Core 继续只负责一个
Thread 的显式 Turn Loop、工具契约、限制、停止分类和观察事件；并发由
Host/App Server 在结构上表达为独立的 Session/runtime。

当前落地的是第一条控制垂直切片：

1. exact `session/fork` 可以在父 Turn 运行时读取最近一次完整持久化
   checkpoint；
2. 不读取或修改父 Core 的可变 checkpoint、当前 prompt、工具调用或审批；
3. WebStudio 通过独立的 Mini Agent App Server client 启动 child Turn；
4. child 的 SessionStore lineage、Thread events、runtime status 和历史仍
   使用各自的标准边界；
5. 每个父 Thread 同时最多两个 child Turn，第一版只允许 exact context。

`compact` fork 仍要求父 Thread idle，因为它需要源 runtime 准备压缩上下文。
正在 stopping 的父 Turn 也不会接受新的 fork。父 Turn 的 in-flight 内容不会
复制到 child；child 从最近一次已提交 checkpoint 开始，再接收独立任务 prompt。

## 为什么不破坏 Core

父、child 分别拥有自己的 Session lock、Thread、App Server worker、Host/
Capabilities 和 event replay。Web Gateway 只负责创建/绑定独立 client 和
转发标准事件，不把 child 历史或授权状态复制成第二份权威状态。也没有把
多租户调度器、线程池或递归委派机制加入 Core。

这里的“统一执行路径”指统一 Protocol、Host admission、Approval、Session
持久化和 replay 契约；不要求父子运行在同一个进程。

## 验证

- `cargo test -p mini-agent-capabilities`：通过；
- `cargo test -p mini-agent-app-server`：通过；
- Rust JSON-RPC fixture 验证父 Turn active 时 exact fork 使用初始 settled
  checkpoint；
- Web Gateway fixture 验证 child 使用独立 client，并且父 Turn active 时不等待
  父 Turn 结束；
- Web Session catalog fixture 验证 child lineage 直接来自 SessionStore 的
  `forked_from`，不依赖 Gateway metadata；
- Gateway 与 frontend 相关测试通过。

## 尚未实现

该接缝还不是完整的 `delegate_task` 产品能力。Notebook、可恢复的持久化
operation register、断线后的 child task 控制、有限重试、父子取消级联和
模型自主委派仍按提案批次推进；它们不能通过本切片的 Web endpoint 假设已经存在。

相关计划见
[时间上延展、结构上并发：Agent Harness Charter](../../../../.agents/notes/proposed/architecture/2026-09-17-time-extended-structural-concurrency.zh.md)。
