# 跨 Turn 的定时唤醒任务

状态：第一版已实现（有界 delay 标记、App Server 控制面、SDK/Gateway/Studio 投影）；
供应商适配器、Webhook、runtime 重启恢复和自动模型续跑仍不属于第一版。

> 当前实现校正（2026-09-19）：下文“当前 Turn 立即结束/下一轮模型读取”是原设计目标，
> 实际工具只记录 delay marker，不结束当前 Turn，也不唤醒或启动后续 Turn。以
> [App Server 当前契约](../../../../docs/app-server.md)为准。

## 决策

模型有时需要等待远程状态变化，例如 GitHub Action、云端构建或部署任务。让模型
在当前 Turn 中执行 `sleep 30` 再轮询，会占住 Turn，使 `steer`、`interrupt` 和
手动停止受到长 Shell 的影响。本提案增加一个很小的模型可见能力：定时唤醒标记。

它不是 `ExternalWaitOperation`，也不是通用 Scheduler。第一版只做：创建一个有界
的 delay 标记；当前 Turn 立即结束；到期后标记变为 `ready`；下一轮模型读取标记，
再显式执行一次远程状态查询。

```text
BackgroundShellTask = 本地进程和进程组的跨 Turn 生命周期
ScheduledTask       = 下一轮可以继续查询的时间标记
```

两者不互相替代：Web、Backend、Dev Server 仍使用后台 Shell；GitHub Action 等无本地
进程的等待使用 `ScheduledTask` 协助跨 Turn，不进入后台 Shell 运行面板的进程控制。

## 第一版接口

模型工具名为 `scheduled_task`，只允许以下动作：

```json
{
  "action": "create",
  "task_id": "check-action",
  "delay_seconds": 30,
  "summary": "等待 GitHub Action 后查询状态"
}
```

```json
{ "action": "list" }
{ "action": "read", "task_id": "check-action" }
{ "action": "cancel", "task_id": "check-action" }
```

约束：

- 只有 `delay` 触发，延时为 1 秒至 24 小时；
- 只返回 `task_id` 和当前记录，不执行 Shell、不执行任意模型提示、不自动发起下一轮；
- 重复创建同一 `task_id` 返回已有任务，不创建第二个记录；
- `cancel` 只取消本地等待标记，不取消远程 GitHub Action、构建或部署；
- 当前 runtime 内存持有记录，runtime 关闭时清理；第一版不做重启恢复；
- Child Session 可以读取 owner Thread 的任务，但不能创建或取消；
- `list/read` 在读取时刷新到期状态，避免增加后台 worker 或第二份调度注册表。

## 分层与所有权

- Core 不新增 scheduler、等待线程、远程引用或持久化状态；
- Capabilities/Host 持有有界 `ScheduledTaskManager`，负责创建、到期投影、幂等和取消；
- App Server 暴露 `scheduled-task/list/read/cancel` 及 `scheduled-task/updated`，并以
  主 Thread runtime 作为唯一 owner；
- Gateway 只转发 owner 的权威结果，Child 路由只读；
- Web Studio 运行面板显示用途、状态和剩余时间，不猜测远程操作是否完成；
- 下一轮模型负责调用对应的远程查询工具，并根据返回结果决定继续安排、结束或报告失败。

## 远程等待边界

`ScheduledTask` 不保存供应商特定的取消协议，也不声称拥有远程任务。未来如果要减少
模型手工查询，可以在 Host/Capabilities 增加明确的供应商适配器或 webhook 入口，但
必须先证明其授权、重试、去重和故障恢复边界；不能把这些责任塞进通用 scheduler。

以下行为仍明确不属于第一版：

- 自动在到期时续跑模型 Turn；
- 自动执行一个任意 Shell 或 prompt；
- 取消远程 GitHub Action 或云端任务；
- App Server/Gateway 重启后恢复内存任务；
- 用文件变更或端口状态隐式重启本地服务；
- 维护独立的 Gateway 调度器或 Service Registry。

## 六问准入

1. 归属：Capabilities/Host 提供受限能力，App Server 承担 runtime 级控制，Core 不变。
2. 责任复用：复用现有 ToolRouter、ActionResult、RuntimeCommand、通知和 Thread owner，
   不新建 `ExternalWaitOperation` 或通用调度框架。
3. 删除旧概念：模型侧不再用前台 Shell sleep 表达远程等待；保留 BackgroundShellTask
   只服务本地进程，不把两种生命周期合并。
4. 预算：实现后运行 `python scripts/line_budget.py`；新增代码必须保持 release-source
   硬上限，并记录实际 delta。
5. 可见面：新增一个受限模型工具、三条 App Server 方法、一个更新通知和 SDK/REST/UI
   投影；不新增 Core 事件、跨 runtime 持久化字段或 Gateway 状态缓存。
6. 证据：Capabilities 单测覆盖延时、幂等、Child/禁用 runtime；App Server/SDK/Gateway
   覆盖 list/read/cancel 与 Child 拒绝；Harness 场景覆盖“创建后 Turn 结束、到期变
   ready、下一轮可读取”，并明确不证明远程 Provider 的最终状态。

## 验收标准

- 创建任务不会阻塞当前 Turn、steer 或 interrupt；
- 到期前为 `scheduled`，读取后到期变为 `ready`，取消后为 `cancelled`；
- 重复 task_id 幂等，Child 创建和取消 fail closed；
- runtime 关闭时记录清理，不遗留等待线程或子进程；
- App Server、SDK、Gateway 和 Web Studio 展示同一份权威状态；
- 运行面板只展示事实，不把 `ready` 推断成远程 Action 已完成；
- 远程等待不会被识别为 BackgroundShellTask；
- 没有自动模型续跑、供应商取消或重启恢复的隐式行为。
