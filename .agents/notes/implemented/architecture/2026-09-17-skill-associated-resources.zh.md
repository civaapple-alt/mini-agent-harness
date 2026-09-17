# Skill 启用目录只读放行

Status: implemented
Date: 2026-09-17
Scope: mini-agent Capabilities、Host 和 WebStudio 文档

## 决策

已启用 Skill 的根目录作为 Host/Capabilities 的受信任只读根。模型可以在当前
Turn 中通过现有 `read_file` 查看该目录下的 `SKILL.md`、`references/`、
`scripts/`、`assets/` 和其他文本文件，不需要为每种资源增加单独的权限模型。

这与 [Agent Skills 规范](https://agentskills.io/specification) 的渐进式披露
保持一致：发现阶段只解析 `SKILL.md` 的 bounded metadata，显式 `$skill` 加载
Skill 正文，关联资源由模型按需读取。

自动放行只代表读取：

- `read_file` 和受信任 Skill 根下的 `read_image` 可以直接读取；
- `apply_patch` 不会因为 Skill 根而获得写权限；
- 脚本源码可以查看，但执行仍需要 `shell`、approval policy 和 sandbox；
- 不递归预加载资源，不把资源路径或正文放进 manifest、事件或 WebStudio。

## 实现

`Discovery::skill_read_roots()` 从启用 Skill 的 `SKILL.md` 父目录派生 canonical
根目录。Host 在 `ToolBuildRequest` 中把这些目录传给 Capabilities，Workspace
将其与普通 `extra_read_roots` 分开保存。`ensure_readable` 统一执行 canonical
路径和根目录 containment 检查，因此读取能力不会改变写入路径判断。

`read_file` 使用现有分页和 UTF-8 校验，并按当前 `ToolExecutionContext.turn_id`
累计 Skill 目录产生的 rendered output。单个 Turn 的 Skill 读取总量上限为
64 KiB；普通工作区文件不计入该预算。未携带 Turn context 的直接 Harness 调用
保持原有行为，App Server 的模型工具请求会携带 context。

没有新增公共 App Server 字段、资源类型 catalog、资源工具或资源事件。现有
`skills_loaded` 仍只表示 Skill 正文加载或按需读取的 `SKILL.md`，不会为每个
reference 或脚本文件制造事件。

## 边界和未覆盖项

Skill 根目录只读授权不等于禁用项目 Skill 的目录隔离。项目 Skill 位于工作区
内时，禁用 Skill 后仍可能作为普通项目文件被读取，但它不会出现在有效 Skill
catalog，也不能通过 `$` 或 `+` 激活。若未来需要严格隔离，应单独设计 deny-root
策略，不应混入普通 Skill read-root。

当前预算状态适用于一个 Runtime 的活动 Turn。Mini Agent 当前一次只运行一个
活动 Turn；如果未来允许同一 Workspace 的 Turn 并发，需要将单一预算状态替换为
按 Turn ID 管理且有界回收的状态表。

脚本和 MCP、hooks、多模型、并行 Agent 仍不自动启用。资源文件也不会根据
Markdown 链接自动激活另一个 Skill。

## 验证

Capabilities 测试覆盖了：

- 启用 Skill 根目录读取无需 approval；
- Skill 根目录不能因此写入；
- Discovery 只返回启用 Skill 根；
- Skill 读取预算按 Turn 隔离；
- 现有普通工作区和扩展根读取行为保持不变。

文档同步到：

- `docs/app-server.md`
- `docs/configuration.md`
- `docs/limits.md`
- WebStudio `docs/skills.md`
- WebStudio `server/README.md`

## 依据

- [Agent Skills 概览](https://agentskills.io/home)
- [Agent Skills 规范](https://agentskills.io/specification)
- [Skill 发现与工具组装](../../../crates/mini-agent-capabilities/src/skills.rs)
- [Workspace 文件边界](../../../crates/mini-agent-capabilities/src/workspace.rs)
