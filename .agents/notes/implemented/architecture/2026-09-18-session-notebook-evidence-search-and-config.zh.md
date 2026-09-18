# Session Notebook 证据、检索与简化配置

## 状态

已实现。Notebook 仍是 Session-owned 的独立持久化面，不改变 checkpoint
历史，也不把父 Session 内容复制进 Child。

## 决策

- 条目保留 `importance`，并增加有界 `keywords` 和 `evidence`。
- 证据只支持 `commit` 和 `file` 两类。Commit 证据在写入时保存 hash、项目、
  规范化 subject、author/commit 时间和记录时间；读取或检索不再次查询 Git。
- subject 先压缩空白，再限制为最多 160 个 Unicode 字符，并通过
  `subjectTruncated` 保留截断事实。不保存完整 commit body。
- WebStudio 的 Notebook search API 搜索 key、正文、关键词和缓存的证据元数据，
  最多返回 8 条；Mini Agent 运行时仍只暴露有界 Notebook read/write 工具。
- WebStudio 的可调配置只保留两项：`max_entries` 和 `max_entry_chars`。
  默认值分别是 64 和 4096，允许范围分别是 `1..=64` 与 `256..=4096`。
  有效文件上限按条数、单条上限和固定元数据预算计算，再封顶为 64 KiB；
  关键词/证据数量和 subject 长度保持固定硬上限。

## 边界

Mini Agent Capabilities/Host 负责证据字段的校验、规范化、持久化和工具搜索；
App Server 负责 JSON-RPC 接缝；WebStudio 只提供项目/全局配置、检索 API 和 Memory
面板展示。Web 不扫描 Session 文件，也不创建第二份 Notebook 权威副本。

父级 Notebook 继续只读；`forget` 只影响当前 Session 的 Notebook，不删除
checkpoint。证据只是可追溯元数据，不扩大 workspace、shell、approval 或写权限。

## 兼容性与验证

旧 `notebook.json` 缺少 `keywords`/`evidence` 时按空数组读取。旧客户端缺少新
字段时保持原写入行为。覆盖测试包括旧数据反序列化、subject 截断、关键词/commit
检索和旧 SDK 调用。
