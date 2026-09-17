# 全局 Skill 发现、按需加载与阶段事件

状态：implemented

## Decision

Mini Agent 只扫描四类受控 Skill 根：项目 `.agents/skills`、用户
`%USERPROFILE%/.agents/skills`、用户 `%USERPROFILE%/.mini-agent/skills`，以及
`%USERPROFILE%/.mini-agent/skills/builtin/<group>` 下的内置组。扫描只进入直接
子目录并要求 `SKILL.md`，不递归用户目录。优先级固定为：project、Agent Skills
user、Mini Agent user、builtin group、plugin。未限定分组的同名 Skill 按优先级
覆盖并记录 shadowing；pstack 使用 `pstack:<skill>`，因此可以和无分组 Skill
共存，短名在冲突时拒绝解析。

会话提示只注入有界 metadata。有效 Skill 的实际根目录单独进入
`skill_read_roots`，不把整个用户目录或 builtin 根加入普通 `extra_read_roots`。
因此 `read_file` 可以按需读取启用 Skill 下的 `SKILL.md`、references、scripts、
assets 和说明文件，但不会因此取得写入或脚本执行权限。

## Observable contract

Host 归因普通 Turn、`+` 工作流和 `$` 显式激活的 `SKILL.md` 读取。首次读取前
发 `skills_loaded(phase=started)`，成功后发 `phase=loaded`；失败使用既有
`skills_load_failed`。同一 Turn 按 qualified name 去重，关联资源读取不再产生
额外技能事件。事件继续复用 Core sequence、item_id、SSE、WebSocket 和 replay。
旧事件缺少 `phase` 时按 `loaded` 解释。

## Why

全局目录发现让 Skill 的可用范围与用户已有 Agent Skills 一致，同时保留
Capabilities/Host 的单一授权来源。阶段事件把“模型决定使用”与“读取成功”分开，
避免把每次普通文件读取误报成技能加载，也让 WebStudio 能用一个轻量状态行展示
进行中、完成和失败。

## Non-goals

不递归扫描任意用户目录，不预加载无关正文，不为 references/scripts/assets 增加
独立工具或权限类型，不执行 Skill 脚本，也不启用 Cursor 专属命令、hooks、MCP、
多模型或并行 Agent。
