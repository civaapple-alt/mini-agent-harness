# pstack 插件级与命名空间 Skill 双入口

状态：implemented

## Decision

WebStudio 项目默认开启 `pstack` Skill Group。这个默认值表达“技能可被发现
和使用”，不代表每个 Turn 都会执行 pstack 工作流。

- `+ pstack` 是 Turn 级 Skill Group 激活。它通过 `workflow` 传递组身份，
  只把有界的 Skill 元数据和工作流提示加入当前 Turn；模型按需读取相关
  `SKILL.md`，不加载全部正文。
- `$pstack:architect` 是 Skill 级显式激活。Host 根据有效目录重新校验，
  在模型调用前读取正文并注入当前 Turn。
- `$pstack-plugin:how` 是 Codex 兼容别名，`$how` 仅在无歧义时解析为规范
  名称 `pstack:how`。

项目关闭 pstack 后，面板、`+` 和 `$` 补全都会显示不可用；Host 仍是最终
权威，拒绝浏览器伪造的组、Skill 路径或正文。

## Why no extra routing model

`+` 入口的目标是保留 pstack 的 metadata-first 行为。已有 Harness 模型
调用能够根据描述选择相关 Skill，再通过受信任的 `read_file` 读取正文。
增加一次路由模型会引入额外延迟、成本和第二套选择状态，也会让真正执行的
Skill 与路由结果产生漂移。因此 Host 只观察成功的正文读取并聚合
`skills_loaded` 事件。

## Boundary and limits

`workflow`、`selectedSkills` 和结构化事件属于 Protocol/App Server；Skill
发现、命名空间和正文读取属于 Capabilities；项目开关和资源同步属于
WebStudio/Gateway。每个 Turn 最多八个显式 Skill，正文总量最多 32 KiB。
事件只包含名称、qualified name、来源、分组和有限错误码。

## Verification

已覆盖协议 workflow 序列化、pstack 命名空间/兼容别名解析、Skill Group
激活事件、显式 Skill 失败不调用模型、SDK 事件解析、Gateway 结构传递和
WebStudio `$`/`+` 输入解析。剩余验证依赖本机是否安装 Python 的 pytest；
Rust、Node unit/UI/build/lint 已在本次变更中运行。

## Non-goals

本版本不实现 Cursor 专属 `/poteto-mode`、MCP 自动启用、hooks、多模型路由
或并行 Agent。资源同步仍保留 MIT License 和 NOTICE，并通过版本、哈希、
staging 与原子切换保证可恢复。

Windows 路径边界补充：builtin root 和待扫描目录在 containment 比较前都必须
canonicalize。否则合法的 `~/.mini-agent/skills/builtin/pstack` 可能因为路径
形式差异被误报为越界，表现为 capability manifest 只有 pstack 组状态而没有
组内 Skill catalog。
