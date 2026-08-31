## 变更准入检查

> 新增功能、重构、测试调整和协议变更都必须填写。纯文档变更可填写
> `N/A`，但仍需说明不涉及 Rust 行数和运行时边界。

### 六项必答题

1. **所属层**：它属于 `Core`、`Host`、`Capabilities`、`App Server` 还是 `CLI`？为什么？

   <!-- answer here -->

2. **重复职责**：是否已经存在同一职责的路径或类型？检索过哪些文件或符号？

   <!-- answer here -->

3. **旧概念**：能否删除或替换旧概念来实现，而不是继续叠加？如果不能，原因是什么？

   <!-- answer here -->

4. **行数预算**：预计 runtime 和全 Rust 各净增/净减多少行？实际结果是多少？

   ```text
   runtime:       before -> after (delta)
   all Rust:      before -> after (delta)
   ```

5. **可见面变化**：是否增加模型可见输入、事件、持久化内容或公共协议面？若是，列出上限、兼容性和证据。

   <!-- answer here -->

6. **边界测试**：能否由现有公共边界测试覆盖？列出运行的测试、Clippy、fmt 和预算命令；若不能，说明新增证据。

   ```text
   cargo test ...
   cargo clippy ...
   cargo fmt --all --check
   python scripts/line_budget.py
   ```

### 准入确认

- [ ] 我已确认 runtime 不超过 `20,000` 行，全 Rust 不超过 `30,000` 行。
- [ ] 新增代码默认满足净零增长，或已列出明确抵扣项/预算取舍。
- [ ] 我没有为了行数删除 Core 核心测试、Actor/CAS/Session 单一权威或公共协议行为。
- [ ] 若触及模型上下文、事件、持久化或协议，我已补充对应架构说明和集成证据。
- [ ] 若这是纯文档变更，我已明确写出 `N/A` 的范围。

### 关联文档

- [Agent Notes README](../.agents/notes/README.md)
- [Framework and Harness Maturity](../.agents/notes/implemented/architecture/2026-08-31-agent-framework-and-harness-maturity.md)
