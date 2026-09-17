# Builtin Skill 组与 Turn 级显式激活

状态：implemented
日期：2026-09-17
范围：Capabilities、Host、App Server、Protocol，以及 WebStudio 的跨仓调用契约

## Decision

Mini Agent 将内置技能组作为标准 Skill Discovery 的一个来源，不把
`pstack-plugin` 当作 Cursor 或 Codex Plugin 安装。WebStudio 随附
`pstack` 的 26 个 `SKILL.md`，启动时同步到用户级 builtin root。Host 根据
项目配置选择有效的 builtin group，Capabilities 再把 builtin、project、plugin
和 MCP 发现结果保持为独立来源。

技能发现和技能激活保持分离：

- Discovery 只读取有界元数据，并把 `name`、`description`、`source`、`group`
  和 `enabled` 放入 capability manifest；
- `TurnInput.selected_skills` 只表达当前 Turn 的显式选择；
- Host 依据当前有效目录重新校验名称，并从受信任路径读取正文；
- 读取后的正文只追加到当前 Turn 的临时 harness prompt，不写入会话历史，
  也不修改全局 system prompt。

项目关闭技能组时，WebStudio 传递空的
`MINI_AGENT_BUILTIN_SKILL_GROUPS`。运行时必须区分“变量缺失”和“变量为空”。
变量缺失仍默认启用 `pstack`，空值则关闭全部 builtin group。

## Boundary and rationale

Core 不负责文件扫描、路径信任或正文读取。它只负责在 `turn_started` 之后
按顺序投影预检事件，并在预检失败时结束 Turn。Host/App Server 保留最终的
技能名称和正文加载权，避免浏览器提交路径、正文或未经验证的 catalog。

第一版不根据 Skill 依赖自动启用 MCP 或其他 Provider。这样可以保留
“技能提示”和“能力启用”两个独立决定，也避免把插件安装行为带入普通 Turn。

协议使用可选的 `selectedSkills` 字段，缺失时保持旧客户端行为。显式加载
成功发送一次 `skills_loaded`，失败发送一次 `skills_load_failed`。两类事件都
使用现有 `EventEnvelope` 的 Thread、Turn、sequence 和 bounded item identity。

## Limits and failure behavior

- 每个 Turn 最多激活 8 个技能；重复名称按首次出现顺序去重；
- 单个技能沿用现有 `SKILL.md` 文件大小限制；
- 当前 Turn 的技能正文合计最多 32 KiB；
- 未知、禁用、越界或读取失败的技能阻止模型调用；
- 失败事件只包含技能名称和有限的 `reason_code`，不包含正文、路径或敏感数据；
- 普通任务仍然只使用 metadata-first Discovery，不产生 `skills_loaded`。

## Verification

实现与跨仓调用链已用以下证据验证：

- Capabilities 覆盖 builtin group 发现、来源/分组元数据、正文读取和正文总量限制；
- App Server 覆盖技能失败事件先于模型执行的顺序；
- Protocol 覆盖缺失字段兼容，以及 `selectedSkills` 的 JSON round-trip；
- Web Gateway 覆盖启动同步、幂等、哈希变化、异常恢复、目录 API 和 409 冲突；
- Web Studio 覆盖 `$` 解析、去重、转义、搜索、禁用技能和队列消息；
- Rust 受影响包测试、Clippy、fmt、line budget、Python 测试与前端测试/构建均通过。

## Consequences

WebStudio 可以提供全局随产品安装的 `pstack`，同时让每个 Project 独立控制
是否使用该技能组。未来增加其他 builtin group 时，只需增加资源目录、版本
信息和项目开关，不需要把技能正文或插件运行时搬进 Core。
