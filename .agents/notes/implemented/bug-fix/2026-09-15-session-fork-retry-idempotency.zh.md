# Session fork 重试幂等与 child Thread 冲突

状态：implemented
日期：2026-09-15
批次：Iteration 2，Session fork 的重复请求边界
范围：`mini-codex` Capabilities/App Server 与 `mini-agent-web` Gateway/SDK 消费路径

## Decision

同一父 Session、同一已结算 checkpoint、同一 `newThreadId` 的重试请求返回已存在
的 child Session，不再创建新的持久化目录。`newThreadId` 是调用方提供的有界幂等
身份，随机生成的 child `session_id` 不是重试键。

如果同一个 child Thread ID 已绑定其他父 Session 或其他 checkpoint，则拒绝请求，
不覆盖 `thread_index.json`，也不创建第二个 child writer。

Gateway 端在已有 child client 绑定且仍可用时直接返回已保存 metadata，不重复启动
child App Server。若第一次请求已在 App Server 写入 child、但 Gateway 在绑定前失败，
下一次请求可以重新 attach 同一 Session。

## Harness hypothesis

如果 SessionStore 在创建前用 workspace 内的 `session-fork` 锁保护 child 查找和分配，
并使用 `thread_index` 与 child Session header 校验 lineage，那么网络重试、child client
启动失败或 Gateway 进程在返回前崩溃，都不会把一次用户操作放大成多个独立 Session。

反例包括：重复请求仍生成两个 Session 目录、索引被静默覆盖、父文件被修改、Gateway
启动第二个相同 Session writer，或同一个 child ID 被不同 checkpoint 错误复用。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| child Session 分配和 lineage | Capabilities `SessionStore` | 在持久化边界做查找、冲突判断和目录分配 | Gateway 自己扫描或复制 `session.jsonl` |
| RPC 操作顺序 | App Server worker/action admission | 继续按 worker 顺序提交 fork | 用 Gateway 本地状态绕过 App Server busy 检查 |
| child client 绑定 | Web Gateway `ClientPool` | 复用已绑定可用 client | 第二次请求无条件创建 child client |
| 权限和审批 | Host/Capabilities | child 重新按 Project 准入 | 把 fork 成功当作权限 grant |

## Cross-repository contract

| 语义 | mini-codex | mini-agent-web |
| --- | --- | --- |
| 重试身份 | `parent_session_id + parent_checkpoint_seq + new_thread_id` | REST `source_thread_id + new_thread_id` 保持不变即可重试 |
| 相同请求 | `SessionStore::fork_from_checkpoint` 返回已有 `SessionForkInfo` | 已绑定 child 时返回保存的 session/path/lineage metadata |
| child 冲突 | 发现其他 lineage 时返回错误，不覆盖 index | 映射为 HTTP 409，调用方需要换 `new_thread_id` |
| 部分失败 | child Session 可被下一次请求重新发现 | child client 启动失败后允许再次 attach/fork 请求 |
| Session 权威 | SessionStore 文件、lock、thread index | SDK 只解析 `SessionForkResult`，Gateway 不写 Session 文件 |

## Verification

新增 Capabilities 测试：

- `retrying_the_same_fork_returns_the_existing_session`：重复请求返回同一 Session，
  不产生第二个 child 目录。
- `fork_rejects_reusing_a_child_thread_for_another_checkpoint`：checkpoint 改变后
  拒绝复用 child Thread ID。

Gateway 回归测试让 `/api/threads/fork` 连续提交两次，断言：

- 两次返回同一个 `session_id`；
- `MiniAgentClient.fork_session` 只被调用一次；
- child client 创建函数只被调用一次。

验证命令与结果：

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo fmt --all` | passed |
| mini-codex | `cargo test -p mini-agent-capabilities --lib` | 77 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 56 passed |
| mini-codex | `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings` | passed |
| mini-codex | `python scripts/line_budget.py --base b76687c --check-delta --json` | runtime `+0`，release `+193`，无 violation |
| mini-agent-web | `uv run pytest -q tests/gateway/test_gateway_goals_and_items.py tests/gateway/test_session_manager.py` | 56 passed，1 个既有 asyncio 子进程析构 warning |
| mini-agent-web | `uv run ruff check server/control/client_pool.py tests/gateway/test_gateway_goals_and_items.py` | passed |

## Budget

相对第一批 mini-codex 提交 `b76687c`，本批 runtime 为 `20,217 -> 20,217`，
release Rust 为 `30,160 -> 30,353`，control plane 为 `20,995 -> 21,188`。增量
仍低于当前 runtime `+200`、release `+300` 的非红区门禁。

## Remaining risks

- 当前 Session header 只持久化父 Session 和 checkpoint lineage，尚未持久化
  `contextPolicy` 及压缩结果。因此重试必须保持同一请求语义；不同 policy 复用同一
  child ID 的冲突行为暂不作为兼容契约，下一批决定是否需要结构化记录。
- 旧版本已经创建、但没有本批 metadata 的 fork Session 只能通过 lineage 识别，不能
  完整恢复原始压缩 method；不能把推断出的 `exact` 当成历史事实。
- Gateway 仍有一个既有 Windows asyncio 子进程析构 warning，需要在 runtime EOF/
  reconnect 批次中清理。
- 本批没有运行真实 provider、完整 workspace 测试或前端构建。
