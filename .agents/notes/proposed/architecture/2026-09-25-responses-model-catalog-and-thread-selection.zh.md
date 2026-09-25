# Responses 多供应商目录与 Thread 模型选择

状态：提案中；实现与针对性验证已在本地工作区完成，等待后续集成
日期：2026-09-25
适用范围：Harness、App Server、Python SDK、Gateway、Web Studio

## 结论

模型目录是运行 Host 所在机器上的共享配置。供应商和模型用稳定的
`{provider_id, model_id}` 引用；API Key 由 Host 写入系统凭据库，普通配置查询只
返回是否已配置。主模型统一走现有 Responses 路径；不兼容的端点明确失败，不降级
到 Chat Completions。

全局默认由模型引用和推理选择组成；推理选择可以是省略推理参数的
`api_default`，也可以是该模型声明的任意等级。`disabled` 是模型等级枚举中的普通
值，靠模型参数映射表达对应供应商的关闭请求，不是额外的全局开关。用户还可以在
输入框中频繁切换 Thread 当前等级。

项目默认模型、Thread 显式选择和 Goal Verifier 默认模型是分开的配置维度。Thread 的
显式选择优先；没有项目 UI 默认值时保留旧项目 `OPENAI_MODEL`，再使用全局默认模型与
等级。项目默认只指定模型；Thread 没有等级覆盖时采用该模型的 API 默认。Goal 创建时
记录专用 Verifier 模型，修改默认值不改变已有 Goal；缺少 Verifier 配置时明确失败，
不借用主模型。

供应商和模型管理由 Host 持有。App Server 提供本地管理与 Thread 设置协议，SDK 和
Gateway 负责传递，Web Studio 提供设置页和输入框选择器。智能匹配只生成本地建议；
手动编辑后按手动配置管理。供应商 Base URL 不预填，由用户配置。

## 所有权与禁止重复

| 状态或行为 | 权威 | 其他层职责 | 禁止事项 |
| --- | --- | --- | --- |
| 供应商目录、模型资料、全局与项目默认值 | Host `ModelCatalogStore` | App Server 暴露受限管理接口 | Gateway、SDK、Web 保存第二份配置或凭据 |
| API Key | Host 系统凭据库 | Web 只提交新值并读取 `apiKeyConfigured` | 将 Key 放进目录、查询响应、事件或日志 |
| Thread 模型覆盖与推理选择 | Session 的 Thread 设置 | SDK/Gateway/Web 发起设置更新 | 正在运行的 Turn 中途换模型 |
| 当前 Turn 的模型引用 | Turn 输入快照，由 Host 解析并构建 Provider | Core 保留可移植模型引用 | Core 管理供应商、凭据或配置存储 |
| Goal Verifier 选择 | Goal 创建时保存的模型引用 | Host 独立构建无工具 Verifier | 回退到主模型或读取主会话历史 |

## 三批次边界

### 批次一：Responses 运行路径

- Core/Protocol 只携带可移植模型引用和推理等级；Host 在新 Turn 解析引用并构建
  Responses Provider。
- 供应商 Base URL、模型参数和 Key 留在 Host/Capabilities 边界。
- 保持流式文本、推理事件和工具调用；非 Responses 端点报不兼容错误。
- 停止条件：模型选择导致 Core 持有凭据或 Host 权限状态出现影子副本。

### 批次二：机器级目录和设置

- Host 机器级目录保存供应商、模型资料、全局主模型及其推理选择、独立 Verifier
  默认值和项目默认值；凭据写入操作系统凭据库。
- 管理 API 不回传 Key。模型改名在一次目录写入中同步更新主模型、Verifier 和项目
  引用；删除则清除相应默认引用。
- 设置页可新增、编辑、启停、删除供应商和模型。智能匹配不是远程探测，也不能
  覆盖已手动编辑的模型。
- 停止条件：任何配置查询或日志序列化出凭据值。

### 批次三：项目、Thread 和输入框

- 新 Thread 解析顺序为显式 Thread 选择、项目 UI 默认、兼容旧项目 `OPENAI_MODEL`、
  全局默认。
- Thread 模型引用和 typed 推理选择持久化；选择可以是 `api_default` 或该模型支持的
  任意等级（包括 `disabled`）；Fork 与子会话继承；选择变更从下一 Turn 生效。
- 输入框按供应商分组选择模型和推理等级。被停用、缺少 URL 或缺少凭据时给出原因并
  阻止发送。
- Goal 创建时快照独立的 Verifier 默认值。后续修改只影响新 Goal。
- 停止条件：UI 投影取代 Host 配置权威，或 Verifier 与主模型发生隐式共用。

## 跨仓契约

| 边界 | 新增契约 | 责任 |
| --- | --- | --- |
| Core ↔ Host | `ModelSelection`、`ReasoningSelection` 进入 Turn 模型请求 | Core 保持通用；Host 解析到具体 Responses 模型和推理参数 |
| Host ↔ App Server | 目录管理、全局/项目默认值、Goal 创建快照 | App Server 转发和保存运行状态，不持有 Key |
| App Server ↔ SDK | `model/catalog/*` 管理请求及 Thread 模型设置 | SDK 负责类型与 RPC 调用 |
| SDK/Gateway ↔ Web | 供应商/模型目录和 Thread 设置 API | Gateway 不解析或持久化凭据 |

## 兼容与安全边界

- 兼容 `OPENAI_*` 旧主模型配置；项目现有 `OPENAI_MODEL` 在未设置项目 UI 默认值时
  继续生效。
- 兼容 `VERIFIER_OPENAI_*` 旧 Verifier 配置；Verifier 不回退到主模型。
- 供应商类型包括 DeepSeek、Kimi、GLM、字节火山及自定义 Responses 兼容端点。Base
  URL 留空等待用户填写，不按供应商名称推断地址。
- 文件目录原子替换；Unix 权限设为仅当前用户可读写。Windows 使用替换既有目标的
  原子文件 API。
- 不执行 Chat Completions 自动探测或协议回退。实际端点是否支持 Responses，由用户
  配置的端点能力决定。

## 验收与当前证据

- 本地模拟 Responses 服务验证所选模型的请求路由、流式输出、工具调用、推理参数和
  请求中不包含 API Key。
- Rust 定向包测试和 Clippy 通过，覆盖目录凭据脱敏、默认模型等级校验、`disabled`
  映射到 Responses 请求、`api_default` 省略推理字段、改名引用同步和 Thread 持久化。
- Web 模型选择/管理组件测试通过；Gateway/SDK 定向测试通过；前端 lint、生产构建、
  Python Ruff 和 `git diff --check` 通过。
- 当前 Rust 硬预算为 Core + Protocol 4,767/6,000、Control Plane 29,999/30,000、
  Release 43,036/45,000。Control Plane 仅剩 1 行预算；后续实现应先删减或替换现有代码。
- 相对 `origin/main` 的增量检查为 Release +2,144，有效行数超过每个 PR 的 +1,000
  增量额度。集成应按可独立审查的批次拆分，或先减少净增量。
- Windows Host 完整交叉编译尚未验证：当前 Mac 缺少 Windows SDK `windows.h`，使
  `aws-lc-sys` 跨目标构建停止。新增 `MoveFileExW` 用法已在独立 Windows 目标探针中
  编译通过；仍需 Windows 原生环境验证凭据库及运行时文件替换。
- 没有调用付费供应商 API。

## Change admission 回答

1. **责任层**：模型协议引用在 Protocol/Core；Provider、目录、凭据和模型构建在
   Host/Capabilities；目录 RPC 在 App Server；调用包装在 SDK/Gateway；交互在 Web。
2. **既有责任**：复用现有 Responses Provider、Session Thread 设置、Goal Verifier
   生命周期和 App Server 通信；未在 Web/Gateway 建立第二套模型运行时。
3. **替换旧概念**：旧 `OPENAI_*` 配置保留为迁移兼容入口；目录模型是新配置权威。
   没有加入 Chat Completions fallback。
4. **预算**：当前总量均在硬上限内；Release 增量 +1,809 超出单 PR +1,000 额度，需
   在 PR 集成前按批次拆开或精简。
5. **可见变化**：新增模型选择、模型管理、Thread 持久化及 Goal Verifier 模型快照；
   凭据仅进入系统凭据库，不进入公共协议响应。
6. **边界证据**：Rust 单元测试、Web/Gateway/SDK 测试和本地 mock Responses 覆盖
   主要契约；Windows 原生环境与全 Host 跨编译仍缺证据。

## 待集成事项

本记录保持提案状态，直到实现被合并并按目标平台补齐验证。当前硬行数上限有余量，
但 Control Plane 仅余 259 行；任何追加都应先删除或替换已有概念。PR 增量需要重新
按最终基线计算。
