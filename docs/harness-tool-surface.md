# Builtin Tool Surface Upgrade

Status: current tool surface

## Decision

将默认 Builtin model-visible tool surface 收敛为四个稳定能力：

```text
read_file | apply_patch | shell | read_image
```

文件修改统一由 `apply_patch` 承担；`write_file`、`edit_file` 不再保留实现、
catalog entry 或兼容路径。`web_fetch` 是独立的显式扩展，不属于默认工具集。
这样默认 harness 更接近 Pi 的极简内核：少量稳定内置工具，扩展能力由
Host/extension 显式加入。

## Boundary

- Core 继续拥有 turn loop、observation events、limits 和 history writeback；
- Protocol 的 `ToolHandler` 负责参数解析和 admission 描述；
- Host 的 `ToolOrchestrator` 负责 admission、approval 和 execution ordering；
- `ToolRuntime` 继续拥有具体副作用与 sandbox；
- App Server 只选择 allowlisted tool profile，并通过 Thread settings 返回当前
  selection；不再维护 workflow 聚合状态快照；
- Web Gateway、SDK 和 SidePanel 保持相同的 tool selection 语义，显式空选择不
  被默认值覆盖。

## Read / patch contract

`read_file` 改为有界分页读取：支持 `offset` 和 `limit`，默认 200 行、最大 2000
行，单页输出不超过约 15 KiB，结果带行号并返回 `next_offset`。文件 source
读取上限为 8 MiB；长文件必须继续分页，不能依赖截断输出。

新增 `apply_patch` 使用 Codex 风格的 `*** Begin Patch` 文本协议，支持 Add、
Update、Move、Delete。一次 patch 最多 512 KiB、16 个操作、32K hunk 行；先对
所有路径和 hunk 做完整校验，再执行副作用。沿用 Workspace path policy、approval
和 Plan Mode；后续写入失败时尽力回滚已完成的文件写入。

## Six-question admission record

```text
1. Layer: Capabilities/Host/App Server + Web mirrors; Core contract unchanged.
2. Duplicate responsibility: reuse existing Workspace policy, ToolRouter,
   ToolOrchestrator and Thread settings; no second file-edit loop or workflow
   aggregate.
3. Replace vs add: replace the old file mutation pair with one bounded patch
   protocol; keep the four-tool default and do not retain compatibility entries.
4. Net line delta: runtime/release budget measured after implementation by
   scripts/line_budget.py; removed file tools do not have a second or compatibility
   path.
5. Visible surface: default tool manifest, read_file pagination and apply_patch
   schema changed; all payloads remain bounded; Thread settings expose the
   allowlisted selection, not arbitrary prompt replacement.
6. Boundary evidence: affected Rust package tests, CLI public scenarios, Web
   gateway/SDK tests, frontend tests/build, fmt, Clippy, line budget and diff check.
```

## Verification evidence

- Affected Rust package tests pass; strict affected-package Clippy passes with the
  repository's existing `session.rs::initialize_new` argument-count allowance.
- `cargo fmt --all --check` and `python scripts/line_budget.py` pass.
- Web Gateway/SDK pytest passes, Ruff checks pass, frontend tests and production build
  pass using the freshly built local App Server binary.
- No paid provider call was used.

## Maintenance

New tools should first prove that one of the four defaults cannot express the
required workflow, then add bounded extension/profile capability rather than
growing the default catalog. Implementation changes are recorded in dated notes,
while this file is updated when the current tool contract changes.
