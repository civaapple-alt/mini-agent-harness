# Agent Notes

`.agents/notes/` 是 `mini-agent-harness` 的决策记录区，保存架构决策、提案、
实验依据、被拒方案和可复用的工程过程指南。它是对“为什么这样做、如何证明”
的全局记录，不是当前产品规格，也不是持续追加的项目日志。

当前产品行为、稳定配置和运行手册分别以 `docs/` 及各目录 README 为准；本目录
中的记录不能反向替代这些规范。历史记录可以帮助追溯，但不能被当作当前实现。

## 生命周期目录

```text
.agents/notes/
├── README.md
├── proposed/     评审中或尚未完成验证的方案
├── implemented/  已落地、仍对当前实现有指导意义的决策
├── rejected/     需要保留其否决理由的方案
└── archived/     已冻结、只供历史追溯的记录
```

生命周期记录的文件路径使用 `{lifecycle}/{class}/yyyy-mm-dd-topic-title.md`。`class` 包括
`architecture`、`feature`、`simplification`、`process`、`testing` 和
`bug-fix`。

架构、功能、实验和决策记录只属于一个生命周期目录：

- `proposed/`：正在评审或尚未完成证据验证的提案；
- `implemented/`：已落地且仍对当前实现有指导意义的决策；
- `rejected/`：被否决但仍值得保留其否决理由的方案；
- `archived/`：已冻结、只供历史追溯的记录。

根目录只保留少量跨生命周期的过程指南。过程指南是如何起草、实施和验证的
工作规则，不是某个功能的实现记录，也不随单个提案移动到 `implemented/`。

README 不维护“当前决策列表”，也不把某一篇记录提升为隐含总规范。需要查找
记录时，先按生命周期和 class 定位，再通过文件标题、`rg` 或 Git 历史检索。

## 过程指南

- [Proposal Quality and Evidence Guide](proposal-quality-and-evidence-guide.md)：
  提案起草、批次实施、跨仓同步、证据验证和状态晋级的可执行工作手册。

## 维护规则

- 当前行为写入 `docs/` 的主题文档；决策理由写入本目录的一篇主题记录；过程
  指南可以放在 notes 根目录，但必须说明它是过程规则而非产品规范；
- 一个主题只保留一个当前决策记录，不在旧记录末尾追加下一轮工作日志；
- 提案完成后移动到 `implemented/` 并改写为已实现的事实；被否决的方案只在
  仍能阻止重复误入时保留；
- 已进入 `archived/` 的文件冻结，不再翻译、重排或追加内容；
- 不在 README 复制完整方案、测试输出、当前限制或用户操作步骤；README 只
  解释 notes 的用途、生命周期、命名和维护方式。
