# Agent Notes

本目录记录 `mini-agent-harness` 的架构决策、实验依据和被拒方案。它是决策
索引，不是当前产品规格，也不是持续追加的项目日志。

## 生命周期目录

```text
.agents/notes/
├── README.md
├── proposed/     评审中或尚未完成验证的方案
├── implemented/  已落地、仍对当前实现有指导意义的决策
├── rejected/     需要保留其否决理由的方案
└── archived/     已冻结、只供历史追溯的记录
```

文件路径使用 `{lifecycle}/{class}/yyyy-mm-dd-topic-title.md`。`class` 包括
`architecture`、`feature`、`simplification`、`process`、`testing` 和
`bug-fix`。

## 当前决策入口

### Implemented

- [Proposal Quality and Evidence Guide](proposal-quality-and-evidence-guide.md)
- [Core Harness Boundary](implemented/architecture/2026-08-24-core-harness-boundary.md)
- [Hard Limits System](implemented/architecture/2026-08-24-hard-limits-system.md)
- [Session as Single Durable Store](implemented/architecture/2026-08-28-session-single-source-of-truth.md)
- [Runtime Authority and Action Ordering](implemented/architecture/2026-08-30-runtime-authority-and-action-ordering.md)
- [Python SDK and App Server Integration](implemented/architecture/2026-08-31-python-sdk-architecture-and-app-server-integration.md)
- [Codex-Aligned Thread and Goal Runtime](implemented/architecture/2026-09-03-codex-aligned-thread-goal-runtime.md)

### Proposed

- [Codex-Aligned Agent Control Plane](proposed/architecture/2026-09-03-codex-aligned-agent-control-plane.md)
- [Codex-Aligned Agent Control Plane（中文）](proposed/architecture/2026-09-03-codex-aligned-agent-control-plane.zh.md)
- [Codex-Aligned Skills, Plugins, Builtin Tools, and ThreadItems](proposed/architecture/2026-09-01-codex-aligned-capabilities-thread-items.md)
- [Docker Sandbox Isolation Policy](proposed/architecture/2026-08-31-docker-sandbox-isolation-policy.md)
- [CLI Through App Server](proposed/architecture/2026-08-28-cli-through-app-server-unified-runtime.md)
- [Stabilization and Evidence Gates](proposed/process/2026-08-27-stabilization-and-evidence-gates.md)

Other records remain discoverable under their lifecycle directory but are not
listed here unless they are active reference points.

## 维护规则

- 当前行为写入 `docs/` 的主题文档；决策理由写入本目录的一篇主题记录；
- 一个主题只保留一个当前决策入口，不在旧记录末尾追加下一轮工作日志；
- 提案完成后移动到 `implemented/` 并改写为已实现的事实；被否决的方案只在
  仍能阻止重复误入时保留；
- 已进入 `archived/` 的文件冻结，不再翻译、重排或追加内容；
- 不在本索引复制完整方案、测试输出、当前限制或用户操作步骤。
