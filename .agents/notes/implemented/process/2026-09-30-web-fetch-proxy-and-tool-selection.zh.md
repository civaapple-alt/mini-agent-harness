# Web Fetch 本地代理与搜索工具可见状态

- status: implemented
- date: 2026-09-30

## 决策

`web_fetch` 使用 App Server 进程的 HTTP、HTTPS、SOCKS 代理环境变量，并尊重
`NO_PROXY`。直连请求继续固定已验证的目标 IP。公网域名仅解析到 Clash
`198.18.0.0/15` fake-IP 时，只有匹配的代理存在且 `NO_PROXY` 未排除该域名，才把
hostname 交给代理解析。Loopback 始终直连；其他非公网或混合 DNS 结果仍拒绝。

Host 构建运行时时保存实际应用的 Builtin tool selection，并交给 App Server
RuntimeManagementState。配置有效 `web_search` provider 与 key 时，Host 默认同时启用
`web_fetch`。Settings RPC 因此返回与模型工具目录一致的初始选择，Web Studio 不再把
实际可用的 `web_fetch` 显示成禁用。

## 六项准入

1. **归属：** 网页网络副作用归 Capabilities `WebFetch`；初始工具选择归 Host，状态投影归 App Server。
2. **已有 owner：** 复用 `WebFetch` 的 URL、DNS、redirect 和 approval 边界，复用 `BuiltinToolSelection` 与 Runtime settings RPC。
3. **替换旧概念：** 不增加代理配置协议。Host 只把已经施加到 Harness 的初始选择传到 RuntimeActorState，移除“重新使用默认值”的错误投影。
4. **行数：** Core + Protocol 无变化；Control Plane 与 Release Rust 的实际增量见验证命令。
5. **模型可见面：** 已有 `web_fetch` tool schema 不变。有效搜索配置会让 Host 与 settings RPC 一起暴露它；代理只影响 Host 的网络路径，不扩大 URL admission。
6. **边界证据：** Capabilities resolver fixtures 覆盖 fake-IP、私网、混合地址和 `NO_PROXY`；App Server JSON-RPC scenario 验证设置返回和模型可见工具；Gateway fixture 验证保留 Host 的选择。

## 有界场景

- **假设：** 搜索 provider 和 key 已配置时，新 Thread 的设置响应会报告 `web_fetch`，模型可以调用它并收到页面结果。
- **公共路径：** App Server `thread/settings/update`、`turn/start`；Gateway `GET /api/workflows/state`。
- **刺激与轨迹：** Mock model 检查 `web_fetch` schema，调用 fixture tool；第二次模型请求读取已完成工具结果。
- **结算：** 工具执行一次，Turn 完成；无需公共网络或付费 provider。
- **失败反例：** 无代理的 fake-IP、代理排除后的 fake-IP、私网或混合解析结果均不准入。
- **缺口：** 本地 Clash 实际转发和第三方 provider 未调用；验证使用确定性地址 fixture。

## 验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server --all-targets -- -D warnings`：通过。
- `cargo test -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server -- --test-threads=1`：通过。串行运行隔离了仓库中共享临时目录的测试。
- Web Studio：`uv run ruff check tests/gateway/test_workflow_tool_selection.py` 通过；`uv run pytest -q tests/gateway/test_workflow_tool_selection.py` 通过（1 项）。
- `python3 scripts/cargo_boundary.py --json`：通过，0 项违规；报告保留既有 App Server 到 Capabilities 的 review finding。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：通过，无违规。Core + Protocol `6,101/6,500`（增量 0），Control Plane `36,157/38,000`（增量 170），Release Rust `53,143/55,000`（增量 334）。
- `git diff --check`：通过。

未调用公共网页或付费 Provider；Clash 转发路径以可重复的 fake-IP 地址夹具验证，仍需在用户本机实际代理环境中确认端到端访问。
