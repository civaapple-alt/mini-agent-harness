# Web Studio Session Doctor

状态：implemented

日期：2026-09-27

适用仓库：`mini-agent-harness`、`mini-agent-web`

## 决策

Web Studio 提供项目级的手动 Session 日志检查。Capabilities 复用现有
`load_records` parser 检查主日志；App Server 提供不启动 Agent runtime 的本地维护命令；
Gateway 按登记的 Project ID 启动该命令；Studio 展示检查结果，并要求用户确认后才修复。

检查和修复使用正交状态。每条 finding 分别报告 `inspection`、`integrity` 和 `recovery`，
例如，一个带 `recovery_gap` 的 Session 可以恢复，但其历史仍不完整。Gateway 不解析日志，
也不缓存诊断结论。

修复只接受有效 settled checkpoint 后的未完成尾记录。Capabilities 取得独占锁且重新检查日志，
先保存并同步完整原始日志，再截去未完成尾部。序号断档、`recovery_gap`、无效完整记录和缺少
checkpoint 不会自动修复。Doctor 不会判断或回收过期锁。

## 数据流与所有权

```text
Studio 项目菜单
  → Gateway 项目级 REST 路由
  → 项目主工作区下的一次性 App Server 维护命令
  → Capabilities SessionStore 与 load_records
  → 有界 JSON 报告
```

Gateway 只接受已登记的 `project_id` 和通过格式校验的 `session_id`。它从 Project registry
解析主工作区，不接受浏览器传来的路径。维护命令在 App Server 二进制的普通 runtime 初始化前
分流，因此不会加载 `RuntimeConfig`、Host、provider 或目标 Session writer。该入口不是 JSON-RPC
方法，也没有新增 SDK 方法。常规 Thread 与 Turn 操作仍沿 SDK、Gateway 和 Studio 的 App Server
连接路径运行。

Capabilities 同一 parser 继续服务普通 Session 加载和 `read_settled_checkpoint`。后者在
`runtime_actor.rs` 有生产调用，继续用于读取 Child Session fork 的 settled checkpoint；实施时
没有迁移或删除这个接口。

## 报告与边界

- 扫描最多读取 8,192 个目录项、检查 4,096 个 Session 目录，并最多返回 256 条 findings。会话日志继续受现有最大字节数限制。
- 报告包含 schema 版本、扫描和截断状态、分组计数、Session ID、稳定问题码、三类状态、适用的位置和建议。
- 报告不包含 prompt、工具参数、工具结果、日志正文或绝对路径。
- 锁文件位于 Session Store 目录中，与 Session 子目录同级，名称为 `<session_id>.lock`。扫描在读取前后检查该锁；出现锁或无法确认锁状态时不把日志报为健康。
- 修复使用不回收过期锁的独占锁入口。备份位于 `~/.mini-agent/recovery-backups/<workspace-key>/<session-id>/`，JSON 结果只返回 `recovery-backups/...` 相对路径。
- Gateway 子进程有 60 秒超时，stdout 最大 1 MiB；它只接受登记项目对应的主工作区。
- Gateway 路由是 `POST /api/threads/project/{project_id}/sessions/doctor` 和 `POST /api/threads/project/{project_id}/sessions/{session_id}/doctor/repair`。

## 验证记录

### 有界 Session 检查和尾部修复场景

- **假设：** 项目级维护入口会复用 Capabilities parser，安全报告可恢复日志的尾部损坏；修复只删除未完成尾部。
- **公共路径：** App Server `doctor --json`、`doctor repair --session-id <id> --json`；Gateway Project 路由；Studio 项目菜单。
- **设置：** 使用临时 workspace 和包含有效 settled checkpoint 的 Session JSONL，不调用 provider。
- **刺激：** 在有效日志后追加未完成 JSONL 记录，扫描项目后再明确请求修复。
- **轨迹：** 扫描报告 `incomplete_tail` 和 `repair_available=true`；修复报告返回相对备份路径；备份字节与修复前日志一致，原日志字节等于有效前缀。
- **失败情形：** 序号断档和存在 Session 锁时不提供修复，修复命令失败且原日志保持不变。
- **命令：** `python3 scripts/session_doctor_scenario.py` 使用临时 `HOME`、workspace 和三份 JSONL fixture，直接运行已构建的 App Server binary。
- **缺口：** 本地自动化不覆盖跨平台文件系统的断电持久性。Gateway 与 Studio 的测试使用维护命令和报告 stub，不会调用模型 provider。

在 `mini-agent-harness` 根目录运行 Rust 与 Scenario 检查：

```sh
cargo fmt --all
cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
cargo clippy -p mini-agent-app-server --all-targets -- -D warnings
cargo test -p mini-agent-capabilities
cargo test -p mini-agent-app-server -- --test-threads=1
cargo build -p mini-agent-app-server --bin mini-agent-app-server
python3 scripts/session_doctor_scenario.py
python3 scripts/line_budget.py --base da68450 --check-delta --json
```

在 `mini-agent-web` 根目录运行 Gateway 与 Studio 检查：

```sh
uv run ruff check server/session_doctor.py server/routes/threads.py server/control/project_registry.py server/session_manager.py tests/gateway/test_session_doctor.py
uv run pytest tests/gateway/test_session_doctor.py -q
cd frontend
npm run test:ui -- src/tests/Sidebar.test.jsx
```

Scenario 验证了真实 binary 的扫描、备份、尾部修复，以及序号断档和锁定 Session 的拒绝路径。
它在 `MINI_AGENT_SESSION_MODE` 故意无效、没有项目 provider 配置时仍通过。Capabilities 139 项通过，
App Server 串行全包 78 项通过。一次默认并行 App Server 测试中，未修改的
`goal_runtime::tests::applies_approved_verdict_after_settled_checkpoint` 失败；该用例单独重跑通过，
串行全包也通过。

预算结果为 Core + Protocol 5,450（基线 5,450，增量 0），Control Plane 33,536（基线 33,367，
增量 169），Release 48,967（基线 48,026，增量 941）。三项硬限制和单次 +1,000 行 Release
增量检查均通过。

完整 workspace 测试未运行，按仓库约定需单独批准。

## 六问准入

1. **归属：** 日志解析与修复属于 Capabilities；无 Agent runtime 维护入口属于 App Server；Project 路由属于 Gateway；用户报告与确认属于 Studio。Core 不变。
2. **现有责任：** `load_records` 负责日志解析，Project registry 负责工作区解析，App Server SessionStore 持有日志写入和锁。Gateway 不复制这些责任。
3. **替换或删除：** 复用 parser 和 executable，没有新增协议层或日志 parser。保留 `read_settled_checkpoint`，因为 `runtime_actor` 仍需要它读取 Child fork 的 settled checkpoint。
4. **实际增量：** `python3 scripts/line_budget.py --base da68450 --check-delta --json` 报告 Core + Protocol +0、Control Plane +169、Release +941；新增代码低于各自 hard limit 和单 PR +1,000 行上限。
5. **可见面：** 不改 prompt、tool schema、Core event 或 JSONL schema。新增两个 Gateway REST 路由和本地维护参数；显式修复会写入备份并截去不完整尾部。
6. **边界证据：** Capabilities parser/repair 测试、App Server 维护入口测试、Gateway Project 绑定测试、Studio 确认与重扫测试，加上上面的有界 Session 场景。SDK JSON-RPC 行为未变化。

## 后续约束

Doctor 只检查主 `session.jsonl`。索引、sidecar、附件、Notebook 和 plan 不在报告范围内。若增加这些检查，必须单独约束输入大小、隐私字段和修复责任，不能扩展当前修复动作去推测缺失历史。
