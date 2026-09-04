# 从指挥型交互到结果导向的长期 Bot

Status: proposed
Date: 2026-09-04
Scope: mini-agent Host、App Server、Python SDK、独立 Bot Studio 以及有限的 Custom Agent 适配

## 提案摘要

将 mini-agent 从以 Project/Thread/Turn 为中心的临时任务执行器，逐步扩展为
支持长期岗位、持续结果和多 Bot 协作的 Agent 平台。

本提案不把 Bot 变成新的执行循环，也不把长期记忆、调度、连接器或路由塞进
Core。Bot 应是位于 Host/App Server 控制面上的长期工作身份；具体一次工作仍
通过现有 Thread、Turn、Goal、Tool、Session 和 Approval 完成。

本提案也不把当前 Web Studio 直接改造成 Bot 产品。当前 Web Studio 继续作为
已有的 Thread-first 客户端和执行证据界面；Bot 能力先在底座、App Server 和
Python SDK 中形成稳定语义，之后由 Web 侧另建独立的 Bot Studio。两个 Studio
可以共享协议、事件和组件，但不共享一套互相耦合的页面状态或产品生命周期。

目标形态：

```text
Bot（长期岗位）
  → Job / Goal（一次结果责任）
      → Thread（执行上下文）
          → Turn / Tool / Event
              → Artifact（交付物）
```

## 背景与动机

当前常见的 Agent 使用方式是用户不断发出指令：打开一次对话、说明背景、派出
一个任务、得到一个结果，然后结束协作。这种方式适合一次性问题，却不适合
持续出现、拥有固定方法和稳定交付标准的工作。

`.tmp/bot` 中的两份材料提出了一种不同的组织方式：把 Agent 看成长期在岗的
数字员工。每个岗位拥有职责、规则、Skill、数据源、工具、交付位置和人工
审批边界；Main Agent 负责入口和路由；专业 Agent 负责稳定结果；定时任务
负责重复触发；Handoff 负责岗位之间交接；Human in the loop 负责外部后果和
最终确认。

这与当前 mini-agent 的方向相容，但抽象层级更高：当前系统已经能执行一个
有界的 Goal，却还没有表达“谁长期负责这类结果”“这次 Job 应由谁处理”以及
“结果如何交给下一个岗位”。

## 核心概念边界

| 概念 | 责任 | 与现有概念的关系 |
| --- | --- | --- |
| Bot | 长期岗位身份、职责、默认能力和结果责任 | 不是 Thread，也不是一次 Goal |
| Custom Agent | 岗位如何工作：指令、工具、模型、子 Agent 和交接配置 | 可作为 Bot 的定义来源，但不等于完整 Bot |
| Skill | 一类重复工作的具体方法和验收步骤 | 复用现有 `.agents/skills` |
| 岗位过程资产 | 经确认的 Runbook、Case、Inventory、Decision 等工作经验 | 不是原始聊天历史，按 Job/Artifact 提炼和版本化 |
| Job | 交给 Bot 的一次具体工作 | 初期可映射为 Thread + Goal |
| Goal | 当前 Job 的目标、验收和继续/停止条件 | 复用现有 GoalRuntime |
| Thread | 一次工作的对话和执行上下文 | 继续由 App Server/Session 管理 |
| Turn | 一次模型和工具执行周期 | 继续由 Core/Thread 管理 |
| Artifact | 报告、文件、数据表等正式交付物 | 初期复用 ResultStore 和 ThreadItem |
| Handoff | 一个 Bot 把有界上下文、交付物和下一步交给另一个 Bot | 当前 `steer`/`fork` 只能部分覆盖 |
| Schedule | 何时产生或重复产生 Job | 初期放在 Python Worker 或外部调度器 |
| Routine | 一个 Bot 所拥有的可重复流程、触发条件和运行策略 | `Skill` 负责怎么做，Routine 负责何时做 |
| Connector | 进入外部软件、账号和数据源的能力 | 复用 MCP/Capabilities，后续补凭证边界 |
| Presence | Bot/Job 当前状态的可解释投影 | 只读投影，不改变执行权威 |
| Computer / Workspace | 浏览器、命令行、文件和可连接工具组成的工作面 | 是执行环境，不等于 Bot 身份或安全边界 |

最重要的判断是：

```text
Custom Agent：这个岗位应该怎样工作
Bot：这个岗位长期负责什么结果
Skill：某类工作具体怎样完成
Goal：这一次要达到什么结果
Thread：这一次工作在哪里执行
```

## 从 Grok Bot 资料提炼的可迁移语义

以下内容来自官方 Grok Bot 入门、Bot 管理、协作、结果、Computer、Skill/Routine
以及安全文档，并与 `.tmp/bot/bot.md`、`.tmp/bot/introducing-grok-bot.md` 和
前面的 `designing-grok-bot.md` 交叉核对。它们是产品行为的参考，不是要求
mini-agent 复制 Grok Bot 的实现或协议。

### 1. Bot 创建的最小信息不是一段长 Prompt

Grok Bot 的创建入口只要求短名称、一个主岗位和岗位如何工作的描述；官方还建议
为每个 Bot 选择有明确差异的目标或责任范围、工具和来源、工作风格、审批边界及
重复周期。`General Helper` 这类无限职责会降低长期上下文的复用价值。

因此，mini-agent 的岗位定义应优先表达“责任合同”，而不是把一个通用 Agent 的
prompt 继续加长：

```text
BotSpec
  → 我负责哪一种长期结果
  → 我可以使用哪些来源、Skill 和工具
  → 我按什么方法和格式交付
  → 哪些动作必须停下来等人
  → 哪些 Job 可以由我长期接收
```

这也解释了为什么 `Custom Agent` 只能作为 Bot 的定义来源之一：它描述岗位的
工作方式，但不自动提供长期责任、运行记录、记忆、调度或结果生命周期。

### 2. 一次任务应使用结果合同启动

官方入门文档把一个强任务请求拆成五项：

```text
Outcome       要完成什么结果
Sources       哪些应用、网站、文件或对话是依据
Constraints   哪些事情不能做、何时必须询问
Deliverable   以什么 Artifact 或格式返回
Review point  在哪个动作或结果节点停下来交给人
```

这五项应成为 Job 的最小输入协议，而不是只作为 prompt 写作建议。Host 或
App Server 可以将其归一化成 `JobRequest`，再由 BotSpec 补充默认岗位规则；
缺失字段时可以由模型询问，但不得默默使用无限历史或默认放宽审批。

结果也应是可审阅的结构，而不是只返回一段最终文本：

```text
ResultEnvelope
  facts              来自源系统的事实
  assumptions        推断与假设
  completedActions   已经完成的动作
  pendingApproval    等待人工确认的动作
  unresolved         未解决问题或无法验证的部分
  artifacts          文件、链接、截图、日志或草稿
  nextAction         建议的下一步和负责人
```

对于发送、发布、付款、删除、权限修改和生产变更，必须把“结果已准备好”与
“允许产生外部后果”分开。`Artifact review` 不是工具动作审批的别名。

### 3. 先完成一次，再保存 Skill，最后才自动化

Grok Bot 资料呈现出一条重要的能力晋级顺序：

```text
一次性 Job
    → 人工审阅与纠正
        → 保存 Skill
            → 安全样例 Test run
                → 创建 Routine
                    → 按时间或窄事件重复运行
```

`Skill` 是可复用的“怎么做”，至少应包含使用时机、输入与访问要求、步骤、
验证方式、返回格式和审批边界。`Routine` 不是另一个 prompt，而是某个 Bot
拥有的 Skill/workflow 加上负责人、触发器、时区、输入源、失败策略和结果去向。

Routine 在启用前必须能测试，而且测试可能真的修改文件、调用工具或访问网站；
因此测试本身也要走沙箱和审批。Routine 还应具有无数据/旧数据策略、幂等键、
部分完成报告、暂停和最近运行记录。事件触发必须使用窄匹配规则，不能默认监听
“每一条新消息”。

“跟随用户演示一次工作”可以作为未来 Skill 的输入方式，但演示得到的只是待审阅
草稿，不能把录制的点击序列直接当作可信自动化；仍需补充决策规则、异常处理、
验证和审批边界。

### 4. Computer 是长期执行面，不是 Bot 的身份

官方资料中的云端 Computer 让工作在用户关闭电脑后继续：浏览器、命令行、文件
和连接器都可以被使用；登录会话、文件和命令行凭证在同一用户下的多个 Bot 之间
共享。每个 Bot 可以有独立屏幕以支持并行工作，但屏幕不是安全边界。

这对 mini-agent 有两个直接结论：

1. `BotSpec` 可以选择工作环境、workspace root、sandbox 和工具集合，但不能把
   Bot 名称当成凭证或授权主体；真正的边界仍是 Host principal、Tool allowlist、
   ToolRuntime sandbox、路径范围和 Approval policy。
2. 共享计算机、Bot 专属 workspace、Job 临时目录和用户本地电脑必须分层表达。
   本地执行是额外能力，默认不能因为 Bot 使用云端工作区就获得本地权限。

Connector 应优先走结构化能力；浏览器/Computer 用于没有 Connector 或必须进行
视觉操作的流程。认证应通过用户 takeover 或安全 secret handoff 完成，凭证不能
进入普通消息、模型上下文、事件日志或 Handoff payload。

### 5. 长期记忆要帮助岗位稳定，而不是替代事实源

Grok Bot 的 Bot 会保留稳定偏好、重要事实和工作摘要；但官方同时强调变化中的
事实应留在源系统，重要判断应重新引用当前数据。由此可以把记忆分成三类：

```text
Bot Profile     岗位职责、稳定边界、长期输出偏好
Bot Memory      经确认的规则、方法、事实摘要和用户纠正
Job Session     当前输入、工具调用、事件、Artifact 和审批状态
```

记忆写入也不是普通低风险工具调用：应记录来源、时间、置信度、作用域和修正
方式，并允许用户查看、纠正、停用或删除。每次 Job 只检索有界摘要；对会变化的
客户、库存、指标和生产状态，必须回到 Connector 或源文件读取当前值。

### 6. 协作的关键是单一阶段负责人，而不是 Bot 数量

本地资料和官方协作文档都指向同一原则：Main/Chief 可以负责入口和分派，但每个
阶段应有一个明确 owner；专业 Bot 只在存在稳定责任边界时加入。Bot 可以异步
消息、直接交接或进入 2～6 个 Bot 的可见群聊，但过多并行 handoff 会产生重复
工作和噪声。

用户在工作进行中发送的新指令可以改变当前优先级，对应 mini-agent 的 `steer`
或 `interrupt`；“停止”只停止后续动作，不撤销已经完成的副作用。Bot-to-Bot
交接则应是可追踪的 Handoff，包含目标、Artifact 引用、验收标准、当前 blocker
和下一位 owner，而不是把完整会话复制给目标 Bot。

### 7. Bot 复制、分享、隐藏和删除必须有明确生命周期

长期岗位不是 Thread 的别名，生命周期语义需要单独定义：

| 操作 | 应保留 | 不应隐式复制或删除 |
| --- | --- | --- |
| Duplicate Bot | profile、工具/Skill 配置、Routine 模板、岗位边界 | conversation history、learned memory、attachments |
| Share Bot | 可公开的身份、描述和配置 | computer、logins、历史、凭证、客户数据 |
| Hide Bot | profile、Job、Routine 和运行记录 | 不应等同于 pause，后台 Routine 仍按策略运行 |
| Delete Bot | 按保留策略处理审计和已交付 Artifact | 不应假设共享 workspace 文件或登录会话自动消失 |

这些语义会影响 Bot Catalog、`BotSpec` 版本和 App Server 的删除/归档 API；不能
直接套用 `thread/fork` 或 `thread/archive` 的含义。

### 8. Presence 是观察投影，不是新的运行状态机

Bot Studio 可以显示 `idle`、`thinking`、`working`、`waiting`、`blocked`、
`done` 等 Presence，并显示当前动作、等待原因、最近 Artifact 和下一步。它们
应由现有 Thread/Turn/Goal/Approval/Job 事件折叠得到：

```text
事件流 → bounded projection → Bot/Job Presence
```

Presence 只帮助用户决定是否介入，不得成为 Web 前端自己维护的第二套执行状态，
也不能以动画或“在线”状态暗示 Bot 仍拥有未记录的后台执行权。

## 评论反馈转化为设计假设

`.tmp/bot/comment` 中的两份 CSV 是相关帖子的评论导出，但导出结果同时包含作者
回复、原帖和少量无关时间线记录。因此它们适合作为方向性用户反馈，不适合作为
精确的投票统计；本提案只提取反复出现、且能转化为可验证场景的信号。

### 反复出现的信号

| 评论信号 | 对提案的影响 |
| --- | --- |
| 岗位、文件、数据、接口和权限比 Bot 名字更重要 | 先建设 `BotSpec`、Role Asset 和能力边界，不先扩展人设系统 |
| “把一件小事先干利索” | 第一阶段只验证一个 Main Bot + 一个专业 Bot 的小结果，不先组建大团队 |
| 状态延续、Skill、排队和交接才是“在岗” | 把 Job、Artifact、资产提炼、Handoff 和恢复列为底座证据 |
| 本地上下文是开发者最重要的资产 | 增加 allowlist 的本地上下文导入/同步桥，不把云端 Memory 当作唯一方案 |
| 费用、额度和运行耗时会限制长期使用 | Job、Routine 和 Bot 需要可观察的 step/tool/token/byte/耗时预算和失败原因 |
| Codex 也可以按这套方式变成数字员工 | Bot Studio 与仓库/项目型 Web Studio 分离，强调岗位责任和过程资产的产品差异 |
| 中文、模型切换、X/飞书等连接器影响真实采用 | 作为 Connector、模型选择和本地化的后续适配项，不让产品宣传替代边界证据 |
| “数字员工只是脆弱 Cron + 过期记忆”的质疑 | 强制幂等、来源、版本、stale 标记、恢复和人工撤回，作为反例验收 |

### 由反馈得到的优先级

```text
岗位边界 / 数据与权限
        → 一件小事的可靠结果
            → 过程证据和岗位资产
                → 状态延续、Handoff、恢复
                    → Skill / Routine 自动化
                        → 多 Bot 团队和独立 Bot Studio
```

这条顺序意味着“多 Agent 数量”不是第一阶段的成功指标。成功指标应是同一岗位
在第二次、第三次处理相似 Job 时，能基于已确认的 Case/Runbook 减少重复解释，
同时仍能识别当前环境变化并引用新的事实源。

## 当前能力与缺口

### 已有基础

mini-agent 已经具备可以承载 Bot 的执行底盘：

- Core 提供 Model Step、Tool Loop、Context Limits、Stop Classification 和事件；
- Host 的 `RuntimeComposition` 组合 Agent、Persona、工具、扩展、沙箱和安全策略；
- `builtin/prompts` 提供编译期嵌入的 `general`、`explore`、`plan` 基础提示词，以及
  `reviewer`、`implementer`、`researcher` Persona；
- App Server 提供 Thread、Turn、Goal、Session、Item、Approval 和有序事件；
- Python SDK 已提供 App Server 进程连接、类型解析、事件流、steer、interrupt 和
  approval 回调；
- `.agents/skills`、MCP 和插件已经提供有限的可复用能力和扩展入口；
- GoalRuntime 已经能够在 settled checkpoint 后验证、继续或停止长期任务。

### 主要缺口

当前系统还没有：

1. 持久化的 Bot 身份、岗位目录和负责人关系；
2. 从 Bot 到 Job/Goal/Thread 的明确绑定；
3. Main Bot 到专业 Bot 的路由和路由结果；
4. 带有目标、交付物和验收标准的 Bot-to-Bot Handoff；
5. 跨重启、跨 Job 保存的岗位记忆；
6. 岗位过程资产的提炼、确认、版本、过期和来源追踪；
7. 本地上下文、代码仓库、服务器清单与云端 Bot 之间的受控同步；
8. 幂等的定时触发和长期运行记录；
9. 结果交付、结果复核和外部发布前的人工确认；
10. 有 Outcome、Sources、Constraints、Deliverable、Review point 的 JobRequest；
11. Skill 到 Routine 的晋级、测试、暂停、失败报告和窄事件触发；
12. 连接器账号、凭证引用、用户主体和岗位级能力边界；
13. 云端/本地/共享 workspace 的执行环境与恢复语义；
14. Bot 的 Presence 投影，以及复制、分享、隐藏、删除的生命周期语义；
15. VS Code `.agent.md` 的有限解析和 Custom Agent 适配。

当前 `.tmp/agents` 中的 Ask、Explore、Plan 是 VS Code Agent 配置参考，不会被
mini-agent 自动发现。mini-agent 当前使用 Rust 类型化的 `AgentKind`、
`PersonaKind` 和编译期 prompt asset；它不执行 VS Code Custom Agent 的
handoff、hooks、UI 元数据、模型选择或 subagent isolation。

## 目标架构

### 控制面与执行面

```text
控制面：Bot Catalog / Job / Handoff / Schedule / Approval / Artifact
                         ↓
执行面：App Server Thread / Goal / Turn / Tool / Event / Session
                         ↓
                 Host → Capabilities → Core
```

控制面决定谁负责、做什么、交给谁以及何时停下；执行面负责一次具体的模型和
工具运行。两者必须共享 App Server 的 Thread、Goal、Session 和事件权威，不得
由 Python SDK 或 Web Gateway 另建一套对话历史或执行循环。

### 底座优先与产品隔离

Bot 不是当前 Web Studio 的一个页面模式，而是建立在执行底座之上的长期工作
抽象。底座必须先能被 Python Worker、命令行、自动化任务和不同 Web 客户端共同
使用，再决定 Bot Studio 的交互形式。

```text
稳定底座：Core → Host → App Server → Python SDK
                         ↓
       既有 Web Studio（Thread-first）   独立 Bot Studio（Bot-first）
```

这里的“独立”是产品和状态边界独立，不是另起一套执行内核：两个客户端都通过
同一条权威链路观察和驱动工作。当前 Web Studio 不承担 Bot Catalog、Bot Memory、
Scheduler 或 Handoff 的状态权威；Bot Studio 也不能通过复制 Thread/Session
来绕过 App Server。

### Bot 定义

建议引入一个有界的 `BotSpec` 或等价的 Host 类型，最小字段包括：

```text
id / name / description
responsibility / ownership
instruction sources
allowed tools
allowed connectors
allowed skills
role asset scope
context sync / import policy
workspace roots
sandbox / execution environment reference
approval policy
memory read/write policy
handoff targets
run / routine policy
result contract
```

Bot 定义不直接保存凭证、进程句柄或无限大的历史。`BotSpec` 是岗位和能力的
声明，不是独立的认证主体；默认应绑定到用户或受控 Worker principal。岗位级
allowlist 可以减少可用能力，但不能取代 Host 的真实授权、沙箱和审批。

岗位说明和 Skill body 进入模型上下文前必须受到既有字节上限、来源筛选和脱敏
规则约束。`memory read/write policy`、Connector 引用和 execution environment
reference 也只保存声明或句柄，不保存 OAuth token、密码或一次性验证码。

现有的 `AgentKind`、`PersonaKind` 和 `RuntimeComposition` 应先作为内置 Bot 的
种子复用，不急于删除。只有在 Custom Agent 和岗位目录的真实场景证明需要时，
再把固定枚举推广为数据驱动的 `BotSpec`。

### 岗位过程资产

开发团队中的岗位 Bot 不应只拥有一段岗位描述，还应拥有一套经过工作验证的
过程资产。过程资产是“岗位经历的可复用部分”，它与一次 Job 的原始 Session
分开保存，并且必须能回到原始 Artifact、日志、代码变更或测试结果。

| 资产类型 | 记录内容 | 新鲜度要求 | 典型岗位 |
| --- | --- | --- | --- |
| Runbook | 已验证的步骤、前置条件、回滚和验收方式 | 相对稳定，但环境变化后需复核 | 部署、测试、运维 |
| Case | 症状、假设、排查路径、根因、修复和验证证据 | 与具体版本、环境和时间绑定 | 前后端、测试、部署 |
| Inventory | 服务器、服务、域名、版本、负责人、依赖和状态 | 高度易变，优先回源查询 | 部署、项目管理 |
| Decision | 选型、约束、备选方案、结论和决策人 | 直到被新决策取代 | 产品、项目、架构 |

这四类资产不能混成一个“长期记忆库”：

```text
Case       过去发生了什么
Runbook    以后通常怎么做
Inventory  当前真实状态是什么
Decision   为什么采用这个方案
```

#### 资产生成闭环

过程资产必须从真实工作中逐步产生，而不是创建 Bot 时凭空编写：

```text
Job 执行
  → 事件、命令、日志、截图、代码变更、测试结果
      → bounded Artifact / 过程文档
          → 生成候选资产
              → 岗位负责人确认、修订或驳回
                  → 发布为有版本的岗位过程资产
                      → 下次 Job 按任务和环境检索
```

模型可以提出“这次经验是否值得沉淀”的候选项，但不得自动把整段对话、所有
命令或所有本地文件永久变成岗位记忆。资产发布至少应记录：

```text
assetId / botId / kind / title
scope / source references / effective time
version / reviewed by / reviewed time
confidence / supersedes / stale condition
access scope / lifecycle status
```

岗位负责人可以是人、团队或另一个受控协调 Bot，但最终发布和废止应可审计。
当环境、版本或流程发生变化时，旧资产应被标记为 stale、superseded 或需要复核，
而不是继续以“历史上成功过”为理由自动执行。

#### 检索与上下文组合

一次 Job 的上下文应按以下优先级组合：

```text
当前源系统事实 / 当前文件 / 当前版本
        + 适用的 Runbook / Decision
        + 相似 Case 的有界摘要
        + 当前 Job 的输入和历史事件
```

`Inventory` 和外部业务数据优先回到源系统读取；`Case` 和 `Runbook` 只提供
经验与方法，不能覆盖当前事实。Host 负责选择允许的资产范围和工具，Python SDK
或 App Server 负责持久化与索引，Core 只看到经过限制的最终上下文。

#### 本地上下文同步

当前讨论中暴露的现实问题是：开发者的主要经验常在本地代码仓库、Markdown、
服务器清单、命令输出和测试报告中，而未来 Bot 可能运行在云端。这里需要的是
受控的上下文导入/同步桥，不是把本地目录无差别复制到云端：

```text
本地来源发现
  → allowlist 选择文件/目录/仓库
      → 按 Role Asset 类型分类
          → 脱敏、版本化、生成来源引用
              → 人工确认后发布到 Bot 可检索范围
```

同步策略必须明确方向、频率、冲突处理、删除传播、敏感信息过滤和访问范围。
本地路径、SSH 凭证、环境变量、客户数据和未提交改动不应因为“上下文同步”
自动进入云端 Bot。反向同步也应优先同步资产和 Artifact 引用，不覆盖本地源文件。

#### 团队岗位映射

一个开发团队可以先建立小而清晰的岗位目录：

```text
产品 Bot      需求、用户反馈、产品 Decision
项目 Bot      计划、风险、依赖、里程碑和会议结论
前端 Bot      UI/组件经验、浏览器问题、前端 Case
后端 Bot      API/架构/性能问题和后端 Runbook
部署 Bot      服务器 Inventory、发布/回滚 Runbook
测试 Bot      测试计划、缺陷 Case、回归结果和质量报告
```

这不是要求每个人都复制出一个独立 Agent。第一版应让每个岗位有明确 owner、
输入源、工作目录、工具边界、交付物和 Handoff 目标；只有重复结果和责任边界
稳定时，才拆成独立 Bot。Bot 的岗位资产可以被团队共享，但写入、修改和废止
需要按资产 scope 授权，避免一个 Bot 的经验未经确认污染所有岗位。

### JobRequest 与 RoutineSpec

为了把“给 Bot 发活”从自然语言愿望变成可恢复的工作合同，可以先在 Host/SDK
边界定义等价于以下的有界结构；这不是要求现在立即新增 Rust 公共协议类型：

```text
JobRequest
  botId / jobId / idempotencyKey
  outcome / sources / constraints
  deliverable / reviewPoint
  input artifact references

RoutineSpec
  ownerBotId / skillId
  schedule or narrow event matcher / timezone
  input source / output destination
  no-data and stale-data policy
  approval boundary / retry and idempotency policy
  pause / test / recent-run metadata
```

`JobRequest` 描述一次结果责任；`RoutineSpec` 描述如何再次产生 Job。二者都应
能映射回 Thread、Goal、Artifact、Approval 和事件，而不是各自拥有一套执行循环。

### Job、Thread 与 Goal

初期不要新增独立 Job 执行循环：

```text
BotSpec + 用户任务
    → 生成 Job identity
    → 选择或创建 Thread
    → 设置 bounded Goal
    → 使用现有 Thread/Turn/GoalRuntime 执行
    → 产生 bounded Artifact
```

一个 Bot 可以服务多个 Job，一个 Project 也可以被多个 Bot 处理。Bot 的长期
身份不应等同于进程、Thread 或 Project；进程可以重启，Thread 可以分叉，Project
可以切换，而 Bot 的岗位定义应保持稳定。

### Handoff

第一版 Handoff 应传递引用和边界，而不是复制完整上下文：

```text
sourceBot
targetBot
jobId
bounded summary
artifact references
acceptance criteria
next action
handoff status
```

交接可以先落在 Session-owned artifact/result store 中，由 Python SDK 编排；当
交接需要跨重启、可恢复、可审计或被多个客户端观察时，再提升为 App Server 的
持久化协议对象。

`turn/steer` 适合修改当前工作，`thread/fork` 适合复制一个 Thread 分支，二者
都不能单独表达“某岗位把一份结果正式交给另一个岗位”。Handoff 应有明确的
发送、接收、完成、失败和人工退回状态。

## 一个最小岗位链路实例

可以用 `.tmp/bot/bot.md` 中的 Sales Outbound 作为第一条实验链路，但先使用
mock connector、测试数据和“只生成草稿”的安全边界：

```text
Sales Outbound Bot
  责任：形成待人工复核的外联草稿队列
  来源：允许的客户表、CRM、研究页面
  交付：每个联系人一份草稿、证据链接、跳过原因和汇总报告
  禁止：发送消息、修改生产数据、重复触达近期联系人

一次 Job
  → 读取当前客户集合
  → 过滤近期已联系对象
  → 研究账户和联系人背景
  → 生成草稿与证据
  → 写入 bounded Artifact
  → 等待结果审批
  → 人工只批准选中的条目
  → 执行批准的发送动作并记录 action log
```

这条链路同时验证五个关键事实：Bot 有稳定责任，Job 有明确结果，来源和约束
可追踪，草稿 Artifact 可以独立复核，外部副作用在明确的 Approval 后才发生。
若执行中发现需要账户健康分析或内容审校，可以创建 Handoff：目标 Bot 只接收
账户范围、草稿 Artifact 引用、验收标准和下一步，不接收无界完整历史。

当一次性 Job 连续几次在安全样例上稳定后，再把“过滤、研究、评分、起草和
复核清单”保存为 Skill；最后才创建每周 Routine。Routine 的默认结果仍是
草稿和复核清单，不能因为它是后台运行就自动获得发送权限。

## Custom Agent 适配策略

Custom Agent 应作为 Host 层的配置输入，而不是 Core 的新概念。

建议先支持一个最小子集：

| Custom Agent 能力 | mini-agent 目标映射 |
| --- | --- |
| `name`、`description` | `BotSpec` 元数据 |
| Markdown body | 有界岗位指令 overlay |
| `tools` | Host 工具 allowlist，仍需经过 ToolRouter/Orchestrator/Runtime |
| `agents` | 有界的可委派 Bot 列表 |
| `model` | 已注册 Provider/Model 的 allowlisted selector |
| `handoffs` | Handoff 目标和下一步模板 |
| `mcp-servers` | 已注册 MCP 能力的声明性引用 |
| `user-invocable` | 客户端显示和调用元数据 |
| `hooks` | 暂缓，不进入 Core |

以下规则必须保持：

- Custom Agent body 只能追加到稳定基础提示词，不能替换 Core safety 或 Host policy；
- 工具权限不能由 Markdown 声明单独授予，必须由 Host 的 allowlist、sandbox 和 approval
  再次确认；
- 公共 App Server 不接收任意 raw system prompt、命令、路径或凭证；
- Agent body、Skill metadata 和 Handoff payload 都必须有界；
- `user-invocable`、`disable-model-invocation` 和 `handoffs` 属于编排元数据，不能
  被误当成安全边界；
- Builtin prompt 继续由 crate-owned Markdown asset 编译期嵌入，Custom Agent 才采用
  有界的运行时来源。

VS Code 的 Ask、Explore、Plan 与 mini-agent 的关系应保持近似而不假设兼容：

- Ask 是面向用户的只读问答角色，mini-agent 当前没有专门的 Ask 基础 Agent；
- Explore 最接近 `AgentKind::Explore`，但 mini-agent 的只读能力由 Host policy 强制；
- Plan 最接近 `AgentKind::Plan`，但 VS Code 的 handoff 不会因为同名 prompt 自动出现；
- 一般实现角色可由 `General + Implementer` 近似表达，但 Implementer 本身不是权限授予。

## 通过 App Server 与 Python SDK 的推进方式

### 第一阶段：Python SDK 证明一个最小岗位链路

先只做一名 Main Bot 和一名专业 Bot：

```text
用户 → Main Bot
       → 判断是否命中专业岗位
       → 读取岗位定义和 Skill
       → 启动或复用 Thread + Goal
       → 产出符合 ResultEnvelope 的 Artifact
       → 返回可核对结果和待审批动作
```

第一阶段先按 `JobRequest` 的 Outcome、Sources、Constraints、Deliverable 和
Review point 完成一次性任务；人工审阅并纠正后，才保存可复用 Skill。第一阶段
不做完整 scheduler、不做多 Bot 群聊、不做通用记忆框架，也不把 Bot 配置复制到
Web Gateway 的第二份会话状态中。Python SDK 负责薄编排、事件流和审批转发；
App Server 仍是 Thread/Turn/Goal/Session 的权威。

由于当前 App Server 没有公共的 Profile/Bot identity 选择入口，Bot 选择必须先
通过一个有界的启动组合或内部 Host seam 完成，不能把完整 prompt body 作为公共
请求传入。这个 seam 的设计需要单独记录 Bot ID、来源、工具范围、目录和策略，
并在 initialize manifest 中只返回非敏感摘要。

### 第二阶段：提升稳定语义到 App Server

只有第一阶段证明岗位定义、结果交付和交接确实需要持久化、恢复和多客户端观察，
才考虑加入有限的 Bot/Job/Handoff 协议面。候选操作可以包括 Bot catalog 的
读取、Job 提交与读取、Handoff 状态读取和 Artifact 引用读取；具体方法名、事件
和错误码应由独立协议批次决定。

这一阶段仍不把 scheduler 放入 Core。调度器只负责按时间或外部事件提交一个带
幂等键的 Job；App Server 负责接收、串行化、执行、审批、持久化和报告结果。
Routine 的创建、测试、启停、时区、窄事件匹配、无数据策略、重试记录和最近运行
记录优先由 Python Worker 或外部调度器承载；每次触发仍必须回到统一的 Job/Goal
执行路径。

### 第三阶段：建设独立的 Bot Studio

第三阶段不是改造当前 Web Studio 的信息架构，也不是把现有 Project/Thread 页面
逐步替换成 Bot 页面。它是在底座语义已经稳定后，新增一个面向长期岗位的 Web
客户端，主要入口是 Bot roster、Job inbox 和 Artifact review：

```text
Bots
├── 负责中的 Jobs
├── 待审批动作
├── 待接收 Handoffs
├── 最近 Artifacts
└── 运行与失败历史
```

Bot Studio 可以链接到某个 Job 的 Thread/Turn 执行证据，但不把现有 Web Studio
的页面或前端状态当作依赖。当前 Web Studio 继续服务调试、一次性任务和 Thread
观察；独立 Bot Studio 服务岗位管理、长期 Job、交接、结果复核和运行状态。两者
共享 App Server 的数据和权限边界，按需共享低层 UI 组件，但保持可独立演进、
部署和验收。

## 后续计划：先打牢底座，再建设独立 Bot Studio

后续工作按“能力先于界面、协议先于产品、结果先于会话”的顺序推进：

### 0. 底座盘点与边界冻结

- 固化 Core、Host、App Server、Python SDK 的责任，不因 Bot 概念改写 Core
  执行循环；
- 梳理现有 `RuntimeComposition`、`AgentKind`、`PersonaKind`、Skill、Goal、
  Approval、Artifact 和事件流，确认哪些可以直接复用；
- 选取一组真实岗位过程文档作为种子数据：故障排查、部署清单、服务器整理、
  测试过程和结果，并标注来源、负责人、新鲜度和敏感级别；
- 把当前 Web Studio 定位为既有客户端和回归观察面，而不是后续 Bot 产品的
  迁移目标；
- 用 bounded scenario 先验证岗位路由、结果交付、重启恢复和审批边界。

### 1. Host 侧形成最小岗位底座

- 先以类型化、allowlist 的 `BotSpec` 或等价 Host 组合承载 Bot 身份、职责、
  工具、Skill、工作目录、沙箱、审批和结果契约；
- 让内置 Agent/Persona 和有限的 Custom Agent 适配都落到同一套受控组合，
  不允许 Markdown body 直接获得权限；
- 让岗位过程资产以 Host 可控的范围和来源引用参与上下文组合，先支持只读检索
  和候选资产提炼，不先建设通用 Memory Framework；
- 明确 Bot identity、Job identity、Thread 和 Project 的生命周期关系；
- 这一步优先服务 Python Worker、测试夹具和 App Server 内部 seam，不要求 Web
  UI 同步改造。

### 2. App Server 与 Python SDK 形成可恢复的工作语义

- 用一个 Main Bot 和一个专业 Bot 验证 Job、Goal、Artifact、Handoff、审批、
  重启和幂等触发；
- 先复用现有 Thread/Turn/Goal/Session，不满足证据要求时才增加有限的 Bot/Job/
  Handoff 协议对象；
- 将 Python SDK 保持为薄编排层，App Server 继续拥有执行、事件、持久化和
  恢复权威；
- 增加 Job 完成后的资产候选生成和人工确认路径，把 Case/Runbook/Inventory/
  Decision 与原始 Artifact 建立可追溯关系；
- 让命令行或后台 Worker 可以在没有 Web Studio 的情况下完整运行一条岗位链路。

### 3. 补齐长期运行所需的控制面能力

在最小链路被场景证据证明有效后，再按需加入 Bot Catalog、Job 生命周期、有限的
岗位记忆、Artifact 复核、外部 Scheduler、Connector 凭证边界和 Bot 间 Handoff。
这些能力仍应放在 Host、App Server、Python Worker 或专门的外部服务中，不进入
Core，也不要求当前 Web Studio 承担管理入口。

### 4. 新建独立 Bot Studio

当底座已经能够在无 Web 的环境下稳定完成岗位链路，再建设独立 Bot Studio：

- Bot roster：岗位、职责、能力、健康状态和负责人；
- Role assets：Runbook、Case、Inventory、Decision 的检索、确认、版本和过期状态；
- Job inbox：待处理、执行中、阻塞、待审批、待复核和已完成工作；
- Presence：idle、thinking、working、waiting、blocked、done 等可解释状态；
- Artifact review：以交付物、验收标准和外部后果为中心，而不是只展示聊天记录；
- Handoff/Run history：可追踪交接、重试、失败和恢复；
- Bot workspace / Job workspace / User workspace 的明确隔离。

Bot Studio 的第一版可以是新的 Web 应用或新的前端入口，通过 App Server 协议
和 Python SDK 使用底座；它不需要等待当前 Web Studio 完成迁移，也不应为了复用
页面而把 Bot 状态写入当前 Web Studio 的第二套前端会话模型。

因此，短期交付重点不是“把 Web Studio 改成 Bot Studio”，而是让未来任何客户端
都能可靠回答：哪个 Bot 负责、Job 到哪一步、交付物是什么、下一步是谁接手、
什么动作需要人批准。

## 长期记忆、调度和 Human in the loop

### 岗位记忆

Session 保存“某次工作发生了什么”；Bot Memory 保存“这个岗位长期确认了什么”。
两者必须分开：

```text
Bot Memory：长期规则、已确认偏好、稳定方法、可复用事实
Job Session：当前输入、当前工具调用、当前事件、当前结果
```

在开发团队场景中，Bot Memory 更适合作为岗位过程资产的有界索引和摘要；可审阅、
可引用的 Runbook、Case、Inventory、Decision 才是长期资产，原始 Session 和
命令日志保留在 Job/Artifact 证据链中。这样既能让岗位持续变聪明，也能在记忆
过期时定位和撤回具体来源。

记忆写入需要来源、时间、置信度和人工修正边界。每次请求只检索有界摘要，不能
把所有历史自动拼进 system prompt。用户明确纠正的岗位规则可以更新 Bot Profile
或 Bot Memory，但不断变化的业务事实仍应回到源系统；删除 Bot 也不能默认删除
共享 workspace 中的文件、浏览器会话或已交付 Artifact。

### 人工审批

当前 ApprovalController/ApprovalBroker 已能处理工具动作前的审批。Bot 模式还需
区分两种门：

```text
动作审批：是否允许执行写入、发布、发送、付款或删除
结果审批：交付物是否允许成为正式发布、外部通知或下一岗位输入
```

第一种继续复用现有 approval scope、path scope 和 policy；第二种应先在 App
Server/SDK 层作为 Job/Artifact 状态实验，不把“结果已审阅”伪装成工具 approval。
密码、Passkey、二次验证码、CAPTCHA、付款确认等敏感步骤应转交用户 takeover；
它们不能通过普通聊天传入。若未来引入 Auto Review，它只能作为动作前的附加审查
层，不能替代最小权限、工具 allowlist、网络限制和显式人工审批。

### 定时任务

定时任务至少需要负责人、时区、输入来源、版本、幂等键、重试策略和停止条件。
没有这些信息时，不应把一个普通 Goal 直接变成无人值守任务。发布、删除、付款、
退款、权限修改和生产变更继续停在 Human in the loop。创建 Routine 前必须有
安全样例测试；源数据缺失或过期时报告失败，不得静默使用旧数据。网站、Connector
或输入格式改变后应重新测试并允许暂停 Routine。

## 最小实验与证据门槛

第一批只验证真实的岗位结果，不建立通用 benchmark framework。建议使用本地
mock provider、临时 workspace 和现有 App Server/SDK 公共路径。

| 场景 | 要证明的事实 |
| --- | --- |
| Main 路由正式写作任务 | 专业岗位规则和 Skill 被正确选中 |
| Main 处理普通问题 | 不因关键词相似而误启用完整岗位流程 |
| 缺少 JobRequest 字段 | Bot 会补问或显式标记缺口，不会默认扩大权限 |
| ResultEnvelope 复核 | 事实、推断、已完成动作、待审批动作和未解决项可区分 |
| 同一 Bot 处理两次不同 Job | 长期岗位规则稳定，当前任务没有污染下一次任务 |
| Job 产出 Case 候选 | 排查过程、日志和验证结果可生成候选资产，并等待负责人确认 |
| Runbook/Inventory 过期 | 环境或版本变化后资产会标记 stale，并要求重新验证 |
| Decision 被新决策取代 | 新旧版本、决策人和适用范围可追踪，不继续误用旧结论 |
| 本地上下文受控导入 | 只同步 allowlist 来源，敏感信息被过滤，资产有来源和版本 |
| Bot → Bot 文件交接 | 目标 Bot 只收到有界摘要和 Artifact 引用 |
| 交接后失败或人工退回 | source/target 两边都能看到明确状态和原因 |
| App Server 重启后恢复 | Job/Goal/Thread 不重复执行已结算的副作用 |
| 重复定时触发 | 幂等键阻止重复写入、重复发送或冲突文件 |
| Routine 安全测试和旧数据 | 缺源、旧源、格式变化和部分完成都有显式结果 |
| 外部副作用审批 | 动作审批和结果审批都可观察、可拒绝、可恢复 |
| 跨 Bot 目录和工具边界 | 一个岗位不能因路由或 prompt 获得未授权资源 |
| 云端与本地执行隔离 | 云端 workspace 不会自动获得本地文件和命令权限 |
| Bot 生命周期操作 | Duplicate/Share/Hide/Delete 不错误复制历史或删除共享资源 |
| 无 Web 运行 | Python Worker 或命令行可以独立完成同一 Job 链路 |

每个场景至少记录：输入、Bot/Skill 选择、允许工具、事件摘要、最终 Artifact、
Session/Goal 状态、step/tool/token/byte 使用、耗时、失败分类和边界违规。不得
调用付费 Provider，不把单一模型分数当作完成标准。

## 失败信号与撤回条件

出现以下结果时，应暂停扩展 Bot 层，而不是继续添加抽象：

- 专业 Bot 没有提升重复任务的结果稳定性；
- Main 路由错误多于直接使用单一 General Agent；
- 岗位上下文和记忆使模型可见输入持续膨胀；
- Handoff 需要复制完整历史才能工作；
- scheduler 重试无法可靠避免重复副作用；
- Custom Agent 的 Markdown 规则与 typed tool/policy 产生无法解释的冲突；
- 人工审批点不清晰，用户无法判断批准的是动作还是结果；
- Bot Studio 必须复制 Web Gateway 的状态才能解释 Bot/Job；
- Routine、Memory 或 Computer 的新增层无法在无 Web 环境下被测试和恢复；
- 岗位过程资产无法比直接使用原始文档提升重复任务的结果稳定性；
- 资产过期、来源断裂或本地/云端同步冲突无法被发现和人工撤回。

若失败，应优先删除 Bot/Job/Handoff 的新增概念，退回现有 Thread/Goal/Skill
流程，而不是以更多配置掩盖证据不足。

## 六项变更准入

1. **层级**：Bot 定义、资产范围和 prompt/tool/policy 组合属于 Host；Job、Handoff、
   Artifact、岗位过程资产的版本和公共生命周期属于 App Server；Python SDK 负责
   薄客户端与提炼编排；Gateway/Web 负责投影和交互；Core 保持现有执行循环、
   限制、停止分类和事件责任。
2. **重复责任**：复用 `RuntimeComposition`、`AgentKind`/`PersonaKind`、Skill、
   Thread、GoalRuntime、SessionStore、ResultStore、Approval 和现有事件流；岗位资产
   只建立在 Artifact/索引之上，不新增第二套 Thread、Goal、Session、记忆库或工具
   执行器。
3. **替换优先**：第一阶段先用 Bot Catalog、现有过程文档、Artifact 索引和
   `thread/start`/`thread/goal/set` 验证链路；只有确认现有 Thread/Goal 无法表达
   持久 Job/Handoff/资产版本时，才新增 App Server 对象。Hooks、通用 scheduler、
   通用 memory framework 和多 Bot 群聊不作为第一批能力。
4. **行数预算**：本提案只增加一份中文 note，runtime/release Rust 行数增量为零。
   后续实现默认 Core 净增量为零；Host/App Server/SDK 的增量必须在每批记录预期、
   实际和抵扣，并运行现有 line budget 检查。
5. **模型可见面**：Bot body、Skill metadata、岗位资产摘要、本地上下文导入、
   Memory、Handoff payload、新事件和持久化记录都会扩大模型或客户端可见面，必须
   设置字节、数量、历史、来源和 Artifact 引用上限；公共协议不得接受任意 raw
   prompt、命令、路径或凭证。
6. **边界证据**：使用现有 Core/Host/App Server/SDK/Web 公共测试作为最低覆盖，
   另加上文 bounded Bot routing、handoff、restart、idempotency、approval、memory
   isolation、asset promotion、asset staleness 和 context sync 场景。仅有 prompt
   单测不能证明长期岗位行为。

## 非目标

- 不复制 Codex 或 Grok Bot 的全部产品功能；
- 不把 Bot 直接等同于 Project、Thread、Session 或进程；
- 不在 Core 中加入 Bot、scheduler、memory、connector、handoff 或 policy framework；
- 不默认实现云电脑、OAuth、群聊、动态模型路由或跨组织 Agent 市场；
- 不允许 Custom Agent body 绕过 Host 的工具、路径、沙箱和审批策略；
- 不把所有本地文件、聊天历史、命令输出自动同步成云端 Bot 记忆；
- 不把未经确认的 Case、Runbook、Inventory 或 Decision 当作全团队事实源；
- 不因为支持 VS Code `.agent.md` 就承诺与 VS Code/Copilot 完整协议兼容；
- 不以“自动运行”替代 Human in the loop 的外部结果确认。

## 预期结果

如果第一阶段证据成立，mini-agent 将从：

```text
用户指挥 → Thread 执行 → 返回答案
```

扩展为：

```text
用户交付结果责任
  → Main Bot 路由
  → 专业 Bot 执行 Job
      → Skill 驱动工作方法
          → App Server 保存过程和状态
              → 提炼并确认岗位过程资产
                  → Human 审批外部后果
                      → 交付并可继续复用
```

这会超越当前 Web Studio 的 Thread-first 交互，但不要求改造或废弃当前 Web Studio。
未来由独立 Bot Studio 提供 Bot-first 体验；它与当前 Web Studio 一样，建立在
同一条 `Core → Host → App Server → Python SDK → Web` 权威链路上。

## 依据

- [如何从0到1用Codex打造一支多Agent数字员工团队](../../../../.tmp/bot/如何从0到1用Codex打造一支多Agent数字员工团队.md)
- [GrokBot从入门到精通](../../../../.tmp/bot/GrokBot从入门到精通.md)
- [Designing Grok Bot for a world of persistent agents](../../../../.tmp/bot/designing-grok-bot.md)
- [评论导出：Codex 数字员工](../../../../.tmp/bot/comment/export_20260904-093446.csv)
- [评论导出：Grok Bot](../../../../.tmp/bot/comment/export_20260904-093725.csv)
- [Grok Bot 官方入门](https://docs.x.ai/grok-bot/get-started)
- [官方：创建与管理 Bot](https://docs.x.ai/grok-bot/bots)
- [官方：消息与协作](https://docs.x.ai/grok-bot/chat-and-collaboration)
- [官方：文件与结果](https://docs.x.ai/grok-bot/files-and-results)
- [官方：Computer 与应用](https://docs.x.ai/grok-bot/computer-and-apps)
- [官方：Skills、Routines 与自动化](https://docs.x.ai/grok-bot/skills-routines-and-automations)
- [官方：审批、安全与隐私](https://docs.x.ai/grok-bot/approvals-security-and-privacy)
- [官方：Grok Bot 安全模型](https://docs.x.ai/grok-bot/security)
- [当前配置与 Prompt/Rule 组合](../../../../docs/configuration.md)
- [当前 App Server 契约](../../../../docs/app-server.md)
- [当前 Web Studio 集成边界](../../../../docs/studio-integration.md)
- [当前 Harness 分层](../../../../docs/harness-framework.md)
- [当前 Harness 边界](../../../../docs/harness-boundaries.md)
