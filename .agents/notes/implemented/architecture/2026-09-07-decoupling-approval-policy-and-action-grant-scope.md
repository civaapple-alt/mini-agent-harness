# Decoupling Approval Policy and Action Grant Scope (解耦全局审批模式与单次动作授权范围)

* **日期**: 2026-09-07
* **状态**: implemented
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

1. **显式恢复两套正交类型**：
   ```rust
   /// 全局审批策略
   #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum ApprovalPolicy {
       /// 交互模式：敏感动作触发回调拦截
       Interactive,
       /// 自动副驾模式：除显式 Deny 或极端高危动作外自动放行
       Automatic,
   }

   /// 单个具体动作的授权记忆作用域
   #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub enum ActionGrantScope {
       /// 仅本次执行
       Once,
       /// 当前 Session 记忆
       Session,
       /// 当前 Project 跨 Session 记忆
       Project,
   }
   ```

2. **重构 `ApprovalController::approve_request` 执行流**：
   ```rust
   pub fn approve_request(&self, request: &ToolApprovalRequest) -> Result<(), ToolError> {
       // 1. 安全策略判定 (Deny / Allow / Ask)
       match self.policy.read().unwrap().evaluate(&request.action) {
           SecurityDecision::Deny => return Err(ToolError("forbidden by security policy".into())),
           SecurityDecision::Allow => return Ok(()),
           SecurityDecision::Ask => {}
       }

       // 2. 全局策略为 Automatic 时直接放行 (支持 Auto Copilot)
       if self.approval_policy() == ApprovalPolicy::Automatic {
           // 可选保留对破坏性未受限命令 (如 rm -rf) 的熔断保护
           return Ok(());
       }

       // 3. 检查是否有历史缓存的已授权记录 (Session / Project 级别)
       if self.is_action_cached(&request) {
           return Ok(());
       }

       // 4. 调用回调向外派发审批请求
       let resolution = self.request_decision(&request)?;
       if resolution.approved {
           // 5. 根据单次决定的 grant_scope 动态存入 ApprovalStore
           match resolution.grant_scope {
               ActionGrantScope::Once => {}
               ActionGrantScope::Session => self.store.remember_for_session(&request),
               ActionGrantScope::Project => self.store.remember_for_project(&request),
           }
           Ok(())
       } else {
           Err(ToolError(format!("user denied: {}", request.action)))
       }
   }
   ```

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
   - 消除原先仅返回布尔值导致的 `grant_scope` 丢失问题。

### 3.3 `mini-agent-web` (FastAPI 网关层与 SDK)

1. **SDK 对齐**：
   - `client.set_world_execution(access="full_machine", policy="automatic")`；
   - `client.respond_approval(request_id, decision="approve", grant_scope="session")`。
2. **`SessionManager` 状态维护**：
   - 彻底修复 `resolve_approval`：同时支持 `session` 级与 `project` 级的 grant 缓存；
   - 统一管理全局 `approval_policy`，当切换到 `automatic` 时通知底座与前端。

### 3.4 Web Studio 前端交互体验重塑

1. **输入框底部执行控制栏 (Global Policy)**：
   - 访问范围下拉项：
     - `项目范围 (Project Scope)`
     - `完全访问 (Full Machine)`
   - 审批策略下拉项：
     - `交互把关 (Interactive)`：每逢新敏感指令，暂停等待人工决策；
     - `自动副驾 (Auto Copilot)`：在安全范围内全自动执行，无需人工反复确认。
   - 视觉状态指示：
     - 当选择 `完全访问 + 自动副驾` 时，点亮绿色 `Auto Copilot 运行中` 徽章，启动 `/goal` 后畅行无阻。
2. **待审批操作弹窗 Dock (Action Intercepted)**：
   - 按钮排布清晰化：
     - `[仅允许本次 (Allow Once)]`
     - `[本会话记住 (Remember for Session)]`
     - `[本项目记住 (Remember for Project)]`
     - `[拒绝 (Deny)]`
   - 彻底消灭“明明批准了，为什么还弹出来”的用户困惑。

---

## 4. 变更准入检查 (PR Admission Self-Assessment)

### 六项必答题

1. **所属层**：
   核心架构与类型属于 `Capabilities` 和 `App Server`；外部契约对齐触及 `Protocol`、Python `SDK` 和 `Web Gateway`。因为审批仲裁权属于宿主环境与安全沙箱，符合 `mini-agent-core` 保持纯粹运行循环的硬边界要求。
2. **重复职责**：
   检索了 `mini-agent-capabilities/src/workspace/approval.rs`、`mini-agent-capabilities/src/security.rs` 和 `mini-agent-app-server/src/lib.rs`。当前代码中 `ApprovalMode` 与 `ApprovalScope` 职责交织并存在降维损耗。本提案不引入第三套路径，而是理顺并恢复两者原本的正交关系。
3. **旧概念**：
   删除 App Server 协议中容易混淆的单一伪 `ApprovalMode`（`per_action | current_session | current_project`），拆解并替换为正交的 `policy` 与 `grant_scope`。
4. **行数预算**：
   预计重构仅涉及参数管道传递与枚举展开：
   ```text
   runtime:       18,240 -> 18,310 (+70 行)
   all Rust:      26,850 -> 26,940 (+90 行)
   ```
   远低于 20,000 / 30,000 行上限。
5. **可见面变化**：
   - `world/set_execution` 参数由 `approval: ApprovalMode` 变为 `policy: ApprovalPolicy`（平滑兼容原有字符串映射）；
   - `approval/respond` 增加可选字段 `grant_scope`（缺省默认为 `once`，向前兼容）；
   - 模型可见提示词中的 `<world_state>` 将准确输出当前为交互把关还是自动副驾。
6. **边界测试**：
   - `cargo test -p mini-agent-capabilities`：验证自动模式下的放行逻辑与会话/项目缓存生命周期；
   - `cargo test -p mini-agent-app-server`：验证 JSON-RPC 端点对 `policy` 与 `grant_scope` 的装配与响应。

---

## 5. 验证与落地结果 (Verification & Evidence)

1. **底层能力与 App Server (`mini-codex`)**:
   - `mini-agent-capabilities`：解耦 `ApprovalPolicy` 与 `ActionGrantScope`，实现 `ApprovalStore` 分层管理；
   - `mini-agent-app-server`：`ApprovalBroker` 支持 `request_resolution` 返回包含 `outcome` 与具体 `approval` 作用域的结构；
   - 自动化测试：
     - `cargo test -p mini-agent-capabilities`：67 passed, 0 failed
     - `cargo test -p mini-agent-app-server`：48 passed, 0 failed
   - Commit: `d7d2477 feat(security): decouple global approval policy and action grant scope`
2. **网关与 SDK (`mini-agent-web`)**:
   - Python SDK `set_world_execution` 与 `respond_approval` 对齐 `automatic` 模式与多作用域动作决议；
   - `SessionManager` 实现独立的 `_session_approval_grants` 集合管理会话级授权生命周期；
   - `uv run pytest -q`：66 passed, 0 failed
3. **Web Studio 前端交互**:
   - 输入栏全局策略切换为 `交互把关` 与 `自动副驾 (Auto Copilot)`；
   - 待审批弹窗 Dock 明确排布 `[允许本次 (Once)]`、`[会话记住 (Session)]`、`[项目记住 (Project)]` 与 `[拒绝 (Deny)]`；
   - Commit: `95d5410 feat(security): align automatic approval mode and multi-scope action grant UI`
