# Multi-Directory Workspace Context Perception (多目录项目上下文感知)

Status: implemented
Date: 2026-09-07
Class: feature
Scope: `Host` (`WorldState`, `harness_builder`), `App Server` (`runtime_actor`), `SDK & Web Gateway` (`session_manager`)

---

## 1. Context & Problem Statement

在 Web Studio 中，一个项目可以绑定多个目录（例如主目录 `D:\gh-ws\pi`，关联目录 `D:\gh-ws\fx`，代表一个由多子仓库构成的复合工程）。

在先前的实现中：
1. **模型对多目录完全失明**：
   - 尽管网关通过 `MINI_AGENT_EXTRA_READ_ROOTS` 与 `MINI_AGENT_EXTRA_WRITE_ROOTS` 在沙箱与文件工具层放行了关联目录的读写权限；
   - 但在注入模型上下文的 `<world_state>` 中，仅包含单个 `cwd` 属性（即主工作区）；
   - 当用户在对话中提问（例如：“简单介绍这两个项目”），大模型由于只看到一个 `cwd`，会误判并回复：“当前工作区只有一个项目（D:\gh-ws\pi）...”，无法建立跨仓库全局视野。
2. **项目工程类型检测缺失**：
   - `WorldState` 的 `project_kinds` 检测仅扫描了主工作区；若主目录是 Rust 仓库而关联目录是 Python 仓库，模型环境上下文将遗漏 Python 生态标记。

---

## 2. Decision & Implementation

### 2.1 `WorldState` 扩展多目录结构
在 [`crates/mini-agent-host/src/world.rs`](file:///d:/gh-ws/codex-ws/mini-codex/crates/mini-agent-host/src/world.rs) 中：
- `WorldState` 新增 `extra_roots: Vec<PathBuf>` 字段，并对外暴露只读引用 `pub fn extra_roots(&self) -> &[PathBuf]`。
- 新增构造入口 `WorldState::detect_with_roots(workspace: &Path, extra_roots: Vec<PathBuf>, ...)`，原有的 `detect` 保留并默认传入空 `extra_roots` 保证向后兼容。
- `with_execution` 调整保留现有的 `extra_roots`。

### 2.2 跨目录工程标记聚合 (Multi-Root Project Kinds Detection)
`detect_project_kinds` 扫描主目录及所有 `extra_roots`，合并去重后排序：
```rust
let mut project_kinds = detect_project_kinds(workspace);
project_kinds.extend(extra_roots.iter().flat_map(|r| detect_project_kinds(r)));
project_kinds.sort();
project_kinds.dedup();
```

### 2.3 提示词显式结构注入 (`<workspace_roots>` & Guidance)
在 `model_context(&self)` 中：
- 单目录项目保持轻量，不输出 `<workspace_roots>` 标签，零额外 Token 损耗。
- 当 `!self.extra_roots.is_empty()` 时，向 `<world_state>` 注入结构化目录列表：
  ```xml
  <workspace_roots>
    <root name="pi" path="D:\gh-ws\pi" primary="true" />
    <root name="fx" path="D:\gh-ws\fx" primary="false" />
  </workspace_roots>
  ```
- 在 `<execution_guidance>` 注入明确的跨目录操作指引：
  `"Multiple workspace directories configured. All roots in <workspace_roots> are part of this project; inspect and modify files across these roots using absolute paths or paths relative to cwd."`
- `status_json(&self)` 同步输出 `"workspace_roots"` 数组，供客户端状态检查或调试显示。

### 2.4 Harness 组装与动态刷新联动
- **[harness_builder.rs](file:///d:/gh-ws/codex-ws/mini-codex/crates/mini-agent-host/src/harness_builder.rs)**：
  从 `runtime_config.extra_write_roots()` 和 `runtime_config.extra_read_roots()` 提取去重后的全部外部目录，传入 `WorldState::detect_with_roots`。
- **[runtime_actor.rs](file:///d:/gh-ws/codex-ws/mini-codex/crates/mini-agent-app-server/src/runtime_actor.rs)**：
  在 `RuntimeCommand::RefreshWorld` 触发世界状态重新探测时，透传 `current.extra_roots().to_vec()`，保证动态刷新后不丢失关联目录。

### 2.5 Web Studio 网关环境对齐
- **[session_manager.py](file:///d:/gh-ws/codex-ws/mini-agent-web/server/session_manager.py)**：
  在 `_runtime_env` 中除 `MINI_AGENT_EXTRA_READ_ROOTS` 与 `MINI_AGENT_EXTRA_WRITE_ROOTS` 外，补全传递 `MINI_AGENT_PROJECT_NAME`。

---

## 3. Verification & Evidence

### 3.1 自动化测试
1. **Rust 单元测试 (`mini-agent-host`)**：
   新增 `detect_with_roots_includes_workspace_roots_and_guidance` 单元测试，验证主目录与关联目录被同时识别、两处标记（Rust + Python）被正确合并、XML 上下文包含 `<workspace_roots>` 和跨目录指导、JSON 输出包含 2 个 roots。
   - `cargo test -p mini-agent-host`: **29 passed, 0 failed**
2. **App Server 集成测试 (`mini-agent-app-server`)**：
   - `cargo test -p mini-agent-app-server`: **48 passed, 0 failed**
3. **Web Gateway 测试 (`mini-agent-web`)**：
   - `uv run pytest -q`: **66 passed, 0 failed**

### 3.2 行数预算合规性
严格遵守 `mini-codex` 规模红线（Runtime <= 20,000, Release Source <= 30,000）：
- `runtime`: **19,887 / 20,000** 行
- `release Rust source`: **29,997 / 30,000** 行

### 3.3 代码规范与代码提交
- `cargo fmt --all` 与 `cargo clippy`：0 warnings, 0 errors
- Commit Hash:
  - `mini-codex`: `f776b39 feat(host): expose multi-directory workspace roots to world state model context`
  - `mini-agent-web`: `cfaabdb feat(session): pass MINI_AGENT_PROJECT_NAME to runtime environment`

---

## 4. Consequences

1. **消除多目录认知盲区**：大模型能精准识别项目拥有的所有目录，支持自然语言跨仓库问答与跨工程代码审查/重构。
2. **最小侵入设计**：零引入新外部依赖；单目录工程完全不受额外 Prompt 影响；通过现有的 `extra_read_roots` / `extra_write_roots` 管道打通，保持核心 harness 纯粹。
