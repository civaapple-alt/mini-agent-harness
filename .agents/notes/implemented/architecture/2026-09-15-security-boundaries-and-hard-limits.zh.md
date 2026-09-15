# 安全边界与硬限制证据

状态：implemented  
Date: 2026-09-15  
Batch: Iteration 11，Plan Mode、WebFetch resolver 与 context compaction hard limits  
Scope: mini-codex Core/Capabilities 边界，以及 mini-agent-web 的消费契约

## Decision

Plan Mode 的变更路径继续由各自的 typed admission 拦截：ApplyPatch 返回
`Deferred`，Shell mutation 返回 `ApprovalRequired`，MCP tool call 返回
`Deferred`；只读 Shell、工作区和 extension-root 读取仍可执行。当前公共工具目录
没有 `SpawnAgent`，因此本批不伪造不存在的覆盖；未来加入时必须先定义 child
session、路径和审批 owner。

`WebFetch` 在 DNS 解析后固定 origin endpoint，要求所有解析地址符合已准入的
public/loopback class；redirect 只能留在同一 host 和同一 class。公共 URL 进入
Host approval，显式 loopback URL 作为本地允许目标；公共域名不能通过解析绕到
loopback、私网或 cloud metadata 地址。

Context compaction 使用 UTF-8 安全截断的辅助 user prompt，并让 prefix、摘要、
最近 tail 以及 system/tool 请求统一经过既有 context byte accounting。即使配置为
0、1 或其他很小的 `max_user_input_bytes`，稳定的 compaction prompt 也不能越过
用户输入限制。

## Harness hypothesis

如果每个 mutation 和外部网络入口都在副作用前完成 typed admission，并且 compaction
的辅助输入也服从同一硬字节上限，那么 Plan Mode、SSRF 防护和小配置值不会依赖
诊断文本或客户端推断；核心控制流在 Shell、MCP、WebFetch 和上下文压缩之间保持
可观察且可回放的边界语义。

## Ownership and boundaries

- Core 负责 loop、context accounting、compaction 消息和 observation event，不负责
  文件、进程、网络或审批。
- Capabilities 负责工具参数 admission、resolver 地址分类和具体副作用结果。
- Host 负责 approval ordering 与 grant authority；不复制 WebFetch、Plan Mode 或
  retry 的授权状态。
- App Server 负责公共事件和 Thread Item 投影。
- SDK、Gateway、Web Studio 只消费并展示结构化结果；本批没有新增前端授权、本地
  执行或第二套 Gateway 状态机。

## Cross-repository contract

`ThreadItem.status` 继续表示生命周期，`ThreadItem.outcome` 继续表示工具结果；
未知 outcome 仍由 SDK、Gateway 和 Web Studio 透传。Web 侧只验证消费和展示，不把
resolver、Plan Mode 或审批规则复制到浏览器。

## Verification

- `cargo fmt --all`：通过。
- `cargo test -p mini-agent-core --lib`：43 passed。
- `cargo test -p mini-agent-capabilities --lib`：80 passed。
- CLI public scenarios：typed file/mutation gate 1 passed；非 TTY Shell
  fail-closed approval 1 passed。
- `cargo test -p mini-agent-app-server --lib tests::projects_mcp_timeout_through_public_app_server`：1 passed。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `python scripts/line_budget.py --base e6688ad --check-delta --json`：无 violations；
  current runtime `20554`、release `31234`、control-plane `21842`，增量分别为
  `+17`、`+55`、`+0`，均为 green。
- `git diff --check`：通过；仅有 Windows 工作树 LF/CRLF 提示。
- 未调用真实付费 provider；WebFetch 使用受控 resolver/redirect fixture，Harness
  evidence 已写入 `docs/harness-evidence.md`。

## Consequences

- Plan Mode 的 mutation 不会因为客户端或错误文本分类失守。
- WebFetch 的安全分类发生在网络副作用前，redirect 不能改变准入 class。
- compaction 的固定提示不再绕过极小 user-input 配置，且不会切断 UTF-8 字符。
- Batch 12 的长期运行产品面继续延后，等待 EOF、权限和结果契约在更长运行中
  获得独立证据。

## Remaining risks

- resolver 证明使用受控地址 fixture，尚未替代真实恶意 DNS、跨平台网络栈和生产
  resolver 的测试。
- 当前工具目录没有 SpawnAgent 公共路径，因此其 child-session 和权限契约仍未
  验证。
- 未运行完整 workspace test，也未进行真实 provider 质量验证；Docker 隔离、CLI
  统一经 App Server、Goal verifier 和长期 Bot Agent 属于 Batch 12。
