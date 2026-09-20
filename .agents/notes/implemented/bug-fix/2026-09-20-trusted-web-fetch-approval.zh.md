# Trusted 模式下的公网 web_fetch 审批修正

- status: implemented
- date: 2026-09-20

## 问题

Trusted 模式下读取通过 URL 校验的公网地址仍会弹出审批。Web Studio 中的
`full_machine + trusted` 配置也无法消除这次审批。

根因是 Capabilities 的 `is_trusted_high_risk` 把 `web_fetch` 固定归类为高风险。
这覆盖了通用 `is_high_risk` 中对 `web_fetch` 的低风险分类。`WebFetch::admission`
已经校验公网 URL 并要求 Host 作审批判定，因此 Trusted 收到待审批请求后又弹出确认。

## 修正

从 Trusted 专属高风险工具列表中移除 `web_fetch`。`ApprovalController` 因此在
Trusted 模式下自动通过已经过 `WebFetch::admission` 的公网请求。Interactive 模式
仍然要求审批。Automatic 模式原本就会按通用低风险分类自动通过。

URL、DNS、IP、凭证和重定向校验没有变化。Security Deny 仍然先于审批策略执行，
Loopback 仍由 `WebFetch::admission` 显式允许。

该修正与 Web Studio 的 Trusted 决策一致：
`mini-agent-web/.agents/notes/implemented/feature/2026-09-14-trusted-low-interruption-execution.md`
记录公网 `web_fetch` 沿用有界 URL 准入，不应新增审批打断。

## 六项变更准入

1. **所属层**：Capabilities 的审批风险分类；Host `ToolOrchestrator` 仍按既有顺序执行准入、审批和工具。
2. **重复职责**：复用 `WebFetch::admission`、`is_high_risk`、`is_trusted_high_risk` 和 `ApprovalController`，没有增加第二条审批路径。
3. **旧概念**：删除 Trusted 风险列表中对 `web_fetch` 的重复强制分类；URL 安全检查仍由 WebFetch 所有。
4. **行数预算**：Rust 有效行数净变化为 0；基线与结果见验证记录。
5. **可见面变化**：不改协议、事件结构、持久化或模型输入。Trusted 下成功的公网请求不再产生审批等待；Interactive 行为不变。
6. **边界验证**：本次没有修改或运行测试。现有 URL admission 测试不覆盖 Trusted 与公网 URL 的组合；格式、Clippy 和行数门禁用于静态验证。

## 验证

- `cargo fmt --all --check`：通过。
- `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`：通过。
- `python scripts/line_budget.py`：通过。Core + Protocol 为 `4,663/6,000`，Control Plane
  为 `26,775/30,000`，Release Rust 为 `39,598/45,000`；相对基线均为 `+0`。
- Web Studio `npm run lint`、`npm run build`：通过；Vite 保留既有 500 kB chunk 提示。
- `python scripts/check_docs_links.py README.md docs` 和 `git diff --check`：通过。
