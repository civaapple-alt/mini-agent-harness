# `mini-agent` Python SDK

本目录是 `mini-agent` Python 包。SDK 通过 stdio JSON-RPC 连接
`mini-agent-app-server`，提供异步 client、协议类型和有界事件解析。它不拥有
Agent Loop、Session history、授权 grant 或恢复策略。

## 安装

SDK 由 `mini-agent-harness` 仓库维护，不发布到 PyPI。历史 `1.0.0` wheel 附在
[`mini-agent-web` v1.0.0 GitHub Release](https://github.com/civaapple-alt/mini-agent-web/releases/tag/v1.0.0)；
后续版本的 wheel 和 sdist 附在 Harness Release。下载对应版本的 wheel，进入保存该文件
的目录后安装。在 macOS 或 Linux 上运行：

```bash
python3 -m pip install ./mini_agent-1.0.0-py3-none-any.whl
```

Windows PowerShell 运行：

```powershell
py -3 -m pip install .\mini_agent-1.0.0-py3-none-any.whl
```

从仓库根目录构建当前 Harness 源码的 wheel 和 sdist：

```bash
uv build sdk/python
```

如果当前目录已经是 `sdk/python`，运行：

```bash
uv build .
```

构建文件写入 `sdk/python/dist/`。在 macOS 或 Linux 上从仓库根目录安装 wheel：

```bash
python3 -m pip install ./sdk/python/dist/mini_agent-1.0.0-py3-none-any.whl
```

Windows PowerShell 使用：

```powershell
py -3 -m pip install .\sdk\python\dist\mini_agent-1.0.0-py3-none-any.whl
```

如果当前目录是 `sdk/python`，macOS 或 Linux 使用：

```bash
python3 -m pip install ./dist/mini_agent-1.0.0-py3-none-any.whl
```

Windows PowerShell 使用：

```powershell
py -3 -m pip install .\dist\mini_agent-1.0.0-py3-none-any.whl
```

从仓库根目录直接安装源码，不需要先构建 wheel。在 macOS 或 Linux 上运行：

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install ./sdk/python
cargo build --release --locked -p mini-agent-app-server
export PATH="$PWD/target/release:$PATH"
```

Windows PowerShell 运行：

```powershell
py -3 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install .\sdk\python
cargo build --release --locked -p mini-agent-app-server
$env:PATH = "$PWD\target\release;$env:PATH"
```

SDK 需要 Python 3.10 或更高版本，且运行时不依赖第三方 Python 包。确保
`mini-agent-app-server` 在 `PATH` 中，或设置 `MINI_AGENT_APP_SERVER_PATH`。
SDK 会启动这个本地子进程，并通过 stdio JSON-RPC 调用它；SDK 与 App Server
需要使用兼容的发布版本。

## 最小示例

```python
import asyncio

from mini_agent import MiniAgentClient


async def main() -> None:
    async with MiniAgentClient() as client:
        await client.initialize()
        await client.start_thread()

        async for envelope in client.stream_turn("List the files in the workspace."):
            if envelope["type"] != "event":
                continue
            event = envelope["typed_event"]
            if event.event_type == "assistant_text_delta":
                print(event.delta, end="", flush=True)


asyncio.run(main())
```

## 支持的 SDK 边界

| 范围 | 主要方法 |
| --- | --- |
| 连接与协议 | `start()`、`stop()`、`restart()`、`initialize()` |
| Thread | `start_thread()`、`list_threads()`、`list_skills()`、`read_thread()`、`close_thread()`、`fork_thread()`、`resume_thread()` |
| Turn | `start_turn()`、`stream_turn()`、`read_turn()`、`wait_for_turn()`、`resume_turn()`、`reconcile_turn()`、`steer_turn()`、`interrupt_turn()` |
| 观察与协作 | `get_runtime_status()`、`replay_events()`、`list_thread_items()`、`session_control()`、`child_task_action()` |
| Session | `get_session_info()`、`fork_session()`、`read_context_manifest()`、`read_notebook()`、`search_notebook()`、`write_notebook()`、`forget_notebook()` |
| Thread 设置与 Goal | `update_thread_settings()`、`get_thread_model_settings()`、`update_thread_model_settings()`、`set_collaboration_mode()`、`get_workflow_state()`、`set_goal()`、`update_goal()`、`get_goal()`、`clear_goal()` |
| 用户交互 | `approval_handler`、`user_questions=True`、类型化 `user-question/*` 通知、`respond_user_question()` |
| Host 配置 | `manage_model_catalog()`、`test_model_connection()`、Web Search 设置与测试、World 方法、MCP 方法 |
| 本地任务 | 后台 Shell task 与 scheduled task 的 list/read/control 方法 |

`manage_model_catalog()` 读写 Host 拥有的用户级模型目录；Web Studio 和 SDK
共享这份配置。API Key 只在供应商更新请求中发送，查询结果只包含是否已配置。
`test_model_connection(provider_id, model_id)` 会发起一次有界、无工具请求，供应商
可能对其计费；不会在 SDK 启动或读取模型目录时自动调用。

`AsyncMiniAgentClient` 是 `MiniAgentClient` 的兼容别名。`ThreadItem` 是
App Server history 的读取投影，不是 SDK 的第二个持久化存储。

当前协议版本为 `2`。SDK 保留未知事件为 `GenericEvent`，使较新的 App Server 事件
不会立即破坏较旧的 SDK consumer。调用方应使用 `ThreadItem.status` 呈现生命周期，
使用 `ThreadItem.outcome` 判断结构化工具结果，不能从工具输出文本推断审批或重试。

`ThreadCheckpoint.execution_recovery` 和 `TurnReadResult.recovery` 返回
`ExecutionRecoveryInfo`，提供有类型的状态、阶段、检查点序号、进度和原因。
`recommended_action` 只给出提示，不会恢复 Turn 或重试工具。未知恢复值保留在
`raw` 中，并建议调用方检查状态。请读取 `recovery.status.value`，不要再用
`recovery["status"]` 访问恢复状态。

设置 `user_questions=True` 后，App Server 可向客户端发送 `ask_user` 请求。
通知中的 `typed_user_question` 提供当前问题和选项；客户端用
`UserQuestionAnswer` 构造并提交答案。断线后可以从 `read_thread()` 返回的
`pending_user_question` 恢复展示。完整的交互式本地个人助手示例见
[`08_personal_agent.py`](../../cookbook/python-demo/08_personal_agent.py)。

## 审批与通知

`approval_handler` 接收尚未由运行时结算的审批请求，并必须返回
`{"decision": "approve" | "deny", "grantScope": ...}`。未提供 handler 时 SDK
拒绝请求。`notification_handler` 接收 `turn/event`、item 生命周期、运行时状态和
传输错误通知。回调应尽快把工作交给调用方自己的队列，避免阻塞 SDK 的读取循环。

执行控制由 `access`（`project` 或 `full_machine`）和 `policy`（`interactive`、
`automatic` 或 `trusted`）组成。`full_machine` 只扩大候选路径范围，不等于
allow-all。Deny、Plan 锁、工具可用性和高风险确认仍由 Host/App Server 执行。
审批响应的 `grantScope`（`once`、`session` 或 `project`）是一次授权的生命周期。

每个 JSON-RPC 请求默认在 30 秒后超时。你可以通过
`MiniAgentClient(request_timeout=...)` 为本地环境调整该值。超时会清理 pending
request 并抛出 `AppServerRequestTimeoutError`（它继承 `ServerProcessError`，并提供
`method` 与 `timeout` 字段）。`wait_for_turn()` 会在自己的超时窗口内重试单次
`turn/read` 请求超时；窗口到期时抛出 `TurnTimeoutError`。`AppServerError` 保留 JSON-RPC 的 `code`、
`message` 和 `data`。`session/fork` 的身份或 context-policy 冲突使用
`SESSION_FORK_CONFLICT_CODE`（`-32001`）。

`search_notebook()` 在 SDK 中对 App Server 返回的有界 Notebook 投影执行本地过滤；
它直接过滤已有投影，不会发送独立的搜索 RPC。

## 深入阅读

[`python-sdk-guide.md`](python-sdk-guide.md) 说明 client 生命周期、事件处理、
审批、恢复和协议验证方式。

## 开发检查

从仓库根目录运行以下命令：

```bash
uv sync --project sdk/python --group dev
uv run --project sdk/python ruff check sdk/python/src sdk/python/tests cookbook/python-demo
uv run --project sdk/python ruff format --check sdk/python/src sdk/python/tests cookbook/python-demo
uv run --project sdk/python pytest sdk/python/tests -q
```

SDK 代码、文档、测试和通用 App Server 示例由 Harness 维护。Web Studio
单独维护 Gateway 和界面集成；同级目录布局下，Web 仓库通过
`../mini-agent-harness/sdk/python` 的 editable path source 使用这里的 SDK。仓库布局和启动步骤见
[Web Studio 开发说明](https://github.com/civaapple-alt/mini-agent-web/blob/main/README.md#同时开发-harness-sdk)。

## License

MIT License.
