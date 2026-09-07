# Decoupling Approval Policy and Action Grant Scope (解耦全局审批模式与单次动作授权范围)

* **日期**: 2026-09-07
* **状态**: 提案中
* **进度说明**: 已有部分实现，但评审要求按 canonical 模型返工，尚未达到已实现门槛
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

   `ApprovalStore` 的 key 必须是结构化的、可审计的 `ActionGrantKey`，至少包含
   工具/动作类别、规范化命令、目标路径、访问范围和 workspace/revision 边界。
   展示用的 action summary（尤其是“几个文件”这类数量）不能作为授权身份；未知或
   不完整的 key 必须 fail closed。

2. **重构 `ApprovalController::approve_request` 执行流**：
   ```rust
   pub fn approve_request(&self, request: &ToolApprovalRequest) -> Result<(), ToolError> {
       // 1. 安全策略判定 (Deny / Allow / Ask)
       match self.policy.read().unwrap().evaluate(&request.action) {
           SecurityDecision::Deny => return Err(ToolError("forbidden by security policy".into())),
           SecurityDecision::Allow => return Ok(()),
           SecurityDecision::Ask => {}
       }

       // 2. Automatic 只改变低风险 Ask 的默认交互方式；高危动作仍必须受安全边界约束
       if self.approval_policy() == ApprovalPolicy::Automatic {
           if request.risk.is_high() {
               return Err(ToolError("high-risk action requires explicit approval".into()));
           }
           return Ok(());
       }

       // 3. 检查是否有历史缓存的已授权记录 (Session / Project 级别)
       if self.is_action_cached(&request) {
           return Ok(());
       }

       // 4. 调用回调向外派发审批请求；回调必须返回结构化结果，不得只返回 bool
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

## 4. 评审结果 (Review Result)

**结论：Request changes；当前不能标记为已实现。**

本次评审确认目标有价值，但现有实现仍是“旧模型 + 局部适配”的混合状态，尚未形成
内外一致的 canonical contract。主要问题如下：

| 编号 | 证据 | 风险与要求 |
| --- | --- | --- |
| R-01 | `crates/mini-agent-app-server-protocol/src/lib.rs:448` 仍暴露 `approval`；`sdk/python/src/mini_agent/client.py:1023` 仍使用旧参数 | `policy` 与 `grant_scope` 没有贯穿协议、SDK、网关；必须删除混合字段，不得靠别名、双写或静默 fallback 维持两套语义。 |
| R-02 | `crates/mini-agent-capabilities/src/workspace/approval.rs:259` 对 `Automatic` 直接返回；`crates/mini-agent-app-server/src/lib.rs:182` 仍硬编码交互模式；`mini-agent-web/server/session_manager.py:958` 可自动批准 | 高危动作可能绕过人工审批。Automatic 必须只放行明确低风险的 `Ask`，Deny 和高危动作仍要阻断或要求显式决策。 |
| R-03 | `crates/mini-agent-capabilities/src/workspace/patch.rs:61` 用文件数量拼 action；`crates/mini-agent-app-server/src/json_rpc/transport.rs:166` 的 `path_scope.paths` 为空 | 同数量但不同目标的动作可能复用同一授权。必须引入包含规范化动作、目标、访问范围和 revision 的结构化 key。 |
| R-04 | `crates/mini-agent-capabilities/src/security.rs:79` 的 store key 不含策略/访问边界；Web 另有 `_session_approval_grants` | Host/Capabilities 与 Web 出现重复权威，策略切换、撤销、重启和 session 生命周期会不一致。Web 只保留 pending/UI 状态，授权权威归 Host/Capabilities。 |
| R-05 | `crates/mini-agent-app-server/src/lib.rs:76` 的 `ApprovalResolution` 尚无 `grant_scope`/`reason`；`mini-agent-web/server/routes/agent.py:47` 和 `routes/world.py:90` 仍只接受旧 scope | 结构化决议没有完整到达事件、SDK 和 UI，用户无法看到准确原因。必须统一 `outcome + grant_scope + reason`，未知值 fail closed。 |
| R-06 | 当前验证只有能力层/App Server 单包测试和 Web 的局部测试；`docs/app-server.md:207` 仍是旧契约；当前预算为 runtime `19,887/20,000`、release Rust `29,997/30,000` | 缺少跨仓 scenario/eval，且预算余量不足以支持继续堆叠兼容层。完成前必须补齐边界证据，并以删除旧概念抵消新增代码。 |

因此，本提案退回 `proposed/architecture`，后续实现以“完整 canonical 模型先落地、旧混合模型删除、跨层契约一次对齐”为准入条件。

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
   当前基线已经贴近硬上限，不能沿用原先过于乐观的估算。最近验证记录为：
   ```text
   runtime:       19,887 / 20,000（余 113）
   release Rust:  29,997 / 30,000（余 3）
   ```
   后续 Rust 变更默认暂停，除非同批次删除旧概念并给出可核验的净减少。
5. **可见面变化**：
   - `world/set_execution` 只接受 `access + policy`，删除 `approval` 的混合语义；
   - `approval/respond` 必须返回结构化 `outcome + grant_scope + reason`，不以缺省值掩盖字段丢失；
   - 模型可见提示词中的 `<world_state>`、审批事件、SDK 和 UI 使用同一组 canonical 词汇。
6. **边界测试**：
   - Rust 单元/协议 fixture：验证 policy、结构化 action key、grant scope、reason 和未知输入拒绝；
   - Web SDK/Gateway round-trip：验证设置策略、审批响应、超时、撤销、重启和 session/project 生命周期；
   - bounded Harness Scenario/Eval：验证正常动作、高危动作、不同路径、revision 变化和错误决议的 trace；
   - 现有单包测试只能作为回归证据，不能替代上述跨层证据。

---

## 6. 现有局部验证（不代表完成） (Partial Evidence)

1. **底层能力与 App Server (`mini-codex`)**:
   - 当前提交只部分触及审批管道，实际代码仍混用 `ApprovalMode`、`ApprovalScope` 和旧 `approval` 字段，不能宣称已完成解耦；
   - 当前 `ApprovalBroker` 的结构化决议尚未稳定贯穿 `outcome`、`grant_scope` 与 `reason`；
   - 自动化测试：
     - `cargo test -p mini-agent-capabilities`：67 passed, 0 failed
     - `cargo test -p mini-agent-app-server`：48 passed, 0 failed
   - Commit: `d7d2477 feat(security): decouple global approval policy and action grant scope`
2. **网关与 SDK (`mini-agent-web`)**:
   - 当前 SDK、Gateway 和 `SessionManager` 仍存在旧参数与重复授权状态，尚未完成 canonical contract 对齐；
   - 已执行的定向回归为 `uv run pytest -q tests/gateway/test_session_manager.py tests/gateway/test_gateway_agent.py`：17 passed, 0 failed；这不等同于完整 Web 验证。
3. **Web Studio 前端交互**:
   - 前端已有策略与多作用域按钮的局部改动，但尚未有跨层证据证明其状态、决议和底座授权一致；
   - 已执行 `npm --prefix frontend run lint` 和 `npm --prefix frontend test`，通过的是前端局部回归，不覆盖上述权限边界。

上述测试证明既有路径未回归，不证明本提案的安全语义已落地；在下一节计划完成前，状态保持为“提案中”。

---

## 7. 后续实施与验证计划 (Next Steps)

### Batch 0：重新建立基线与契约矩阵

- 保持文件位于 `proposed/architecture`，同步 `.agents/notes/README.md`；
- 以当前代码为事实重新记录 Capabilities → Host/App Server → SDK/Gateway → Studio 的
  字段、状态所有者、写入者和禁止重复实现；
- 更新 `docs/app-server.md` 与相关 SDK 文档，只保留一套 canonical 词汇；
- 记录每批 before/after 行数，当前余量不足时先删除旧模型，不新增兼容层。

### Batch 1：Capabilities/Host 建立唯一授权模型

- 落地 `ApprovalPolicy`、`ActionGrantScope`、`ActionGrantKey` 和结构化 resolution；
- 删除 `ApprovalScope::Automatic`、bool-only callback、按 action summary/文件数量授权，
  以及 Web 侧重复 grant cache；
- 明确 Automatic 的低风险边界，高危、Deny、未知风险和不完整 key 均 fail closed；
- 补齐 session/project/revision、撤销和策略切换的单元测试与失败反例。

### Batch 2：App Server/Protocol 一次性对齐

- `world/set_execution` 只保留 `access + policy`；审批请求/响应统一使用
  `allowed_grant_scopes` 与 `outcome + grant_scope + reason`；
- 删除旧 `approval` 混合字段及隐式映射；旧版本迁移必须有明确版本边界和拒绝证据；
- 增加 JSON-RPC fixture，验证字段往返、未知值拒绝、reason 进入事件记录且不丢失。

### Batch 3：SDK/Gateway/Studio 贯通

- SDK 暴露与协议完全一致的设置和审批响应 API；
- Gateway 只负责 pending request、超时和 UI 生命周期，授权由底座持有；统一使用
  `session_id`、workspace/revision 和结构化 action key；
- 前端按钮只能提交允许的 `grant_scope`，并展示底座返回的 policy、outcome 和 reason；
- 增加 SDK ↔ Gateway ↔ App Server round-trip，以及策略切换、撤销、重启后的集成测试。

### Batch 4：Scenario/Eval 与完成门禁

- 增加 bounded Harness Scenario/Eval，至少覆盖：正常低风险自动放行、高危自动模式、
  Deny、不同目标路径、revision 变化、Once/Session/Project、超时、撤销、重启和未知输入；
- 每个场景记录 approval request/response、tool execution、grant cache 和 world state 的
  trace，并提供一个能证明旧实现错误的反例；
- 重新运行受影响包的 fmt、clippy、test、line budget，以及 Web 全量相关测试；
- 只有当协议、SDK、Gateway、Studio 和底座证据一致，且预算有删除抵消时，才将状态改为
  `已验证`；后续再移动到 `implemented/` 并改写为已实现事实。
