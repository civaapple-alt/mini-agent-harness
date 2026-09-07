# Cargo Boundary Audit

* **日期**: 2026-09-07
* **状态**: implemented
* **Class**: architecture
* **范围**: workspace Cargo 依赖方向与 `App Server → Capabilities` review edge

## 结论

当前 workspace 依赖是无环的单向 DAG，符合现有职责顺序：

```text
Protocol → Core → Capabilities → Host → App Server → CLI
                 ↘ App Server Protocol →
```

本轮没有证据表明必须重构 Cargo crate 关系。`mini-agent-app-server →
mini-agent-capabilities` 保留为 review finding，而不是违规边：App Server 的
`AppServerRuntime` 需要消费 provider/session 类型，Verifier 也需要具体 provider；
但工具、policy、world 和 workflow composition 仍由 Host 组装，Sandbox 和权限匹配
没有在 App Server 中复制实现。

## 审计证据

- `runtime.rs` 使用 Capabilities 的 provider、session 和 image seam，负责把已经
  组装好的 runtime 绑定到 App Server 服务；它没有重新实现 ToolRuntime 或 policy。
- `runtime_actor.rs` 通过 Capabilities 的 `ApprovalController`、MCP loader 和
  `SecurityPolicy` 处理运行时命令；具体工具和 workspace side effect 仍在 Host/
  Capabilities。
- `management.rs` 只持有 session/MCP 配置快照和 bounded action projection；没有
  第二份 Session authority。
- `frontend.rs` 对外只暴露 App Server-owned wrapper，Capabilities controller 的
  具体类型不进入 frontend API。

验证命令：

```text
python scripts/cargo_boundary.py --json
cargo clippy --workspace --all-targets --locked -- -D warnings
```

结果：Cargo boundary `pass`，仅报告既有 review edge；workspace Clippy 通过。

## 保持“不拆包”的条件

只有出现以下证据，才另立 Cargo 重构提案：

1. App Server 和 Host 同时维护同一份 policy、Sandbox、Session 或 recovery authority；
2. 为绕过边界而新增反向依赖、循环依赖或重复 DTO/adapter；
3. provider/session seam 已稳定但仍被多个上层直接复制消费，且 Host facade 能删除旧
   依赖而不是新增胶水层；
4. 跨仓 Web/SDK/Studio 契约要求一个独立、稳定且不依赖具体 provider 的 boundary crate。

在这些条件出现前，依赖方向检查用于防回归，line gate 的路径统计不改变 Cargo 所有权。
