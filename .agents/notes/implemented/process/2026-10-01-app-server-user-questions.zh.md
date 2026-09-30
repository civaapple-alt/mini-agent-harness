# App Server 问答工具实施记录

## 目标与边界

给 Web Studio 增加可恢复的 Agent 澄清交互。Core 仍把 `ask_user` 当作普通
工具调用；Host 负责有限输入校验和工具执行；App Server 是交互状态、身份
校验、答案持久化和 Turn 等待/恢复的唯一所有者。SDK 与 Gateway 只转发通知、
当前交互投影和回答命令。

仅在客户端 `initialize.capabilities.userQuestions=true` 时注册并公开工具。
每次最多三题，每题最多六个单选项；允许文字回答和跳过。每题至多一个选项
带 `recommended`，同时可附一条短理由。UI 强调推荐但不预选或自动提交。

## 变更准入记录

1. **所属层：** Host 定义并校验工具输入，Capabilities 持久化执行日志，App
   Server 独占交互状态、提交校验和 Turn 等待；Protocol 暴露有界消息类型。Core
   仍按普通工具调用推进，不加入问答专用循环；CLI 不变。
2. **重复职责：** 检查了 Host `ToolHandler`/`ToolOrchestrator`、`HostRuntimeFactory`、
   App Server 客户端能力协商、JSON-RPC 通知与 `thread/read`、Session 执行日志。
   它们分别承接工具准入、客户端协商、状态投影和恢复；没有现成的逐题等待与回答
   身份校验职责，因此交互状态归入 App Server Broker，并复用 Session 日志。
3. **旧概念：** 没有可替换的旧问答状态或另一套 Core 循环。实现复用普通工具路由、
   客户端能力协商和现有 Session 日志，没有新增 Gateway 状态源或授权机制。
4. **行数预算：** 相对基线 `d8342c0`，Core + Protocol `6101 -> 6170 (+69)`，
   Control Plane `36326 -> 37835 (+1509)`，Release Rust `53312 -> 54897
   (+1585)`。绝对上限分别为 `6500`、`38000`、`55000`；预算检查通过。每 PR
   `1000` 行是评审参考值，不是硬门禁。
5. **可见面变化：** 新增 `userQuestions` 协商能力、请求/更新/回答方法，以及
   `waitingForUserInput` 阶段。只向声明能力的客户端暴露工具；每次最多 3 题、每题
   最多 6 项、每题至多 1 个推荐项。问题、逐题回答和工具结果进入既有执行日志，答案
   先持久化再确认；不会记录文件正文或新增 Gateway 状态。
6. **边界测试：** App Server JSON-RPC、Broker 竞争与幂等、Session 日志恢复，以及
   mock-model bounded Harness Scenario 覆盖协议、恢复和答案进入下一轮模型输入。
   相关五包 Clippy、Rust 测试、fmt、Web SDK/Gateway/UI/状态测试、文档链接和预算
   结果见下方验证证据。

## 执行与持久化

Host `AskUserTool` 对问题数、题目和选项长度、推荐标记、自由文本及跳过能力做
边界校验，再委托 App Server `UserQuestionBroker` 等待逐题答案。Broker 按
interaction、Thread、Turn、工具调用和当前题校验提交；同题相同答案幂等，冲突、
越序和过期提交拒绝。接受答案前，将完整交互快照同步写入现有 Session 执行日志。

Broker 通过 `user-question/request` 和 `user-question/updated` 发送新题、逐题
进度、完成和取消状态；等待期间运行状态为 `waitingForUserInput`。`thread/read`
仅给已协商能力的客户端附上未完成交互。恢复时从 Session 日志恢复同一批问题和
已提交答案；Host 工具重放后复用已有答案，未回答题继续等待。工具最终答案仍以
正常工具结果进入下一次模型请求。

Web Studio 按 Thread 路由 `user-question/respond`，不在 Gateway 保存交互状态。
会话流逐题显示当前问题、推荐标记与理由、自由文字输入和跳过；完成后问答卡默认
折叠，展开显示题目及答案标签/文本/跳过状态。回答只支持文字，不引入语音权限。

## 验证证据

- App Server JSON-RPC 测试检查能力声明、按身份提交答案和跨 Thread 拒绝。
- 有界 mock-model Harness Scenario 覆盖三题：推荐选项、无推荐选项、自由文本、
  跳过；确认工具答案完整进入后续模型请求。
- Broker 测试覆盖顺序回答、早先相同答案重试、冲突回答、错误作用域和取消后的
  过期提交，包括取消标记置位后立即提交的竞争窗口；Session 测试覆盖部分
  已回答和未回答题目在重载后恢复。
- Web SDK/Gateway 测试覆盖能力声明、Thread 路由答案和 Thread 定向通知；组件
  和状态测试覆盖推荐、不自动提交、折叠历史、Enter 提交及等待状态文案。
- `cargo test -p mini-agent-protocol -p mini-agent-host -p mini-agent-capabilities
  -p mini-agent-app-server-protocol -p mini-agent-app-server -- --test-threads=1`
  通过，相关 Rust 单元测试共 336 项；对应五个包的 `cargo clippy --all-targets
  -- -D warnings` 和 `cargo fmt --all` 通过。
- 有界 Harness 场景与 Session 重载测试单独通过；Web 定向 SDK/Gateway 测试
  `5 passed`，另有 Gateway 通知测试 `1 passed`；问答工具 UI `19 passed`，
  运行状态测试 `9 passed`。
- 行数预算通过：Core + Protocol `6170/6500`，Control Plane `37835/38000`，
  Release `54897/55000`。未调用 Provider。
- 相对 `d8342c0` 的增量为 Core + Protocol `+69`、Control Plane `+1509`、
  Release `+1585`；1,000 行增量为审查参考值，不是硬门禁，绝对预算检查无违规。
