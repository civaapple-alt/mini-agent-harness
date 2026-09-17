# Skill 关联资源按需读取

Status: proposed  
Date: 2026-09-17  
Scope: mini-agent Capabilities、Host、App Server 和 WebStudio 的 Skill 资源读取

## 提案摘要

保留 Skill 的 metadata-first 发现方式。`parse_instruction` 只解析
`SKILL.md` 的有限 front matter，不递归读取 `references/`、`scripts/` 或其他
附件。

当 Skill 在当前 Turn 中被显式激活，或模型通过 `+ pstack` 按需选择 Skill 后，
Host 允许模型读取该 Skill 目录下的关联资源。资源读取继续使用现有
`read_file`，由 Host 校验路径、归属、文件类型和当前 Turn 预算。脚本可以被读取，
但不会因为 Skill 引用它就自动执行。执行脚本仍然只能经过 `shell`、沙箱和审批策略。

这条提案解决两个问题：

1. pstack 的 `SKILL.md` 可以引用 `references/patterns.md` 等辅助材料；
2. Skill 可以携带脚本或其他资源，但资源不会绕过现有文件和命令安全边界。

## 与 Agent Skills 标准对齐

[Agent Skills 标准](https://agentskills.io/home) 把 Skill 定义为一个目录。目录至少
包含带 YAML front matter 和 Markdown 正文的 `SKILL.md`，也可以包含
`scripts/`、`references/`、`assets/` 以及其他文件。

标准使用渐进式披露：启动时只发现 `name` 和 `description`，激活时读取完整的
`SKILL.md`，执行时按需读取 reference 或 asset，也可以在受控环境中运行 script。
它还要求 Skill 内的文件引用使用相对于 Skill root 的路径，并建议引用链保持在
`SKILL.md` 下一层。

mini-agent 会保留这套语义，但把“可以运行脚本”解释为“脚本仍必须经过 Host 的
工具、沙箱和审批边界”。标准格式不会自动授予操作系统权限。

## 当前实现

当前代码已经有两段不同的读取路径：

```text
发现阶段
  parse_instruction
    → 读取 SKILL.md 前 16 KiB
    → 解析 name、description 和工具依赖
    → 生成有界 Skill metadata

显式激活
  load_skills
    → 重新校验规范名称和 enabled 状态
    → 读取选中的 SKILL.md
    → 移除 front matter
    → 将正文加入当前 Turn，正文合计不超过 32 KiB

组级激活
  + pstack
    → 只加入 pstack metadata
    → 模型通过 read_file 读取需要的 SKILL.md
    → Host 观察成功读取并聚合 skills_loaded
```

`augment_system_prompt` 已要求模型从 Skill 文件所在目录解析相对引用。Builtin
Skill root 作为受信任的只读扩展目录加入 `read_file` 的读取范围。因此当前
pstack 的 `typescript-best-practices/references/patterns.md` 可以被模型按需读取，
但它不会在发现或激活时自动注入。

当前实现还没有以下能力：

- 记录某个 `references/` 或 `scripts/` 文件属于哪个 Skill；
- 为 Skill 资源设置独立的当前 Turn 总量和文件数量预算；
- 根据 Skill 正文自动解析并激活其他 Skill；
- 因为 Skill 引用了脚本而自动执行脚本；
- 在公开 capability manifest 中返回资源路径或正文。

## 设计目标

### 保留按需加载

普通任务继续只看到 bounded metadata。显式 `$pstack:how` 只预加载 `how` 的
正文。`+ pstack` 只激活组 metadata，模型自行判断是否需要某个 Skill 及其
关联资源。

不会在发现阶段遍历整个资源树，也不会因为读取一个 `SKILL.md` 就把所有链接的
Markdown、脚本和附件塞进 system prompt。

### 资源属于 Skill 目录

资源通过 `SKILL.md` 所在目录确定归属：

```text
<skill-root>/
├── SKILL.md
├── references/
├── scripts/
└── assets/
```

第一版会识别 `references/`、`scripts/` 和 `assets/` 三个标准约定目录。标准允许
Skill 携带其他文件和目录，因此 mini-agent 不会因为目录名称不在这三个约定中就
拒绝安全的只读资源。其他资源不会因出现在 Markdown 链接中而自动预加载或自动
执行。项目 Skill 的资源仍受项目 workspace 边界约束，builtin Skill 的资源仍受
builtin root 边界约束。

### 读取和执行分离

`read_file` 只读取 UTF-8 文本。它可以读取 `references/*.md` 或脚本源码，不能
执行脚本。

脚本执行必须通过现有 `shell` 工具。Host 继续应用：

- 工具 allowlist；
- workspace 和 builtin 目录边界；
- sandbox 配置；
- 命令长度、输出和超时限制；
- 现有 approval policy。

Skill 正文中的“运行脚本”只是模型可见的工作建议，不是 Host 的执行授权。

## 重要类型和接口

这些类型先作为 Capabilities/Host 内部类型，不立即扩大公共 Protocol：

```text
SkillResourceKind = Reference | Script | Asset | Other

SkillResourceIdentity {
  qualified_skill: String
  kind: SkillResourceKind
  relative_path: String
}

SkillResourcePolicy {
  max_files_per_turn: usize
  max_bytes_per_file: usize
  max_bytes_per_turn: usize
  allowed_directories: Set<SkillResourceKind>
}
```

建议保留现有模型调用形状。模型继续调用：

```json
{
  "name": "read_file",
  "arguments": {
    "path": "C:/trusted-skill-root/typescript-best-practices/references/patterns.md"
  }
}
```

Host 在 `read_file` admission 和执行结果处完成资源识别。前端、队列消息和公共
协议只传 Skill 名称，不传资源物理路径、正文或浏览器计算出的根目录。

如果后续必须强制模型只能读取“本次显式激活 Skill”的关联资源，再增加内部
`resolve_skill_resource(qualified_skill, relative_path)`。它应先解析相对路径，
再 canonicalize，最后确认结果仍在该 Skill root 下。不要让前端直接调用这个
解析器，也不要把未经校验的路径加入 `extra_read_roots`。

## 资源读取流程

### 显式 `$` 激活

```text
InputBar
  → selectedSkills: ["pstack:typescript-best-practices"]
  → App Server
  → Host 校验 Skill
  → Capabilities 读取 SKILL.md 正文
  → 当前 Turn 临时上下文
  → 模型按正文需要调用 read_file
  → Host 校验 references/scripts/assets 路径和预算
```

显式激活成功的 `skills_loaded` 事件仍只表示 Skill 正文已加载。关联资源读取不
需要为每个文件新增一个公开事件。

### `+ pstack` 组级激活

```text
+ pstack
  → workflow: {kind: "skill_group", id: "pstack", mode: "auto"}
  → skill_group_activated
  → 模型查看 pstack metadata
  → 模型读取选中的 SKILL.md
  → 模型读取该 Skill 需要的关联资源
  → Host 聚合 skills_loaded
```

`skills_loaded` 的 Skill 归属根据读取的 `SKILL.md` 确定。读取
`references/patterns.md` 不会被误报成一个新的 Skill。

## 路径和安全边界

Host 是最终权威。Host 不信任客户端传来的物理路径、正文或资源归属。

资源读取必须满足以下条件：

1. 路径先转换为绝对路径并 canonicalize；
2. Skill root 和边界路径使用同一种 canonical 形式比较；
3. 结果必须仍在受信任的 Skill root 内；
4. 符号链接不能把资源解析到 Skill root 之外；
5. 资源必须是普通文件并且是 UTF-8 文本；
6. 读取失败时返回有限错误，不调用模型的下一步；
7. 资源内容不能进入 capability manifest、事件日志或 WebStudio 的错误消息。

Windows 路径比较必须沿用已经修复的 canonical boundary 规则。否则会出现
“pstack 组状态存在但 catalog 为空”的假阴性，或者把合法资源误判为越界。

## 建议预算

第一版建议把 Skill 正文和关联资源分开计算，但都受当前 Turn 的统一上限保护：

| 项目 | 建议限制 | 作用 |
| --- | ---: | --- |
| 显式 Skill 数量 | 8 | 复用现有限制 |
| 单个 `SKILL.md` 正文 | 现有文件和正文限制 | 防止单个 Skill 膨胀 |
| 单个关联资源 | 64 KiB | 防止单文件占满上下文 |
| 一个 Turn 的资源文件数 | 16 | 防止递归读取大量小文件 |
| 一个 Turn 的关联资源总量 | 64 KiB | 控制 references 和脚本源码总量 |
| 资源路径深度 | 8 层 | 限制异常目录结构 |

如果后续证明 64 KiB 与模型上下文不匹配，再调整常量和测试，不把限制改成无限
分页。`read_file` 的单页输出限制仍然有效，分页读取产生的可见字节必须累计到
当前 Turn 预算。

## 失败和可观测性

资源读取失败要区分以下有限错误：

```text
skill_resource_not_found
skill_resource_outside_root
skill_resource_not_text
skill_resource_file_too_large
skill_resource_budget_exceeded
skill_resource_limit_exceeded
```

当前公开的 `skills_load_failed` 可以继续使用 `body_read_failed` 表示显式
`SKILL.md` 正文失败。关联资源失败发生在模型已经开始运行之后，不能伪装成
显式 Skill 正文预加载失败。第一版可以先把资源读取错误作为普通 `read_file`
工具错误，并在 Host 内部记录有限错误码。

如果 WebStudio 后续需要显示“读取了哪些 Skill 资源”，新增事件应只包含：

```json
{
  "type": "skill_resources_loaded",
  "skills": [
    {
      "qualifiedName": "pstack:typescript-best-practices",
      "kinds": ["reference"]
    }
  ]
}
```

这个事件不是本提案第一批的必需项。它不能包含物理路径、资源文件名、正文或
脚本命令。默认只保留现有 `skill_group_activated` 和 `skills_loaded`，避免为
每次文件读取增加事件噪声。

## 备选方案

### 方案 A：发现时递归加载所有资源

不采用。它会把 metadata-first 变成隐式批量加载，增加启动时间、上下文大小和
受恶意资源影响的范围。它也无法判断模型实际需要哪个 reference。

### 方案 B：为每个 Skill 增加新的 `read_skill_resource` 工具

暂不采用。现有 `read_file` 已经拥有分页、UTF-8、文件大小和 Host admission
能力。新增工具会增加模型可见工具面和另一套文件读取语义。只有当现有
`read_file` 无法在真实场景中强制 Skill root 归属时，才考虑这个方案。

### 方案 C：继续把整个 builtin root 作为普通扩展目录

这是当前兼容路径，但不够完整。它能让模型读取 pstack 的 reference，却不能单独
累计资源预算，也不能区分“读取了哪个 Skill 的资源”。提案选择在现有路径上增加
Host 侧识别和预算，而不是立即改变工具名称。

## 分阶段实施

### 第一批：内部资源识别和预算

- 在 Skill 元数据内部保留 canonical Skill root；
- 在 Host 识别 `read_file` 成功读取的 Skill 资源；
- 增加单文件、文件数量和 Turn 总量限制；
- 拒绝越界、非文本和超限资源；
- 保持公共 Protocol 和 WebStudio UI 不变；
- 增加 references、scripts、符号链接和分页读取测试。

### 第二批：受控资源 catalog

只有面板确实需要显示资源能力时，才在 bounded capability manifest 中增加资源
类型或数量，例如 `referenceCount`、`scriptCount`。不返回资源路径和正文。

### 第三批：Skill 间显式依赖

如果 pstack 或其他 Skill 需要稳定调用另一个 Skill，再单独设计
`SkillDependency`。它必须区分“文本建议”与“Host 自动激活”，并复用当前
8 个 Skill 和 32 KiB 正文限制。第一版不把 Markdown 中的链接或自然语言指令
解析成依赖。

## 验收场景

| 场景 | 要证明的事实 |
| --- | --- |
| 普通任务 | 不读取任何 Skill 正文或关联资源 |
| `$pstack:typescript-best-practices` | 先加载 SKILL.md，模型可以按需读取 `references/patterns.md` |
| `+ pstack` | 只加载 metadata，模型按需读取实际选择的 Skill 和资源 |
| 相对路径 | `references/patterns.md` 从所属 Skill root 解析，而不是从工作目录猜测 |
| `../` 路径 | 越界读取被拒绝 |
| 符号链接 | 指向 Skill root 外部的链接被拒绝 |
| 大资源 | 单文件或 Turn 总量超限时停止该读取 |
| 多页资源 | 分页累计到同一个 Turn 预算，不可绕过总量限制 |
| 脚本引用 | 可以读取脚本源码，但不会自动执行 |
| 脚本执行 | 只能经过 shell、sandbox 和 approval policy |
| 资源失败 | 返回有限错误，不泄露路径或正文到公开事件 |
| replay | 现有 Skill 激活事件仍可重放，资源事件不产生大量逐文件噪声 |

## 六项变更准入

1. **层级**：资源发现和安全校验属于 Capabilities；当前 Turn 的资源预算、
   读取观察和错误投影属于 Host/App Server；WebStudio 只消费 bounded manifest
   和事件；Core 不增加资源扫描或执行逻辑。
2. **重复责任**：复用 `parse_instruction`、`load_skills`、`read_file`、
   `ToolOrchestrator` 和现有路径策略，不新增第二套 Skill 发现器或脚本执行器。
3. **替换优先**：先扩展现有 `read_file` admission 和 Host 观察路径。只有真实
   验收证明无法表达 Skill root 关联时，才增加 typed resource tool。
4. **行数预算**：本提案只增加一份 note，当前 runtime 和 release Rust 增量为
   零。实现批次必须分别记录预期和实际 line delta，并运行 line budget。
5. **模型可见面**：资源正文、分页结果和资源错误都会进入当前 Turn 的模型可见
   面，必须受单文件、文件数、总字节和深度限制。脚本执行仍是独立的副作用面。
6. **边界证据**：至少需要 Capabilities 路径测试、Host/App Server 工具事件
   测试、Harness scenario 和 WebStudio replay 测试。仅验证 `SKILL.md` 能读不够
   证明 references、scripts 和分页预算正确。

## 非目标

- 不在发现阶段读取所有资源；
- 不自动执行 `scripts/`；
- 不从 Markdown 链接自动激活其他 Skill；
- 不因为资源目录名称不同于 `references/`、`scripts/` 或 `assets/` 就改变其标准兼容性；
- 不自动启用 pstack 的 MCP、hooks、多模型或并行 Agent；
- 不向前端返回 Skill root、资源物理路径、资源正文或命令；
- 不把资源读取事件变成每个文件一次的公开事件；
- 不把关联资源写入后续 Turn 或全局 system prompt。

## 依据

- [当前 Skill 发现与激活实现](../../../crates/mini-agent-capabilities/src/skills.rs)
- [当前 Skill front matter 解析](../../../crates/mini-agent-capabilities/src/skills/discovery.rs)
- [当前 `read_file` 和 workspace 路径边界](../../../crates/mini-agent-capabilities/src/workspace.rs)
- [当前 App Server Skill 事件协议](../../../docs/app-server.md)
- [当前配置和 Skill 读取限制](../../../docs/configuration.md)
- [pstack 插件级与命名空间 Skill 双入口](../../implemented/feature/2026-09-17-pstack-plugin-and-namespaced-skill.zh.md)
- [Agent Skills 概览](https://agentskills.io/home)
- [Agent Skills 规范](https://agentskills.io/specification)
