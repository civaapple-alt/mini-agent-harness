# WebStudio 会话体验文档回填

- status: implemented
- date: 2026-09-20

## 背景与范围

近期 Plan 文件定位、Turn 阅读布局、block 折叠、后台任务和子代理协作都经历了多次修正。提交历史中已有实现，但当前规范页没有及时反映全部最终行为。本记录按责任边界索引相关提交和规范入口，不重复维护操作细节；当前行为以各仓库 docs/ 页面为准。

## 提交与规范入口

| 主题 | 关键提交 | 当前规范 |
| --- | --- | --- |
| Plan Mode 与计划页路径、刷新 | Harness 0f7a8cd、0bf5189；WebStudio 7faf8b3、dc78c45、af85c51 | Harness docs/configuration.md、docs/studio-integration.md；WebStudio docs/troubleshooting.md |
| Turn rail、回到底部与正文布局 | WebStudio 28ac7b2、31cc42d、3582d59、932feb8、7107218、bfe12c9 | WebStudio docs/troubleshooting.md |
| 活跃/已完成 block 与最终回复 | WebStudio 0e41c22、a5a14bb、2a5453f、ec7d239 | WebStudio docs/troubleshooting.md |
| 后台 Shell 与延时标记 | Harness 1fe02fa、91ab580、8e77e7b；WebStudio a497f56、04b59ea、e8f5fb8 | Harness docs/app-server.md、docs/harness-tool-surface.md；WebStudio docs/background-tasks.md、docs/scheduled-tasks.md |
| 子任务批次、消息流、侧栏与动态控制 | Harness 8fd78f5；WebStudio 815a89e、9f830dc | Harness docs/app-server.md、docs/studio-integration.md、docs/privacy.md；WebStudio docs/child-tasks.md |

这些提交均来自相邻仓库的本地 Git 历史。短哈希可在对应仓库用 git show <hash> 核对；它们是实现证据索引，不替代稳定规范。

## 文档维护决策

- UI 与运行手册按主题放在 WebStudio docs/；App Server 能力、持久化和跨仓权威边界放在 Harness docs/。
- 已完成实现的后台 Shell 和延时标记记录从 proposed/ 晋级到 implemented/。定时任务统一称为“延时标记”，不再描述成自动唤醒或自动结束 Turn。
- 子代理动态协作实现已合并，但活跃 Turn、顺序组失败恢复和 Gateway 重启等验收证据仍未齐全；对应 note 继续留在 proposed/，不把“代码已提交”当作全部验收完成。
- 消息流策略按单个 block 推进：活跃 block 展开，后续 block 开始时折叠前项；最终回复结算后保持展开。

## 本次核对

本次只更新规范、索引、变更记录和已实现决策记录，没有修改运行时代码。验证方式为检查相关提交、git diff --check 和文档链接；没有运行功能测试。
