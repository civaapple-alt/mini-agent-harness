# 03 上下文与状态：模型看见什么，系统记住什么

“模型知道项目里的哪些信息？”和“服务重启后能恢复什么？”听起来像同一个问题，其实属于两种不同边界。前者关乎一次模型请求的上下文；后者关乎 App Server 和 SessionStore 持久化了哪些运行状态。把二者分开，才能解释为什么模型上下文可以压缩，而一个结果未知的工具调用不能靠重跑来恢复。

## 一次模型请求由什么组成

模型请求通常包含稳定的系统提示、对话消息、工具规格，以及本轮运行时追加的动态上下文。Host 会准备工作区状态、Session 能力、当前启用的 Skills 等内容；某些工作区说明则在访问相关文件时才按需注入。Core 将这些输入放进有界请求里，再调用所选模型。

当前关键限制按**序列化字节数**计算：请求上下文最多 `64 MiB`，模型响应最多 `16 MiB`，一个模型步骤最多提出 8 个工具调用，单条工具结果在模型上下文里最多内联 `16 KiB`。`64 MiB` 是包含系统提示、JSON 消息和工具规格的保护上限，不是模型 token 窗口；不同模型和提供方会有自己的 token 上限及使用统计。

如果工具结果更大，Host 可以将结果保存成 Session artifact，并把有界预览和读取句柄交给模型。这样模型能按页读取所需部分，而不是把完整日志或文件一次塞进上下文。

## 动态上下文有来源，也有版本

项目说明、Skills、工作区状态等动态内容作为消息追加到 Session 历史。相同来源指纹不会重复追加；来源变化时，新版本会记录它替代的旧指纹。压缩历史后，运行时仍能识别每个来源槽位当前有效的版本。

Web Studio 的 Context Manifest 用来查看上下文来源元数据，例如来源名称、类型、版本指纹、适用路径、注入原因和字节数。它最多保留 512 条记录，只列来源元数据；上下文正文仍来自运行时消息，Gateway 不持有第二份提示词或授权状态。

## 压缩历史时，最近工作仍然可读

Core 默认启用 `Compact`。序列化上下文达到配置上限的一半时，Core 会先压缩历史。
如果兼容模型仍以 token 上下文超限拒绝请求，Core 会把上下文压到当前大小的约 90%，
再对同一模型步骤重试一次。压缩会保留最近的工作尾部，并为较早的消息生成摘要。
摘要请求不带工具规格。摘要请求失败、摘要无效或没有缩小上下文时，Core 会机械裁剪
较早的消息。调用方可以显式设置 `Reject` 关闭自动压缩。

压缩只改变后续模型请求使用的对话上下文；App Server 仍保留 Turn、事件和 Session 的权威记录。Core 不估算 provider token 数；预压缩按字节阈值运行，模型明确报告 token 上下文超限时再进行一次有界恢复。

## 两种 checkpoint，两个恢复问题

| 记录 | 写入时机 | 用途 |
| --- | --- | --- |
| Session checkpoint | Turn 结算后 | 给下一次 Turn 和 Child fork 提供已结算的对话上下文。 |
| Execution checkpoint | 一个逻辑 Turn 执行期间，在模型请求前和工具批次后 | 保存当前输入、消息、下一模型步骤和已记录工具结果，以便操作者核对后显式恢复。 |

执行日志还会记录工具调用何时开始、何时取得结果。如果进程在副作用发生后、结果落盘前退出，系统不知道文件是否已写好或命令是否已完成。启动时不会自动重放这类调用，而是将 Turn 标为需要核对。操作者通过 `turn/reconcile` 提交有界证据及结果，所有未知调用处理完毕后，才可以显式 `turn/resume` 同一个 Turn。核对本身不会执行工具，也不会自动继续运行。

因此，心跳超时、连接断开、取消被接纳，都不等同于进程已经退出或副作用未发生。可靠恢复必须依据 checkpoint 序号、工具调用身份和 App Server 的持久状态。

## Web Studio 怎样显示有界历史

浏览器打开 Thread 时先读取 Thread 摘要和最新一页活动项；当前页最多 128 条，向上滚动时再请求更早页面。刷新时，前端按稳定 item 身份合并重叠内容；如果新页面和旧页面没有可确认的重叠，就重新从最新位置对账。WebSocket 事件提供实时变化，App Server 的权威 Thread 和 item 投影负责恢复持久内容。

这种设计同时限制单次读取规模和客户端内存，也避免把事件缓存误当成完整历史。界面断线重连后，先对账当前 Project、Thread、Turn 和运行状态，再决定哪些操作可用；它不会因为没收到事件就把 Session 标为空闲。

## 代码与规范入口

- [上下文与限制](../../limits.md)、[App Server 执行恢复](../../app-server.md#turn-execution)。
- [Studio 历史和恢复流程](../../studio-integration.md#turn-事件与重连)。
- Core 上下文与执行 checkpoint：[crates/mini-agent-core/src/harness.rs](../../../crates/mini-agent-core/src/harness.rs)。
- 来源清单：[crates/mini-agent-capabilities/src/session/context_manifest.rs](../../../crates/mini-agent-capabilities/src/session/context_manifest.rs)。
- Web 仓库入口（相对 `mini-agent-web` 根目录）：`frontend/src/utils/threadHistoryPages.js`、`server/routes/threads.py`。
