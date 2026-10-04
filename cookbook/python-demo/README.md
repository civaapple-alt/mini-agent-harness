# Python Cookbook

本目录包含可直接运行的 `mini-agent` 示例。每个脚本都是一个独立入口；示例
之间不共享运行时状态。

## 示例清单

| 脚本 | 内容 | 类型 |
| --- | --- | --- |
| `01_basic_turn.py` | 初始化、Thread、Turn 和 typed tool outcome | live |
| `02_streaming_events.py` | 文本、思考、工具、outcome 和 usage 流 | live |
| `03_approval_handling.py` | Shell 与 `apply_patch` 审批回调及 outcome | live |
| `04_steering_and_interrupt.py` | Steer 和协作中断 | live |
| `05_workflows_and_inspection.py` | World、独立执行策略、Plan、Goal 和 Checkpoint | live |
| `06_protocol_compatibility.py` | 事件、ThreadItem 与已知/未知 outcome 检查 | offline |
| `07_recovery_and_projection.py` | EOF 恢复、Session fork 和身份投影 | offline |
| `08_personal_agent.py` | 命名 Session、持续对话、用户提问与中断恢复边界 | live |

## 运行

以下命令都从仓库根目录运行。需要 Python 3.10 或更高版本和 `uv`。

先运行离线协议示例，确认 SDK 能解析已知和未知事件及工具结果。它不启动 App Server，
也不调用模型：

```bash
uv run --project sdk/python python cookbook/python-demo/06_protocol_compatibility.py
```

Live 示例需要 `mini-agent-app-server` 可执行文件，以及已配置的模型 Provider 和默认模型。
先构建 App Server：

```bash
cargo build --release --locked -p mini-agent-app-server
```

首次运行前，在 Web Studio 的**设置 → Agent 能力 → 模型设置**中配置供应商凭证和默认模型。
配置说明见 [`docs/configuration.md`](../../docs/configuration.md)。SDK 会从 `PATH` 查找
App Server。如果它不在 `PATH`，请从仓库根目录设置 `MINI_AGENT_APP_SERVER_PATH`。
然后运行第一个实时示例：

```bash
# macOS / Linux
export MINI_AGENT_APP_SERVER_PATH="$PWD/target/release/mini-agent-app-server"
```

Windows PowerShell 使用：

```powershell
$env:MINI_AGENT_APP_SERVER_PATH = (Resolve-Path 'target\release\mini-agent-app-server.exe').Path
```

配置好路径后，运行示例：

```bash
uv run --project sdk/python python cookbook/python-demo/01_basic_turn.py
```

其他 Live 示例：

```bash
uv run --project sdk/python python cookbook/python-demo/02_streaming_events.py
uv run --project sdk/python python cookbook/python-demo/03_approval_handling.py
uv run --project sdk/python python cookbook/python-demo/04_steering_and_interrupt.py
uv run --project sdk/python python cookbook/python-demo/05_workflows_and_inspection.py
uv run --project sdk/python python cookbook/python-demo/08_personal_agent.py
```

`08_personal_agent.py` 在 App Server 中使用名为 `personal-agent` 的 Session，
退出后再次运行会附着该 Session。它通过 SDK 类型化处理 `ask_user` 通知，并在
重启后重新展示尚未回答的问题。输入 `/quit` 退出。示例需要已配置的模型 Provider；
如需为自己保留一份独立对话，可在运行前设置专用 ID：

```bash
MINI_AGENT_SESSION_ID=my-personal-agent uv run --project sdk/python python cookbook/python-demo/08_personal_agent.py
```

离线协议检查不启动 App Server，也不调用模型：

```bash
uv run --project sdk/python python cookbook/python-demo/06_protocol_compatibility.py
uv run --project sdk/python python cookbook/python-demo/07_recovery_and_projection.py
```

## 编写约定

- 示例应保持短小，直接展示一个 SDK 边界；
- Live 示例不能成为默认自动化测试的隐式依赖；
- 新增公共事件或 ThreadItem 形状时，同步扩展离线示例；
- 使用 `status` 表示生命周期，使用 `outcome` 表示工具结果；不要从
  `is_error`、输出文本或缺失事件推断 retry 或 approval；
- 所有 `.py` 文件必须保持可编译。
