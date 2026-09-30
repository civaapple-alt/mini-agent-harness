# ask_user 在 Plan、Goal 与子 Agent 中的使用边界

- status: implemented
- date: 2026-10-01

## 决策

- `ask_user` 常用于 Plan Mode 的重要方案选择，以及 grill-me 等访谈式技能的逐项澄清。
- Goal Mode 应先自主探索，靠验证和可逆假设处理常规不确定性；只在被用户本人必须决定的事项阻塞时才提问。
- 子智能体保留直接调用 `ask_user` 的能力。不得要求子 Agent 把问题交回父 Agent 再调用问答工具，以免增加主子模型往返。
- 子 Thread 的问题仍由 App Server 按 Thread / Turn 管理；父会话只显示待答状态，答案在对应子会话提交。

## 准入问题

1. **归属：** 工具使用指引和 Plan / Goal 提示属于 Host；会话列表、子任务面板是 Gateway / Web 投影。问答执行、Thread/Turn 身份和持久化仍归 App Server。
2. **现有所有者：** 复用 `AskUserTool`、Goal 与 Plan 提示、Session 执行日志、`thread/read` 和 Web 的子会话面板；不新增第二套问答循环或状态源。
3. **替换旧概念：** 采用现有问答工具直达用户，不引入“子问题转交父问题”的代理协议。未扩展 App Server RPC。
4. **有效行数：** 相对 `HEAD` 预期 Core + Protocol `0`、Control Plane `+12`、Release Rust `+12`；实测相同。新增预算为 Core + Protocol `7,000`、Control Plane `45,000`、Release Rust `65,000`。
5. **可见影响：** 修改 `ask_user` 工具说明和 Goal / Plan 提示；Gateway 子任务响应增加 `awaiting_user_input` 元数据，并为父会话增加待答 attention reason。没有新增 App Server 事件或持久化状态。
6. **边界证据：** 用有界 App Server `ask_user` 场景检查工具指引和回答往返；Host 单测检查 Plan / Goal 提示；Web 覆盖日志投影、子 Thread 等待态、父会话标识与打开回答入口。

## 实施与验证

- `crates/mini-agent-host/src/user_questions.rs` 指明 Plan Mode 和访谈技能的适用场景，并说明 Goal 自主推进、子 Agent 直达用户的规则；`goal.rs` 为 Plan 和 Goal 提示提供一致指引。
- Session execution journal 的未答 `execution_user_question` 投影为 bounded 的 `awaiting_user_input` 标志；对应工具调用完成或 Turn 结算时清除。App Server `thread/read` 仍提供完整问题和答案权威数据。
- Harness：`cargo fmt --all --check`、`cargo clippy -p mini-agent-host -p mini-agent-app-server --all-targets -- -D warnings` 通过；两个包测试共 `145` 项通过；`python3 scripts/test_line_budget.py` 的 `17` 项通过。
- 行数：`python3 scripts/line_budget.py --base HEAD --check-delta --json` 显示 `0 / +12 / +12`，无预算违规。默认报告为 `6,170/7,000`、`37,847/45,000`、`54,909/65,000`。
- Web：子问题投影、父会话冻结保留问题、待答子任务停止等 Session Manager 定向测试 `3` 项通过；Gateway API `18` 项和两个前端组件测试 `22` 项通过；ESLint、Ruff check/format、Vite build、文档链接检查及 `git diff --check` 通过。
- 一次更宽的 `test_session_manager.py` + `test_gateway_api.py` 联合运行有 `34` 项失败、`140` 项通过；失败覆盖项目隔离广播和子任务队列 / 控制用例，未在本次范围内定位。待答投影定向用例和 Gateway API 单文件用例均通过。
