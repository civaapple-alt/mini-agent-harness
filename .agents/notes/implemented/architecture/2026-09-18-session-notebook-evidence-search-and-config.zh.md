# Session Notebook 证据、检索与简化配置

## 状态

已实现。Notebook 仍是 Session-owned 的独立持久化面，不改变 checkpoint
历史，也不把父 Session 内容复制进 Child。

## 决策

- 条目保留 `importance`，并增加有界 `keywords` 和 `evidence`。
- 证据只支持 `commit` 和 `file` 两类。它是调用方声明的有界来源元数据，写入时
  保存 hash、项目、规范化 subject、author/commit 时间和记录时间；实现不宣称已
  自动向 Git 或文件系统重新校验，读取或检索也不再次查询 Git。
- subject 先压缩空白，再限制为最多 160 个 Unicode 字符，并通过
  `subjectTruncated` 保留截断事实。不保存完整 commit body。
- WebStudio 的 Notebook search API 搜索 key、正文、关键词和缓存的证据元数据，
  最多返回 8 条；Mini Agent 运行时仍只暴露有界 Notebook read/write 工具。
- WebStudio 的可调配置只保留两项：`max_entries` 和 `max_entry_bytes`。
  默认值分别是 64 和 4096，允许范围分别是 `1..=64` 与 `256..=4096`。
  有效文件上限按条数、单条上限和固定元数据预算计算，再封顶为 64 KiB；
  关键词/证据数量和 subject 长度保持固定硬上限。Runtime 只接受
  `max_entry_bytes`，不保留旧字段别名或旧环境变量。

- Notebook 写入和遗忘会发送只包含 `revision` 与 `changedKeys` 的
  `session/notebook/updated` 通知；前端收到后重新读取权威投影，不把正文复制到
  事件。`lastUsedAtMs`、自动过期、权重衰减和 Git 自动验证仍是 deferred。

## 边界

Mini Agent Capabilities/Host 负责证据字段的校验、规范化、持久化和工具搜索；
App Server 负责 JSON-RPC 接缝；WebStudio 只提供项目/全局配置、检索 API 和 Memory
面板展示。Web 不扫描 Session 文件，也不创建第二份 Notebook 权威副本。

父级 Notebook 继续只读；`forget` 只影响当前 Session 的 Notebook，不删除
checkpoint。证据只是可追溯元数据，不扩大 workspace、shell、approval 或写权限。

## 数据约束与验证

Notebook 写入统一使用当前字段模型；缺失的 `keywords`/`evidence` 按空数组处理。
覆盖测试包括 subject 截断、关键词/commit 检索和 SDK 当前调用。
