# Goal Runtime Loop Ownership

Status: implemented  
Date: 2026-09-08  
Class: bug-fix

## Decision

`ContinuationMode` 是普通 Thread 的用户偏好，不是 active Goal 的运行时授权。
当 Goal 状态为 `Running` 时，App Server runtime actor 拒绝公共
`thread/settings/update` 携带的 continuation 改写；Goal 的 milestone loop 和
step budget 继续由 Goal Runtime 在执行期间临时覆盖 Core Harness 配置。

Gateway 可以持有尚未被 App Server SessionStore canonical 持久化的显式
`continuous` 偏好，但它只能作为启动适配缓存：active Goal 启动时延后发送，Goal
settled/paused 后再恢复。普通 builtin tool 或 collaboration mode 更新不再把该
偏好错误覆盖成 `manual`。

## Why

此前 Web Studio 的 active Goal guard 只存在于 UI，直接 JSON-RPC client 仍可能在
两个 Goal milestone 之间改写 Thread Harness config。另一个问题是 Gateway 在启动
恢复时无条件发送缓存的 `continuous`，会与 Goal Runtime 的临时配置竞争；settings
路由也会把一次不携带 continuation 的工具选择结果写回 Gateway 缓存。

修复把所有权留在已有 App Server runtime actor 和 Host Goal store 中，没有新增
Core loop、Gateway authority 或 Cargo edge。

## Verification

- `cargo test -p mini-agent-app-server --lib`：50 passed。
- `cargo clippy -p mini-agent-app-server --all-targets -- -D warnings`：passed。
- `uv run ruff check server/session_manager.py server/routes/world.py tests/gateway/test_session_manager.py tests/gateway/test_gateway_goals_and_items.py`：passed。
- Gateway targeted tests：19 passed。
- `python scripts/line_budget.py --base 40807a4 --check-delta --json`：runtime `17,736`、release Rust `27,156`，delta `+62/+62`，仍为 green。
- `python scripts/cargo_boundary.py --json`：pass；既有 `App Server → Capabilities` review edge 未改变。

## Remaining risk

Thread continuation 尚未进入 App Server `SessionStore` 的 canonical persistence；在此
之前 Gateway 缓存仍是有界适配层，而不是第二个 execution authority。后续若把该字段
持久化到 Session，必须先增加 restart、跨 Project 和 revision 的一致性场景，再删除
Gateway cache。
