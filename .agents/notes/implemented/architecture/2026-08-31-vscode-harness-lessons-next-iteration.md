# Harness 经验与下一迭代准入：拆分索引

Status: frozen index

这份文件曾经同时承载框架对比、场景证据、六项准入记录、工具边界和多批
实现历史，已经不适合作为项目级的持续更新入口。现在它只保留阅读地图和
文档治理规则；原文已移动到
[`2026-08-31-harness-lessons-archive.md`](2026-08-31-harness-lessons-archive.md)
作为不可追加的历史归档。

## 阅读地图

| 主题 | 当前文档 | 内容边界 |
| :--- | :--- | :--- |
| 框架比较 | [`2026-08-31-harness-framework-comparison.md`](2026-08-31-harness-framework-comparison.md) | mini-agent-harness、Codex 原生框架、Turn/Step、steer/cancel 和成熟度判断 |
| 场景与证据 | [`2026-08-31-harness-scenario-evidence.md`](2026-08-31-harness-scenario-evidence.md) | bounded scenario baseline、工具失败矩阵和可审计 evidence 规则 |
| 边界与准入 | [`2026-08-31-harness-boundary-admission.md`](2026-08-31-harness-boundary-admission.md) | Core/Host/Capabilities/App Server 分工、六项准入、Docker/provider 等 deferred 决策 |
| 实现维护历史 | [`2026-09-03-harness-maintenance-history.md`](2026-09-03-harness-maintenance-history.md) | 已完成批次的索引、line budget 和后续缺口 |
| Builtin 工具升级 | [`2026-09-03-harness-tool-surface-upgrade.md`](2026-09-03-harness-tool-surface-upgrade.md) | 少量默认工具、分页读取、预校验 patch 和 App Server/Web 对齐 |
| 完整历史 | [`2026-08-31-harness-lessons-archive.md`](2026-08-31-harness-lessons-archive.md) | 拆分前原文，供考古和核对，不作为新的变更日志 |

## 核心结论

模型只是引擎，harness 负责组装模型可见上下文、声明能力、校验并执行工具、
以及决定继续还是结束。mini-agent-harness 的目标是小型、显式、可替换且可
观察的执行内核，不是复制 Codex 或 VS Code 的全部对象、工具和扩展生态。

后续项目级变更必须新建有明确日期和主题的 note，放在
`.agents/notes/implemented/architecture/` 或相应的 `.agents/notes/` 分类下。
本索引、`.agents/notes/README.md`、仓库 `README.md` 和 `CHANGELOG.md` 只做
索引或当前状态摘要，不再把新的实现批次追加到本文件或历史归档。

## 拆分说明

- 原文的框架对比和成熟度结论整理到 framework comparison；
- bounded scenarios、评审意见和 failure/timeout/retry 矩阵整理到 scenario evidence；
- 六项准入模板、工具执行边界、自动化顺序和 Docker 政策门整理到 boundary admission；
- 原文第 5 节的已实现批次改为维护历史索引，具体细节仍在 archive 中保留；
- 后续新能力（例如 2026-09-03 Builtin 工具升级）使用独立日期 note。
