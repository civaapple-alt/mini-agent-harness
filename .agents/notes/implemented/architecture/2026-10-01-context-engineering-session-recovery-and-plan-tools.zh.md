# Session 超长工具输出恢复、模型耗时与 Plan Mode 工具约束

状态：implemented（已本地提交；尚未推送）

日期：2026-10-01

## 决策

Session 的 ResultStore 负责超长工具正文的持久化与生命周期。正文写入 sidecar，日志仅保存句柄和必要元数据；句柄限定在所属 Session 内。每个 Session 最多保存 8 项、每项最多 8 MiB、合计最多 16 MiB。`read_tool_output` 通过 cursor 分页读取，响应有界；工具输出达到 Core 的 16 KiB 截断阈值时，Host 保留头尾预览和取回句柄。写入失败或配额不足时明确说明全文未保存，并返回有界预览。`ToolExecutionOutcome` 显式携带截断状态，因此提示文字长度不会改变 `ToolFinished.truncated`。

压缩摘要保留目标、已验证事实、关键失败证据和现存文件或结果句柄。Goal 开始时读取 `goal/plan.md`，完成阶段前用实际 `git diff` 对照计划核实进度。执行过程不自动重注入 Todo，也不增加通用记忆存储。

`model_responded.model_timing` 是可选字段，记录请求发出至首个非空生成内容的 TTFT 和响应总时长；Turn Presentation 保存最近一次指标。Gateway 投影、Python SDK typed event 和 Web Studio 支持新字段，旧事件与历史记录缺少字段时仍可读取，界面显示 `—`。无 Provider 用量数据时保持未知，不推算费用。

Plan Mode 的工具选择由 Host 根据 `PlanModeState` 得出：允许 `read_file`、`read_image`、`shell`、`apply_patch`，以及已配置的只读 `web_fetch`、`web_search` 和 `ask_user`。`review_pending` 时不发起模型请求。模型请求保留完整且稳定的工具定义，同时将选择集合传给 Provider 适配器和 `ToolOrchestrator`；OpenAI Responses 使用原生 `allowed_tools`，不支持该能力的适配器收到简短上下文提示。越界请求会被 Host 以有界 `Deferred` 结果拦截，不执行副作用。允许的工具调用仍经过既有 admission、审批、沙箱和 `ActionGrantKey` 检查。

## 验证证据

- Rust：`cargo fmt --all`、Core、Protocol、Capabilities、Host 与 App Server 定向测试（378 项）及这些包的 all-targets Clippy 均通过。
- 跨仓恢复场景覆盖超长 Shell 输出、分页读取、上下文压缩、服务重启及恢复后继续读取；Plan Mode 场景覆盖工具选择约束、稳定工具定义、原生 Responses 参数及不支持原生约束时的提示。
- Web：SDK 与 Gateway 定向测试 47 项通过；前端定向测试共 101 项通过；Ruff、前端 lint/build 和 Python 协议兼容 Cookbook 均通过。
- `python3 scripts/line_budget.py` 通过：Core + Protocol 为 6,256 / 7,000 行，Control Plane 为 39,021 / 45,000 行，Release Rust 为 56,205 / 65,000 行。相对本轮计划采用的基线，增量分别为 +86、+1,174、+1,296 行。
- 变更使用 scripted provider 验证，未调用付费 Provider。

## 剩余限制

尚未新增成功 Goal 的输入 Token、缓存命中率、TTFT 分布、重复失败、恢复结果与场景质量汇总报告，也未进行真实模型的成对质量评测；不能据此声称成功率或缓存成本已改善。Gateway `test_session_manager.py` 的整文件运行有 170 项通过、34 项子 Session 广播相关失败；本轮新增 timing 投影的定向测试通过，但尚未确定整组失败与本次改动的因果关系。该失败组需单独调查。
