# 项目 Skill 可编辑性、来源与模型调用

状态：implemented；日期：2026-10-06；范围：Capabilities、App Server、Gateway、Web Studio

## 决策

项目 `.agents/skills` 内的 Skill 文件按普通工作区文件处理。`skill_read_roots` 只增加受控读取能力；它本身不授予写入。项目 Skill 位于既有写入根时，`apply_patch` 仍走工作区路径校验、审批和沙箱。项目外的个人目录和内置 Skill 根仍只读。

Skill 目录项增加可选 `origin`，标记 builtin group、两类个人目录、项目目录或插件来源，不暴露绝对路径。增加 `modelInvocable` 表达模型能否从 metadata-first 目录选择该 Skill。`disable-model-invocation: true` 只关闭自动选择，不影响 Skill 的启用状态，也不阻止用户用 `$skill` 显式调用。字段缺省按可主动选择处理，以兼容旧 Skill 元数据。

只有启用且可主动选择的 Skill 才进入模型目录。模型按需读取匹配的 `SKILL.md`，不预加载整个技能组。面板仍显示仅手动调用的 Skill，并提供规范 `$` 调用名。组级调用只在 Skill 确实属于内置组时出现。pstack 的 `principle-*` 已允许模型选择；`bro` 和 `technical-writing` 仍只支持手动调用。

## 边界与原因

Capabilities 负责发现、来源分类和写入授权；App Server 将有界目录投影到协议；Gateway 透传目录；Web Studio 搜索、筛选和插入调用名。前端不扫描目录，也不根据 `source` 字段猜测组身份。

这保留了项目文件已有的工作区编辑权限，同时不把读取个人或内置 Skill 的能力扩展成外部写入权限。显式调用与模型主动选择是两个独立入口。

## 验证证据

- `workspace_tests.rs` 覆盖项目 Skill 的更新、新建和删除，并验证项目外 Skill 仍遵守原有写入边界。
- `skills_tests.rs` 覆盖来源分类、元数据标记、缺省值、UTF-8 截断前缀和模型可见目录。
- App Server 场景覆盖模型按需读取匹配 Skill、未匹配任务不读取，以及 `$skill` 显式调用。
- 相关 Harness 提交：`c84daee`、`6e49d2e`、`3056e2e`、`a9e0f26`。Web Studio 对应提交：`32016ae`、`ed8d298`、`c7479d5`、`17a3047`。
- `a9e0f26` 的包测试、Clippy、格式检查、Web 测试、lint 和 build 在该实施批次通过。本文补录没有重跑这些检查。

## 后续

模型是否在匹配任务中主动选择 Skill 仍取决于模型行为。已添加的确定性场景验证了读取边界和事件，不等同于多任务、多模型的触发率评估。
