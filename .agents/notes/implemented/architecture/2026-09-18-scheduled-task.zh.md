# 跨 Turn 延时标记

- status: implemented
- date: 2026-09-18

## 决策

`scheduled_task` 表示远程状态查询的有界延时标记。它不是 Scheduler，不会等待、
结束当前 Turn、唤醒或续跑 Thread，也不会查询或取消远程任务。只有用户或 Host
显式启动后续 Turn 后，Agent 才能读取标记并调用对应的远程状态工具。

本地长驻进程使用 Shell 的后台模式；远程 GitHub Action、云构建或部署等待使用
延时标记。两者分别表示本地进程生命周期和后续检查时间，不合并为通用任务调度器。

## 工具契约

支持 `create`、`list`、`read` 和 `cancel`。`create` 必须提供唯一稳定的
`task_id` 与 `delay_seconds`，延时为 1 秒至 24 小时；可选 `summary` 描述后续检查。
重复 `task_id` 返回原标记，不会改写到期时间。

`scheduled`、`ready` 和 `cancelled` 描述本地标记状态。运行时没有计时 worker；到期后，
标记会在下一次创建、列表、读取或取消操作时转为 `ready`。这只表示延时已到，不表示
远程操作完成。`cancel` 只取消本地标记。Child Session 可以读取父 Thread 标记，但
不能创建或取消。

标记只保存在当前 runtime 内存中，runtime 关闭后清理。Gateway 和 Web Studio 只投影
App Server 的权威状态，不维护第二份记录。

## 落地与规范

实现由 Harness 提交 `91ab580` 建立，工具契约与误用边界由 `8e77e7b` 校正，
WebStudio 与 SDK 文档和接口对齐见 `e8f5fb8`。当前协议与使用说明见
`docs/app-server.md`、`docs/harness-tool-surface.md` 和 WebStudio 的
`docs/scheduled-tasks.md`。

第一版不包含自动唤醒、模型续跑、Webhook、供应商适配器、远程取消或 runtime 重启恢复。
这些边界属于当前契约，不应描述成已经实现的能力。
