# Decoupling Approval Policy and Action Grant Scope (解耦全局审批模式与单次动作授权范围)

* **日期**: 2026-09-07
* **状态**: implemented
* **进度说明**: 已按评审意见完成 canonical contract、底座控制面、App Server、SDK、Gateway、Studio/TUI 对齐，并通过确定性跨层回归；保留的限制见“剩余风险”。
* **Class**: architecture
* **范围**: `Capabilities` (`ApprovalController`, `ApprovalStore`), `App Server` (`ApprovalBroker`, Protocol), `SDK`, `Web Gateway / Studio`
* **关联模块**: `mini-agent-capabilities`, `mini-agent-app-server`, `mini-agent-web`

---

## 1. 背景与问题动因 (Context & Problem Statement)

在 Web Studio 及 CLI 的日常使用中，用户与开发者普遍反馈了一个严重的认知错位与交互断层：

1. **“Auto Copilot 就绪”与频繁拦截的矛盾**：
   - 用户在界面上将执行范围设为 `full_machine`（完全访问）、审批模式设为 `current_session` 或 `current_project`，底部显示 `Auto Copilot 就绪`；
   - 用户期望此时 Agent 进入自主开发模式，不再被琐碎的单步操作打扰；
   - 但实际上，Agent 每次运行编译测试、打补丁或调用不同命令时，仍然高频弹出 `Action Intercepted`（待审批操作）拦截窗口，完全无法做到自动长程推进。
2. **底层概念降维与命名冲突**：
   - 在 `mini-agent-capabilities` 中，原本定义了两个独立概念：
     - `ApprovalMode`：`Interactive`（交互拦截）vs `Automatic`（自动放行）；
     - `ApprovalScope`：`PerAction`、`CurrentSession`、`CurrentProject`（授权记忆的复用生命周期）；
   - 但在向 App Server 协议暴露时，系统直接写死了 `ApprovalMode::Interactive`，并把 `ApprovalScope` 错误命名为了 `ApprovalMode`，强行缩减为一个维度；
   - 导致用户以为选了“当前会话/当前项目”是开启了“会话内/项目内自动执行”，但底层仅仅是对**完全相同的命令字符串**做 Hash 记忆，且单次审批弹窗返回的 scope 还被 App Server 丢弃。
3. **弹窗层缺乏细粒度的动作授权粒度**：
   - 用户在需要拦截审查时，理应能自主决定：“当前这个命令（如 `cargo test`）是只允许这一次，还是在当前会话中记住，还是在整个项目中信任”；
   - 当前弹窗把这个动作记忆范围与全局策略混为一谈，造成交互语义模糊。

---

## 2. 整体解决方案架构 (Overall Solution Architecture)

将混淆的审批机制彻底解耦为**两个正交维度**：

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│ 维度 1：全局执行与安全策略 (Approval Policy)                                │
│ ─────────────────────────────────────────────────────────────────────────── │
│ • Interactive (交互把关)  : 敏感工具默认拦截，等待人工仲裁确认                │
│ • Automatic   (自动副驾)  : 非高危敏感动作自动放行，实现真正的 Auto Copilot  │
└─────────────────────────────────────────────────────────────────────────────┘
                                      ▲
                                      │ (当 Policy 为 Interactive 触发拦截时)
┌─────────────────────────────────────┴───────────────────────────────────────┐
│ 维度 2：单次操作授权记忆范围 (Action Grant Scope)                            │
│ ─────────────────────────────────────────────────────────────────────────── │
│ • Once        (仅限本次)  : 单次生效，下次执行相同操作依然需要人工确认         │
│ • Session     (本会话记住): 在当前会话生命周期内，对相同命令/动作自动放行     │
│ • Project     (本项目记住): 在当前项目及工作区版本内，对相同命令/动作自动放行 │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. 分层详细设计 (Detailed Design by Layer)

### 3.1 `mini-agent-capabilities` (Rust 能力层)

`ApprovalPolicy`、`ActionGrantScope`、`ActionGrantKey` 和结构化 resolution 已经成为
唯一模型。`ApprovalStore` 的 key 至少包含 action class、规范化动作、目标路径、
访问范围以及 workspace/revision 边界；展示用的 action summary（尤其是“几个文件”
这类数量）不能作为授权身份。未知或不完整的 key 不产生可复用 grant。

`ApprovalController::approve_request` 固定执行 `Deny → Allow → Ask`，然后按以下
顺序处理：

1. `Automatic` 只绕过明确低风险的 `Ask`；Deny、高危和未知风险仍需要显式回调或
   fail closed；
2. 以完整 `ActionGrantKey` 检查 Session、Project grant；
3. 回调返回 `outcome + grant_scope + reason`，批准后仅把非 `Once` 的完整 key 写入
   Host/Capabilities 的有界 store。

这样访问范围、全局策略和单次授权生命周期保持正交，策略切换不会被误写成 grant。

### 3.2 `mini-agent-app-server` (协议与通信层)

1. **更新 `world/set_execution` 协议参数**：
   ```json
   {
     "method": "world/set_execution",
     "params": {
       "access": "project | full_machine",
       "policy": "interactive | automatic"
     }
   }
   ```
2. **重构 `ApprovalBroker` 响应通道**：
   - `ApprovalBroker::respond` 接收结构化结果：
   ```rust
   pub struct ApprovalResolution {
       pub outcome: ApprovalOutcome,
       pub grant_scope: ActionGrantScope,
       pub reason: Option<String>,
   }
   ```
   - 消除原先仅返回布尔值导致的 `grant_scope` 丢失问题；`outcome`、`grant_scope`、
     `reason` 必须一路传到 Host/Capabilities 和事件记录。
   - canonical 协议只保留 `access + policy`、`allowed_grant_scopes`，以及响应中的
     `outcome + grant_scope + reason`。不通过旧字段别名、双写或静默 fallback 做“兼容”；
     若需要迁移，必须有明确版本边界，旧输入明确拒绝且 fail closed。

### 3.3 `mini-agent-web` (FastAPI 网关层与 SDK)

1. **SDK 对齐**：
   - `client.set_world_execution(access="full_machine", policy="automatic")`；
   - `client.respond_approval(request_id, decision="approve", grant_scope="session")`。
2. **`SessionManager` 状态维护**：
   - Web 只维护 pending request、超时和 UI 生命周期；grant 的权威状态由 Host/
     Capabilities 持有，删除 Web 自己的一套 grant cache；
   - 所有跨层请求使用 canonical `session_id`、workspace/revision 和结构化 action key，
     不以 `thread_id` 作为静默 fallback；
   - 统一管理全局 `approval_policy`，当切换到 `automatic` 时通知底座与前端，并保留高危
     动作的安全拦截。

### 3.4 Web Studio 前端交互体验重塑

1. **输入框底部执行控制栏 (Global Policy)**：
   - 访问范围下拉项：
     - `项目范围 (Project Scope)`
     - `完全访问 (Full Machine)`
   - 审批策略下拉项：
     - `交互把关 (Interactive)`：每逢新敏感指令，暂停等待人工决策；
     - `自动副驾 (Auto Copilot)`：在安全范围内全自动执行，无需人工反复确认。
   - 视觉状态指示：
     - 当选择 `完全访问 + 自动副驾` 时，点亮绿色 `Auto Copilot 运行中` 徽章，启动 `/goal` 后在低风险动作上自动推进；高危动作仍显示审批拦截。
2. **待审批操作弹窗 Dock (Action Intercepted)**：
   - 按钮排布清晰化：
     - `[仅允许本次 (Allow Once)]`
     - `[本会话记住 (Remember for Session)]`
     - `[本项目记住 (Remember for Project)]`
     - `[拒绝 (Deny)]`
   - 彻底消灭“明明批准了，为什么还弹出来”的用户困惑。

---

## 4. 评审结果与实施结论 (Review Result)

**结论：Accepted with bounded follow-up；已达到已验证门槛。**

评审提出的六项问题已按完整性和内外一致性处理，没有保留旧字段别名、双写、静默
fallback 或 Web 侧第二授权缓存。当前结论如下：

| 编号 | 实施结果 | 验证证据 |
| --- | --- | --- |
| R-01 | `world/set_execution` 只接受 `access + policy`；审批响应只接受 `grant_scope + reason` | App Server protocol、Python SDK、Gateway、Studio、TUI 均使用 canonical wire names；旧输入不在新模型中映射 |
| R-02 | `Automatic` 仅绕过明确低风险的 `Ask`；Deny、高危、未知风险仍进入显式审批或 fail closed | Capabilities admission、CLI public-path scenario、Web “automatic 不等于 Web auto approval” 回归 |
| R-03 | `ApprovalStore` 使用结构化 `ActionGrantKey`，包含 action class、规范化动作、目标路径、access、workspace、revision；key 不完整时不产生可复用 grant | `ApplyPatch` 传递真实目标路径；security store revision/owner 回归；App Server request 携带 action key |
| R-04 | 授权权威归 Host/Capabilities；Gateway/SDK/Studio 只保留 pending/UI 生命周期和显式响应 | `SessionManager` 删除 Web grant cache；world approval snapshot 只报告 `grant_store: host-capabilities` |
| R-05 | request/resolution/event 使用 `allowed_grant_scopes` 与 `outcome + grant_scope + reason`；批准范围由底座校验，拒绝不能带 scope | JSON-RPC notification round-trip、App Server public denial、SDK/Gateway/Frontend API tests |
| R-06 | 文档、SDK、Gateway、Studio/TUI 和底座已完成一轮统一；line gate 统计与增量门禁通过 | runtime `18,970/20,000`、release `29,000/30,000`，`violations: []`；跨仓测试见第 6 节 |

这次落地不需要重构 Cargo 模块关系。现有依赖方向已经足够：Protocol 只承载可复用
类型，Capabilities 持有动作风险/key 和授权存储，Host/App Server 编排准入与事件，
Web 只做外部投影。把动作 key 构造和风险判定留在 Capabilities，避免在 App Server 或
Web 再复制一份安全逻辑。

---

## 5. 变更准入检查 (PR Admission Self-Assessment)

### 六项必答题

1. **所属层**：
   核心架构与类型属于 `Capabilities` 和 `App Server`；外部契约对齐触及 `Protocol`、Python `SDK` 和 `Web Gateway`。因为审批仲裁权属于宿主环境与安全沙箱，符合 `mini-agent-core` 保持纯粹运行循环的硬边界要求。
2. **重复职责**：
   检索了 `mini-agent-capabilities/src/workspace/approval.rs`、`mini-agent-capabilities/src/security.rs` 和 `mini-agent-app-server/src/lib.rs`。当前代码中 `ApprovalMode` 与 `ApprovalScope` 职责交织并存在降维损耗。本提案不引入第三套路径，而是理顺并恢复两者原本的正交关系。
3. **旧概念**：
   删除 App Server 协议中容易混淆的单一伪 `ApprovalMode`（`per_action | current_session | current_project`），拆解并替换为正交的 `policy` 与 `grant_scope`；同时删除旧字段别名、双写和静默 fallback。若确需跨版本迁移，必须在显式版本边界执行，无法识别的旧输入直接拒绝。
4. **行数预算**：
   当前基线已经贴近硬上限，不能沿用原先过于乐观的估算。本批通过删除旧混合模型、
   重复字段和重复 fixture 抵消新增契约，最近验证记录为：
   ```text
   runtime:       18,970 / 20,000（余 1,030）
   release Rust:  29,000 / 30,000（余 1,000）
   ```
   后续 Rust 变更仍默认要求同批次删除旧概念并给出可核验的净减少。
5. **可见面变化**：
   - `world/set_execution` 只接受 `access + policy`，删除 `approval` 的混合语义；
   - `approval/respond` 必须返回结构化 `outcome + grant_scope + reason`，不以缺省值掩盖字段丢失；
   - 模型可见提示词中的 `<world_state>`、审批事件、SDK 和 UI 使用同一组 canonical 词汇。
6. **边界测试**：
   - Rust 单元/协议 fixture：验证 policy、结构化 action key、grant scope、reason 和拒绝路径；
   - Web SDK/Gateway round-trip：验证设置策略、审批响应、超时、撤销、重启和 session/project 生命周期；
   - bounded Harness Scenario/Eval：验证正常动作、高危动作、不同路径、revision 变化和错误决议的 trace；本批使用 CLI/App Server/Web 的确定性公共路径承载这些证据；
   - 现有单包测试只能作为回归证据，不能替代上述跨层证据。

---

## 6. 实施证据 (Implementation Evidence)

### `mini-codex`

- `cargo test -p mini-agent-protocol --locked`: 7 passed。
- `cargo test -p mini-agent-capabilities --locked`: 66 passed。
- `cargo test -p mini-agent-app-server-protocol --locked`: 14 passed。
- `cargo test -p mini-agent-app-server --locked`: 48 passed。
- `cargo test -p mini-agent-host --locked`: 29 passed。
- `cargo test -p mini-agent-cli --locked`: 16 unit + 11 public-path integration passed。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed。
- `python scripts/line_budget.py --base HEAD --check-delta --json`: runtime `18,970`, release
  `29,000`, `violations: []`。

### `mini-agent-web`

- `uv run pytest -q`: 66 passed。
- `uv run ruff check server sdk/python/src tui tests`: passed。
- `npm test`: 12 Node tests + 5 Vitest tests passed。
- `npm run lint` and `npm run build`: passed。

证据覆盖低风险 Automatic 放行、高危动作门禁、Deny 优先级、结构化 action key、目标
路径和 revision 边界、Once/Session/Project 响应校验、pending/timeout/revoke/restart
生命周期，以及 JSON-RPC/SDK/Gateway/Studio/TUI 的 canonical 字段。未调用真实或付费
Provider。

---

## 7. 后续计划与剩余风险 (Follow-up)

本提案的 canonical contract 已冻结；后续工作不再回到旧 `approval` 字段或在 Web
增加兼容胶水层：

1. 在独立环境补充跨平台 Sandbox/恶意内核测试，并把崩溃中断后的 audit 完整性纳入
   bounded scenario；
2. 若需要真正的跨重启 Project grant，先定义持久化安全模型、撤销语义和版本边界，
   不能把 Web `state.json` 变成第二授权存储；
3. 评估真实 Provider 的模型质量和延迟时，只记录不改变本轮权限结论的对照证据。

这些事项不阻塞本提案的已验证结论，但在完成前不得宣称跨平台隔离或崩溃后审计已经
得到完整证明。
