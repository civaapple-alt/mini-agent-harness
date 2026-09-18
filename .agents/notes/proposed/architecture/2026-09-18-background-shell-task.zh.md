# 跨 Turn 的后台 Shell 任务

状态：第一版实现完成（本地进程生命周期、App Server 控制面、Gateway/Studio 投影和
跨 Turn 公共场景已接入；runtime 重启恢复与远程等待仍明确留在后续范围）

## 结论

本提案只解决本地进程型长任务的跨 Turn 生命周期，不把所有长时间操作抽象成
服务。Web、Backend、Dev Server 等本地进程统一视为 Shell 的后台执行模式：

```text
shell
  ├─ foreground：当前 Turn 等待命令结束
  └─ background：返回 task_id，跨 Turn 保持运行
```

运行面板可以统称为“后台任务”，但任务必须明确属于本地 Shell。第一版不引入
`ManagedService`、健康检查、端口声明、通用 Scheduler、Service Registry 或隐式
自动重启策略。

GitHub Action 状态查询、云端构建和部署状态等待等远程长任务不属于本提案。它们
没有本地进程可终止，应另行建模为 `ExternalWaitOperation`：保存远程引用和最近
状态，停止只停止本地轮询，不取消远程操作。

## Shell 接口

现有仅包含 `command` 的 Shell 请求继续表示前台执行。后台模式使用明确的动作：

```json
{
  "mode": "background",
  "action": "start",
  "task_id": "memory-card-web",
  "command": "npm run dev"
}
```

后续操作仍属于 Shell：`status`、`logs`、`stop` 和 `restart`。同一 `task_id` 的
重复启动不创建第二个进程，而是返回已有任务。后台启动只确认进程已创建，不等待
进程退出，因此不占用当前 Turn 的结束、`steer` 或 `interrupt` 等待。

## 内部模型与边界

`BackgroundShellTask` 是 Shell 的跨 Turn 本地进程记录，不是新的业务资源：

```text
task_id, owner_thread_id, state, command_summary, command_hash,
working_directory, process_id, started_at, stopped_at, exit_code,
bounded_log_handle
```

Host/Capabilities 负责进程创建、进程组或 Windows Job Object、有限日志、完整进程树
终止、重复启动保护和 runtime 关闭清理。Core 不新增后台调度器、进程句柄或长期
状态。前台 Shell 的超时、审批、取消和输出行为保持不变。

后台任务归属于创建它的主 Thread runtime：Turn 完成、runtime idle 和页面切换都不
停止任务；主 Thread runtime 明确关闭时停止任务；runtime 异常丢失时终止进程组或
标记为 `lost`。第一版不做 App Server/Gateway 重启恢复。Child Session 可以读取
父 Thread 的任务状态和有界日志，但不能创建、停止或重启任务。

runtime 是否关闭只依据明确的 `closed` 或 `lost` 生命周期信号，不能从 `idle`、
页面状态或连接状态推断。

## App Server、Gateway 与 Web Studio

App Server 提供：

```text
background-task/list
background-task/read
background-task/logs
background-task/stop
background-task/restart
background-task/updated
```

`runtime/status` 仍只描述当前 Turn，不混入后台 Shell 状态。Gateway 只转发主
Thread runtime 的权威结果，不维护第二份进程注册表。Web Studio 运行面板展示任务
ID、状态、PID、工作目录、命令摘要和 hash、运行时长、退出码以及有界日志尾部，
并为主 Thread 提供停止和重启操作，为 Child 显示只读标识。

后续 Turn 修改代码后，由模型显式调用 `restart`。第一版不根据文件变化猜测哪个
任务应该重启。

## 与时间延展提案的关系

保留
`.agents/notes/implemented/architecture/2026-09-17-time-extended-child-session-and-notebook.zh.md`。
该提案描述 Child Session、operation 和时间延展；本提案只补充本地 Shell 进程的
跨 Turn 生命周期，不把它扩展成通用服务管理框架。远程等待任务另行建立
`ExternalWaitOperation` 提案，不复用 `BackgroundShellTask`。

## 六问准入

1. 归属：Capabilities/Host 持有本地进程，App Server 提供 Thread 级控制面，Gateway
   和 Web Studio 只做转发与投影，Core 不改变。
2. 责任复用：复用现有 Shell、ProcessSandbox、Thread runtime、ActionResult 和
   App Server 管道，不新增 ManagedService 层。
3. 删除旧概念：取消 ManagedService、Service Registry、通用 Scheduler 和隐式自动
   重启；远程等待不伪装成本地 Shell。
4. 预算：实现后运行 `python scripts/line_budget.py`；当前 release source 仍需
   保持在 40,000 行硬上限内，并记录实际 delta。
5. 可见面：新增有限 Shell 参数、App Server 方法、SDK/REST DTO 和一条有界更新通知；
   不新增 Core 事件、Session 持久化字段或 Gateway 进程缓存。
6. 证据：Capabilities/App Server 单元测试、App Server 跨 Turn 公共场景、SDK/Gateway
   测试和 Web Studio 构建覆盖公开边界；Child 创建被禁用、runtime 关闭清理有独立
   边界测试。完整的“下一 Turn 显式重启 → Child 读取”组合场景和 runtime 重启恢复
   仍属于后续增强，不作为第一版已实现能力。

## 验收标准

- 前台 Shell 行为不变；后台启动立即返回 `task_id`；
- 后续 Turn 可以读取状态、查看有界日志、停止和重启；
- 任务运行期间 `steer` 和 `interrupt` 不等待任务退出；
- 停止终止完整进程组，同一 `task_id` 不重复启动；
- Child 可以读取父任务，但控制操作被拒绝；
- 主 Thread runtime 明确关闭或丢失时任务被清理；
- 日志不会默认进入信息流；Web Studio 运行面板可操作；
- GitHub Action 等远程等待不会被识别为后台 Shell 任务；
- 跨 Turn、Child 创建拒绝和运行时关闭场景有 bounded Scenario/Eval 或边界测试证据；
- “启动 Web 服务 → 下一 Turn 显式重启 → Child 读取”作为后续组合场景，不提前宣称
  已覆盖。
