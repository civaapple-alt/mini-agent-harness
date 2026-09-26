# 子任务终态与最终结果恢复

**状态：** implemented，2026-09-26

## 问题

WebStudio 根据 Child Session 中匹配的 `turn_settled` 显示完成；`task_read` 和
`task_list` 只投影最新的 child operation。若 Turn 已经持久化而终态 operation
写入失败，模型侧会重复看到旧的 `running`。最终回答含换行、制表符或回车时，旧校验
把它按单行控制文本拒绝；超过 16 KiB 的回答也会被拒绝。worker 只记录 warning，
所以旧 operation 快照可能永久停留在非终态。`reports` 是显式 `task_report` 进展，
不包含最终 assistant 回复。

## 决策

- `SessionOperation::bounded_result` 将 operation 最终结果限制在 16 KiB，保留普通
  换行、制表符和回车，将其他控制字符替换为空格；operation 校验按多行结果验证。
- worker 写入结果失败时，再尝试只写 operation 终态。无法写入终态时，错误仍记录在
  App Server warning 日志中。
- `task_read` 和 `task_list` 使用 Child Session 中与 operation `turn_id` 匹配的
  `turn_settled` 恢复终态。旧 queued 记录只有在提示词和时间戳都匹配时才应用恢复。
  `task_read` 在 operation 未保存结果时，从该 Turn 最后一个 assistant item 恢复前
  512 个字符；`reports` 仍只返回显式进展更新。
- App Server shutdown 在丢弃 worker 的 `RuntimeActorState`、释放 SessionStore 锁后
  才确认关闭，避免立即恢复 Session 与锁释放竞争。
- Session JSONL 保持唯一持久事实来源；不增加 Gateway 或 Web 的状态账本。

## 验证

- `cargo test -p mini-agent-capabilities`：127 passed。
- `cargo test -p mini-agent-app-server -- --test-threads=1`：72 passed。
- `cargo clippy --workspace --all-targets -- -D warnings`：passed。
- WebStudio settled-operation 投影测试：5 passed。
- `python3 scripts/line_budget.py`：passed；Core + Protocol 4,767/6,000，Control Plane 30,904/35,000，Release 44,381/50,000。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：passed；本轮 Release +319、Control Plane +212。
- `cargo fmt --all` 与 `git diff --check`：passed。

分支相对 `origin/main` 的累计 Release 增量为 3,489 行（包含本轮开始前已有的 8 个本地提交），高于单次变更 1,000 行增量门槛；本轮自身增量为 319 行。Web 仓库配置的文档链接脚本在本机给定路径不存在，本轮文档改动已通过 `git diff --check`。

App Server 的无付费模型场景通过真实 Core tool loop 调用 `task_list` 和 `task_read`，
验证旧 `running` operation、超过 16 KiB 的多行 assistant 回复、settled Turn、空
reports 列表及恢复后的 bounded result。既有 follow-up restart RPC 测试同时验证关闭后
可以立即重新打开 Child Session。
