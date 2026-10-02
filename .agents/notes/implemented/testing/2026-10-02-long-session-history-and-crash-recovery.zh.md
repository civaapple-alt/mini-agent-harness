# 长会话历史渐进加载与重启恢复证据

状态：implemented（已本地提交；尚未推送）

日期：2026-10-02

关联决策：[Session 超长工具输出恢复、模型耗时与 Plan Mode 工具约束](../architecture/2026-10-01-context-engineering-session-recovery-and-plan-tools.zh.md)

## 决策

Web Studio 打开 Session 时只请求 `thread/items/list` 最新一页，每页最多 128 项。用户向上滚动时按 cursor 加载旧页。分页结果按 ThreadItem 身份合并。重连后若新旧最新页存在共同条目，Studio 据新增条目数平移已加载页和 cursor；若没有共同条目，bounded 响应不足以推算安全偏移，因此 Studio 回到最新一页。

聊天时间线使用 TanStack Virtual 测量可变高度行，并只挂载视口附近的行。Turn 导航栏单独虚拟化。读取旧页时，Studio 根据虚拟列表中的消息起点恢复滚动偏移；用户跳转到未挂载的 Turn 时，Studio 先滚动并挂载目标行。

超长工具输出继续保存在 Session sidecar，并通过 `read_tool_output` 句柄分页读取。WebSocket 继续只传预览和句柄。SessionStore 和 App Server 仍是持久化历史及恢复状态的权威；本轮没有新增接口、Session 格式或另一份历史缓存服务。

Scripted 场景验证了 50 轮内 Core 的 `tool_manifest_hash` 与 Provider 工具定义序列化不变；Plan Mode 改变 `allowed_tools` 时，工具定义仍相同。Shell 退出码 127 的错误内容和 `is_error` 会留在模型历史中，下一次 scripted 响应据此改用只读检查。重复调用继续使用现有软警告，不增加工具屏蔽或自动熔断。

独立子进程运行 App Server 和 scripted Model，写入完整 Turn 后由父测试强制终止进程。父测试用同一 Session 目录重新打开 Session，检查 Journal 中的 Turn、ThreadItem、model timing 与 Provider usage，并通过重新启动的 App Server 分页读取原工具输出句柄。Gateway 的持久化读取测试也检查了 `SessionCatalog.read_thread` 对 timing 和 usage 的投影。

## 验证证据

- Rust：`cargo test -p mini-agent-core -p mini-agent-capabilities -p mini-agent-app-server --quiet` 通过。App Server 92 项、Capabilities 162 项、Core 55 项测试通过；强杀场景启动的隔离子测试也通过。受影响包的 all-targets Clippy 和 `cargo fmt --all` 通过。
- Web：`npm test` 通过，Node 测试 94 项、Vitest 193 项。前端 lint、生产 build 和 Gateway 持久化读取定向测试通过。Ruff 和两个仓库的文档链接检查通过。
- `python3 scripts/line_budget.py` 通过：Core + Protocol 为 6,395 / 7,000 行，Control Plane 为 39,249 / 45,000 行，Release Rust 为 56,601 / 65,000 行。
- 全部场景使用 scripted provider，没有发起付费模型调用。

## 限制

确定性场景证明分页、持久化和恢复边界，不代表真实模型成功率或缓存成本已经改善。本轮没有运行真实 Provider 评测、完整 Python Gateway/SDK 测试或手动浏览器 smoke。前端生产构建仍提示约 836 kB 的 JS chunk 超过 500 kB 建议值；Vitest teardown 会输出未完成 fetch 的 Happy DOM `AbortError`，测试进程退出码为 0。
