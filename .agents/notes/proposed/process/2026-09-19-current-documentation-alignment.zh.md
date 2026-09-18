# 当前文档与运行时对齐

- status: proposed
- date: 2026-09-19

## 决策

将稳定行为、用户操作和公开协议写入 `README.md`、目录 README 与 `docs/`。将批次
过程、过期结论、实现前后的取舍和验证记录写入 `.agents/notes/`。稳定文档不再保留
“升级记录”、六问回填或已被代码取代的未来时描述。

本批次以 App Server Protocol、Host/Capabilities 的运行时实现，以及 Gateway/Studio
已消费的契约为事实来源。文档从权威层向外更新，顺序为 Core/Host、App Server、SDK/
Gateway、Web Studio。遇到无法由代码或公开测试确认的主张时，删除主张或在 notes
中保留为待验证事项，而不把推测写成当前行为。

## 已观察的问题

| ID | 观察 | 证据 | 影响 |
| --- | --- | --- | --- |
| DOC-01 | `docs/world-state.md` 仍称 Child Session、operation recovery 和 Notebook 未实现。 | `mini-agent-capabilities` 的 child/notebook 工具、App Server RPC、Gateway child/notebook 路由。 | 使用者会误判长任务恢复和委派边界。 |
| DOC-02 | `docs/harness-tool-surface.md` 保留一次工具迁移的决策与六问。 | 当前默认工具和 App Server settings 已稳定。 | 历史实施过程与当前契约混在一起。 |
| DOC-03 | `docs/harness-lessons-history-2026-08-31.md` 位于稳定文档目录。 | 文件是冻结的 2026-08-31 过程记录。 | `docs/` 的当前规范职责不清。 |
| DOC-04 | Web 侧 limits/privacy 使用旧的实现假设和重复的产品定位。 | SDK、Gateway、Studio 代码与 App Server current contract。 | Web 用户难以判断哪些限制由客户端实施，哪些由运行时实施。 |

## 验收

- `docs/` 索引只列当前主题文档，不再列历史运行日志。
- 当前状态文档说明 App Server、Host/Capabilities、Gateway 和 Studio 的唯一权威。
- 历史文档移动到 notes 生命周期目录，并冻结内容。
- Web 文档只陈述能由 Web/SDK 代码或 App Server 公开契约验证的限制和数据流。

## 非目标

- 不在本文档批次改变 JSON-RPC、持久化格式、工具策略或 UI 行为。
- 不把每一份旧 notes 重写成当前规格；已归档记录保持冻结。

## Worktree 验证

- `docs/` 已将运行时架构、World/Session 状态、Builtin 工具和证据规则改为当前
  规格；旧的 2026-08 过程记录与 2026-09 证据批次已移到 `archived/`。
- CLI 文档已改为不带子命令的 `mini-agent` 进入实验性 REPL，避免把不存在的
  `mini-agent repl` 写成命令。
- world execution 更新已按 `RuntimeActor::update_world` 的
  `replace_context_and_persist(..., "world_state", ...)` 说明为上下文槽替换。
- 验证命令：`cargo test -p mini-agent-cli --bin mini-agent`、`cargo fmt --all --check`、
  `python scripts/line_budget.py` 与 `python scripts/check_docs_links.py README.md docs examples`。
