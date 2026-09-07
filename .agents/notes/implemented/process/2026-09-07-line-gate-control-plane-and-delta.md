# Line Gate：Control Plane 统计与增量门禁

* **日期**: 2026-09-07
* **状态**: implemented
* **Class**: process
* **范围**: `scripts/line_budget.py`、维护脚本测试、CI、PR 准入模板、`AGENTS.md`

## 1. 结论

line gate 继续保留，但不再把“所有 Rust 行数”当作架构质量的唯一代理指标。
本轮采用四层策略：

1. 保留 Runtime `20,000` 和 Release Rust `30,000` 的最终硬上限；
2. 增加 Execution Kernel、Host Control Plane、Capabilities Control Plane、
   Provider/Adapter、CLI 的互斥统计；
3. 增加基于 merge-base 的 PR 增量门禁；
4. 将权限、Sandbox、恢复和审计的边界证据作为独立准入条件，而不是用增加行数
   或移动 Cargo crate 来“优化”统计结果。

本提案明确不要求为了 line gate 重构 Cargo 依赖关系。Cargo 边界另行依据职责泄漏、
重复类型、循环依赖或重复适配证据判断。

## 2. 动因与当前基线

Harness 的复杂度主要来自 State、Permission、Sandbox、Recovery 和 Audit 的组合，
而不是 Planner、Coder 或 MCP 的数量。当前绝对行数已经逼近上限：

| 指标 | 当前 | 硬上限 | 余量 |
| --- | ---: | ---: | ---: |
| Runtime（Core + Protocol + Host + App Server） | 19,887 | 20,000 | 113 |
| Release Rust（不含实验性 CLI/REPL） | 29,997 | 30,000 | 3 |

现有 [`scripts/line_budget.py`](../../../../scripts/line_budget.py) 能区分生产、单元和
集成测试，也能报告 Capabilities 和 CLI，但原先只有最终绝对值门禁：没有 PR delta、
运行区间，也没有把 Capabilities 中的权限和 Sandbox 边界单独显示出来。

## 3. 统计模型

统计分类必须互斥，每个 Rust 文件只能进入一个桶：

| 分类 | 内容 | 门禁含义 |
| --- | --- | --- |
| `execution-kernel` | Core + Protocol | 保持 Agent Loop 和可移植契约小而稳定 |
| `host-control-plane` | Host + App Server + App Server Protocol | 运行编排、状态、服务边界和恢复控制 |
| `capability-control-plane` | Capabilities 中的 security、sandbox、approval、path、session、result 和 workspace 边界文件 | 显示权限与副作用边界的真实成本 |
| `capability-provider` | Capabilities 中的模型、MCP、Web、扩展等具体实现 | 防止 Provider 增长掩盖控制面复杂度 |
| `cli` | CLI/实验性 REPL | 独立报告，不进入 Release Rust 硬上限 |

Capabilities 控制面文件通过显式路径清单识别，并与 Provider 桶保持不重叠。该清单
只改变统计视图，不改变 Cargo 所有权，也不把 Web/SDK/Studio 的不同语言行数强行
相加；这些边界由各自的测试和契约 delta 管理。

## 4. 运行区间与增量策略

当前硬上限不放宽，新增运行区间：

| 区间 | Runtime | Release Rust | PR 规则 |
| --- | ---: | ---: | --- |
| Green | ≤19,000 | ≤29,000 | Runtime 单 PR 增长最多 100 行，Release 最多 150 行 |
| Amber | 19,001–19,500 | 29,001–29,500 | 允许受控小额增长：Runtime ≤100 行、Release ≤150 行 |
| Red | >19,500 | >29,500 | 默认冻结增长，只允许删除或安全/恢复修复，且必须净零或净减少 |
| Hard fail | >20,000 | >30,000 | CI 失败 |

`python scripts/line_budget.py --base <merge-base> --check-delta --json` 同时检查
绝对上限、运行区间和 PR 增量。当前仓库的 Release Rust 位于 operating 边界，
仍可进行不超过单 PR 增量上限的小步维护；进入 Red 后新增 Rust 必须由同批次删除抵消。

## 5. 本轮实现

- `line_budget.py` 增加显式分类、Control Plane 汇总、运行区间状态、JSON 输出；
- 增加 `--base`，使用 Git revision 读取基线 Rust 源码并计算 Runtime/Release delta；
- 增加 `--check-delta`，在 Green/Amber 区间限制单 PR 增量，在 Red 区间冻结正增长；
- CI quality job 使用完整 Git history，在 Pull Request 上执行增量门禁；
- PR 模板要求记录 `control-plane` delta 和增量检查命令；
- `AGENTS.md` 写入运行预算、Red 区间和不改变 Cargo 所有权的统计规则；
- 维护脚本测试覆盖路径分类、Amber 区间受控增长、超额增长和 Red 区间零增长反例。

本轮没有修改 Rust 代码、Cargo manifest、模型提示词、公共协议或持久化格式。

## 6. 验证与落地结果

**结论：本提案的 line gate 统计和增量门禁已落地。**

1. 当前分类确认了 Capabilities 中约 4,801 行属于控制面边界，Control Plane
   汇总约 20,290 行；该数字用于暴露复杂度，不作为新的硬上限。
2. `line_budget.py` 支持 `--base`、`--check-delta` 和 `--json`，能够读取 Git
   基线并对 Runtime/Release 执行增量门禁；未分类 Rust 源码会 fail closed。
3. CI quality job 在 Pull Request 上执行基线增量检查；PR 模板、`AGENTS.md` 和
   提案质量指南已同步运行预算、Red 区间和 Control Plane 统计规则。
4. 本轮没有修改 Rust、Cargo 依赖、模型提示词、公共协议或持久化格式；因此没有
   为统计目的引入新的 Cargo 胶水层。

验证记录：

```text
python -m unittest scripts/test_line_budget.py scripts/test_pr_admission.py scripts/test_package_release.py
20 tests passed

python scripts/line_budget.py
runtime: 19,887/20,000
release Rust: 29,997/30,000
Control Plane: 20,290 lines

python scripts/line_budget.py --base HEAD --check-delta --json
runtime delta: +0
release delta: +0

python scripts/line_budget.py --base d7d2477 --check-delta --json
positive growth in the red band: rejected
```

## 7. 六项变更准入回答

1. **所属层**：属于维护脚本、CI 和准入流程，不进入 Core、Host 或 Capabilities
   运行路径；Control Plane 分类只是报告维度。
2. **重复职责**：复用现有 `line_budget.py`、`test_line_budget.py` 和 CI quality job，
   没有新增第二个计数器或第二条执行路径。
3. **旧概念**：保留最终硬上限作为安全底线；用增量和运行区间补足原先“只在超限时
   失败”的缺口，没有增加兼容别名或 Cargo 胶水层。
4. **预算**：Rust runtime/release 行数净增为 0；本轮只增加 Python、YAML、模板和
   文档。当前实际报告为 Runtime `19,887/20,000`、Release Rust `29,997/30,000`。
5. **可见面**：新增仅为维护脚本的 JSON 输出和 CI 日志；不增加模型输入、事件、
   持久化或公共协议。
6. **边界测试**：使用 `python -m unittest scripts/test_line_budget.py` 验证分类、
   增量和失败反例；使用 `python scripts/line_budget.py --json` 验证当前统计；使用
   `--base <merge-base> --check-delta --json` 验证 PR 门禁。

## 8. 后续计划（不阻塞本提案）

- 通过独立的简化批次删除旧适配、重复状态和重复测试，将 Release Rust 恢复到
  `≤29,000`、Runtime 恢复到 `≤19,000` 的 Green operating budget；
- 增加 Cargo 依赖方向检查，但只针对已确认的职责泄漏，不因 line gate 单独拆包；
- 为 Permission、Sandbox、Recovery、Audit 增加 bounded scenario/eval 和失败反例；
- 经过 10–20 个 PR 观察真实 delta 后，再校准 Green/Amber/Red 阈值及非 Red
  增量上限；
- 若后续发现 Cargo 所有权或跨仓契约需要独立改造，另立提案，不回写本记录的已实现
  事实。
