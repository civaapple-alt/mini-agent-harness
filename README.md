# Mini Agent Harness

Mini Agent Harness 是一个面向交付的 Agent 运行系统。它以“薄 Agent Loop、厚
Control Plane”为核心：Core 负责模型和工具契约、有界执行循环、限制、停止分类
和观察事件；Host、Capabilities、App Server 负责准入、审批、沙箱、副作用、
Thread/Turn/Session、恢复、并发和持久化。

`mini-agent` 是命令行入口，Web Studio 是面向长时间运行任务的控制和观察工作台。
两者通过同一条 App Server 运行链路访问同一套 Thread、Turn、Session 和运行状态。
CLI 适合本地运行、脚本和底层边界验证；Web Studio 适合项目管理、审批、状态观察、
停止与继续、Session 恢复、Child Session 和 Notebook。

项目的目标是把一次 Agent 运行做成可启动、可观察、可控制、可暂停、可恢复、可审计
和可验证的系统。它不以复制 Codex、Pi、fx 或 Qi 的全部功能为目标，也不以功能数量
作为成熟度标准。

```text
agent runtime = thin Agent Loop + thick Control Plane
```

模型提出回答或动作，Agent Loop 负责有界执行，Control Plane 负责把执行放进可
审批、可观察、可恢复和可持久化的运行生命周期。

## 当前推进方向

项目沿两条相互依赖的路线推进：

1. **薄 Agent Loop、厚 Control Plane**：保持 Core 的责任窄而稳定，把执行准入、
   审批、沙箱、Session、恢复、并发和对外状态投影放在 Core 之外的控制平面。
2. **时间上延展、结构上并发**：通过独立 Session、Child Session、operation、
   notebook 和 replay，让任务能够跨越更长时间运行，并在明确边界内并发。

第二条路线为第一条路线提供长时间运行所需的状态、恢复和并发结构。相关架构决策
见[薄 Agent Loop、厚 Control Plane](.agents/notes/implemented/architecture/2026-09-08-thin-loop-thick-control-plane-system-engineering.zh.md)
和[时间上延展、结构上并发](.agents/notes/implemented/architecture/2026-09-17-time-extended-child-session-and-notebook.zh.md)。

## 安装与第一次运行

### 使用发布包

从 [GitHub Releases](https://github.com/civaapple-alt/mini-agent-harness/releases)
下载对应平台的归档，校验 `.sha256` 后将 `mini-agent` 放入 `PATH`。支持
Linux x86_64、macOS x86_64、macOS arm64 和 Windows x86_64。

### 从源码构建

需要 Rust 1.88 或更新版本：

```sh
cargo build --release --locked -p mini-agent-cli
./target/release/mini-agent --version
```

Windows 使用 `target\release\mini-agent.exe`。Shell 工具在 Unix 使用 `sh`，
Windows 需要 PowerShell 7 (`pwsh`)。

### 运行

Provider-backed 命令需要配置 `OPENAI_API_KEY`、`OPENAI_MODEL`，以及可选的
`OPENAI_BASE_URL`。凭证优先级、Goal verifier 和运行时组合见
[`docs/configuration.md`](docs/configuration.md)。

```sh
mini-agent --version
mini-agent
mini-agent run "summarize this repo"
```

CLI 的完整命令、参数、Session resume/fork 和 trace 用法见
[`docs/cli.md`](docs/cli.md)。

## 架构总览

```text
mini-agent-core + Protocol
          ↓
Host (workflow and runtime composition)
          ↓
App Server (Thread, Turn, Goal, control, events)
          ↓
Python SDK → FastAPI Gateway → Web Studio

Experimental edges: Rust REPL / Python TUI → App Server
```

| 层 | 主要责任 |
| --- | --- |
| Protocol | Model、Tool、Message、Event、Stop 和 Limit 契约 |
| Core | 有界上下文、Model/Tool step、Compaction、控制和历史写回 |
| Capabilities | Provider、Workspace、Process、Sandbox、MCP、Skill/Plugin |
| Host | Prompt/Rule、ToolOrchestrator 和 Runtime 组合 |
| App Server | Thread/Turn/Goal、Actor/CAS、事件、审批和 JSON-RPC |
| CLI | 终端输入、输出、批准交互和实验性本地客户端入口 |

主线是 `mini-agent-core → Host → App Server → Python SDK → FastAPI Gateway →
Web Studio`。`mini-agent` CLI 和 Web Studio 都消费 App Server 的 Thread/Turn/Item
契约，不另建执行循环。Web Studio 是长时间运行任务的默认控制和观察界面；Rust REPL
和 Python TUI 仍是用于验证 App Server 边界的实验性客户端。

默认 model-visible Builtin 工具只有 `read_file`、`apply_patch`、`shell` 和
`read_image`；MCP 与 `web_fetch` 是显式扩展。文件修改统一由 `apply_patch`
承担。

## 文档地图

| 主题 | 文档 |
| --- | --- |
| CLI 安装、命令和参数 | [`docs/cli.md`](docs/cli.md) |
| Provider、Session、Skill、Plugin、MCP | [`docs/configuration.md`](docs/configuration.md) |
| App Server JSON-RPC、Thread、Turn、Goal、Item | [`docs/app-server.md`](docs/app-server.md) |
| Web Studio 集成、Project、Session 和批准 | [`docs/studio-integration.md`](docs/studio-integration.md) |
| Harness 分层与 Codex 对照 | [`docs/harness-framework.md`](docs/harness-framework.md) |
| 责任边界与变更准入 | [`docs/harness-boundaries.md`](docs/harness-boundaries.md) |
| Builtin 工具契约 | [`docs/harness-tool-surface.md`](docs/harness-tool-surface.md) |
| 限制、超时与上下文预算 | [`docs/limits.md`](docs/limits.md) |
| World state、Session 和 Goal verifier | [`docs/world-state.md`](docs/world-state.md) |
| Scenario 与 Evidence | [`docs/harness-evidence.md`](docs/harness-evidence.md) |
| 故障、隐私和发布 | [`docs/troubleshooting.md`](docs/troubleshooting.md)、[`docs/privacy.md`](docs/privacy.md)、[`docs/releasing.md`](docs/releasing.md) |
| 扩展示例 | [`examples/extensions/README.md`](examples/extensions/README.md) |

[`docs/README.md`](docs/README.md) 是 `docs/` 的本地索引；架构决策和实验记录
见 [`.agents/notes/README.md`](.agents/notes/README.md)。它们不替代当前主题文档。

## 开发验证

针对 Rust 改动运行受影响包的格式、Clippy、测试和行数预算：

```sh
cargo fmt --all
cargo clippy -p <affected-package> --all-targets -- -D warnings
cargo test -p <affected-package>
python3 scripts/line_budget.py
```

跨包发布验证和完整 workspace 测试见 [`docs/releasing.md`](docs/releasing.md)。
每次改动还需要完成 [`AGENTS.md`](AGENTS.md) 规定的六项准入问题；不要为了
行数目标删除 Core、Actor、CAS 或 Session 权威边界。

## 项目身份

项目地址：[GitHub](https://github.com/civaapple-alt/mini-agent-harness)。
Licensed under the [MIT License](LICENSE)。版本历史见 [`CHANGELOG.md`](CHANGELOG.md)。
