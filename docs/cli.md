# CLI 使用指南

本文件只描述 `mini-agent` 命令行的安装、运行参数和本地 Session 操作。协议
与运行时语义见本目录的 App Server 文档。

## 安装

### 发布包

从 [GitHub Releases](https://github.com/civaapple-alt/mini-agent-harness/releases)
下载对应平台的归档和 `.sha256` 文件，校验后解压并将二进制放入 `PATH`。

支持 Linux x86_64、macOS x86_64、macOS arm64 和 Windows x86_64。归档包含
二进制、`README.md`、`LICENSE` 和 `CHANGELOG.md`。

### 源码构建

需要 Rust 1.88 或更新版本：

```sh
cargo build --release --locked -p mini-agent-cli
./target/release/mini-agent --version
```

Windows 使用 `target\release\mini-agent.exe`。Unix Shell 使用 `sh`；Windows
Shell 需要 PowerShell 7 (`pwsh`)。

## Provider 配置

Provider-backed 命令需要凭证和模型配置。推荐将它们放在用户级
`~/.mini-agent/.env`（Windows 为 `%USERPROFILE%\.mini-agent\.env`），避免
凭证进入项目目录：

```dotenv
OPENAI_API_KEY=
OPENAI_MODEL=deepseek-v4-flash
OPENAI_BASE_URL=https://api.deepseek.com
```

配置优先级为：进程环境、启动工作区 `.env`、用户级 `.env`、内置默认值。
完整变量和运行时组合说明见 [`configuration.md`](configuration.md)。

## 常用命令

```sh
mini-agent                         # 交互式会话
mini-agent run "summarize this repo"
mini-agent run --json "review the current changes"
mini-agent resume SESSION_ID
mini-agent fork SESSION_ID
```

使用 `mini-agent help` 或 `mini-agent help <command>` 查看命令帮助。Prompt
以 `-` 开头时，在 Prompt 前加 `--`。

`run PROMPT` 执行一次有界 Turn；省略 Prompt 时从 stdin 读取。`resume` 恢复
settled Session，`fork` 创建独立分支。运行中的进程、队列输入
和其他 live effect 不会被恢复。

### REPL 的实验性定位

`mini-agent repl`、`resume` 和 `fork` 的交互入口是实验性本地 App Server
参考客户端，用于验证 Core/Host/App Server 的终端边界。它使用交互式逐次批准，
只保留 `/steer` 和 `/exit` 等最小终端控制，不提供独立的 Profile、Plan 或 Goal
控制面。用户主流程是 `mini-agent-core → Host → App Server → Python SDK →
FastAPI Gateway → Web Studio`；Plan、Goal、项目会话和 Web 控制应通过 App Server
客户端完成。

## Turn 参数

| 参数 | 适用命令 | 作用 |
| --- | --- | --- |
| `--session-id ID` / `--session ID` | REPL、`run` | 使用已有 durable Session |
| `--auto-approve` / `-y` / `--yes` | `run` | 显式放行敏感工具 |
| `--no-tools` | REPL、`run` | 禁用 Builtin 和扩展工具 |
| `--security-preset PRESET` | REPL、`run` | `default` 或 `full-machine` |
| `--sandbox KIND` | REPL、`run` | `native` 或 `docker` |
| `--web-search` / `--search` | REPL、`run` | 启用内置 Responses `web_search` |
| `--no-web-search` / `--no-search` | REPL、`run` | 禁用内置 Responses `web_search` |
| `--json` | `run` | 输出机器可读结果 |
| `--trace-jsonl PATH` | `run` | 写入一次性有界脱敏事件记录 |

## Session 与 trace

REPL 和 `run` Session 默认持久化到
`~/.mini-agent/sessions/<workspace>/<session-id>/`。Session 记录包含 settled
turn、context item 和结果句柄；恢复只使用最新完整 checkpoint。Session 是
对话持久化，不是未结算外部 effect 的恢复机制。

`run --trace-jsonl PATH` 必须显式指定，且目标文件不能已经存在。父目录必须
已存在；每条记录最多 8 KiB，完整 JSONL 最多 256 KiB。trace 只包含事件元数据、
计数和 hash，不复制 Prompt、工具参数、工具结果或 Session 历史；写入或最终化
失败会使命令失败。该路径不会隐式生成。

## 安全边界

- 默认 Builtin 工具是 `read_file`、`apply_patch`、`shell` 和 `read_image`；
  MCP 与 `web_fetch` 不会被默认工具面隐式启用；
- 非交互 `run` 对敏感工具默认拒绝，必须显式使用 `--auto-approve`；
- 文件访问、模型输入输出、工具调用和 Shell 结果都有硬限制，详见
  [`limits.md`](limits.md)；
- Docker 模式提供受控容器执行，但不自动等同于完整网络、Capability 或资源
  隔离，详见 [`harness-boundaries.md`](harness-boundaries.md)；
- CLI 不提供独立的 Plan/Goal 工作流控制面；这些操作属于 App Server 的
  Thread/Goal 客户端边界。
