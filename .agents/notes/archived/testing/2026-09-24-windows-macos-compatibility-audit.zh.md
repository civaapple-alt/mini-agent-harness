# Windows 与 macOS 兼容性审查

状态：archived
归档日期：2026-09-24

本记录保留 2026-09-24 对 `mini-agent-harness` 和 `mini-agent-web` 的兼容性审查结果。它是一次性证据快照，不替代两个仓库的当前规格或运行手册。

## 范围与方法

目标平台为 Windows x64、macOS Apple Silicon 和 Intel。Linux 只用于核对 CI 对照。Web Studio 以 Chrome 为目标浏览器。

审查跟踪 Harness 的安装、Shell、子进程、Session 路径和 App Server 测试，跟踪 Web 的 Python SDK 子进程、Gateway 启停、目录选择、项目创建和 Studio 请求。审查了现有源码、测试、README、CI 工作流和最近的 GitHub Actions 结果。

本地仓库审查时的 HEAD：

| 仓库 | HEAD |
| --- | --- |
| `mini-agent-harness` | `d6ff7d7d91b6bf6146a58f0bd53691abcc54fb25` |
| `mini-agent-web` | `b7a4390e925db00874f8d0eaf28ccf1794b03999` |

`mini-agent-web/server/routes/world_execution.py` 在审查开始前已有未提交修改，将 macOS 目录选择改为调用 `/usr/bin/osascript`。本轮没有改动两个仓库的源代码、文档或 CI，也没有运行仓库测试套件。Web 工作区的原有修改未纳入本记录所在的 Harness 提交。

## 发现

### P1：macOS 目录选择的 Tk 路径会在工作线程创建 AppKit 窗口

Web 的 `POST /api/world/browse-folder` 路由通过 `run_in_executor` 调用 `_ask_directory_dialog`。提交版本的 helper 使用 `tkinter.Tk()` 和 `filedialog.askdirectory()`。在 macOS 上，这条路径会从执行器线程创建 Tk 窗口，与用户提供的 `NSWindow should only be instantiated on the main thread` 崩溃栈一致。崩溃栈中的 `TkMacOSXMakeRealWindowExist` 也指向 Tk/AppKit 初始化路径。

审查时 Web 工作区的未提交修改为 `sys.platform == "darwin"` 单独调用 `osascript`，从独立进程打开系统目录选择器，并将 stderr 中的 `(-128)` 作为用户取消处理。这避免在 Gateway 工作线程直接创建 AppKit 窗口，但该补丁尚未提交，且本轮未完成原生面板的选中和取消操作。

源码：[`world_execution.py` at audited HEAD](https://github.com/civaapple-alt/mini-agent-web/blob/b7a4390e925db00874f8d0eaf28ccf1794b03999/server/routes/world_execution.py)。`browse-folder`、Tk、AppleScript 和取消路径没有专门的 Python 或前端测试。

最小复现：在不含该未提交修改的 Web 版本启动 Gateway，在 Studio 中新建项目并点击“添加文件夹”。检查 Gateway 是否因 Tk/AppKit 主线程异常退出。验证现有 AppleScript 修改时，需在 macOS 上分别完成选择和取消，并确认项目表单仍可继续操作。

### P1，CI 阻塞：Harness 的 Windows CI 在 Docker 沙箱用例失败

Harness 的 Windows 矩阵运行 `cargo test --workspace --locked`。2026-09-23 的 `windows-latest` 运行中，121 项通过，`docker_sandbox_mounts_workspace_and_keeps_container_tmp_ephemeral` 失败。Runner 的 Docker 引擎请求 `alpine:latest` 时报告没有适用于 `windows/amd64` 的镜像。workspace 测试失败后，release 构建和 CLI smoke 步骤被跳过。

证据：[`workspace_tests.rs`](../../../../crates/mini-agent-capabilities/src/workspace_tests.rs) 的 Docker 用例无平台跳过分支；[Harness CI 工作流](../../../../.github/workflows/ci.yml) 对 Windows 执行整个 workspace 测试。

最小复现：在同类 Windows Runner 执行 `cargo test --workspace --locked`，或单独运行 `docker_sandbox_mounts_workspace_and_keeps_container_tmp_ephemeral`，观察 Docker 对 Alpine 镜像的响应。

这是 CI 阻塞证据，不证明 Windows 原生 Shell 或 Job Object 本身失败。Windows 上 PowerShell 7、UTF-8 环境和 Job Object 有专用实现与测试；但本次失败使 Windows release build 和 CLI smoke 没有在该 CI 运行中执行。

### P2：大小写敏感 APFS 上的跨项目 Session 搜索可能漏掉目录

Web 的 `read_any_project_thread`、`list_any_project_thread_items` 和 `session_path_for_thread` 对解析后的工作区路径调用 `casefold()` 去重。项目注册路径使用解析后的 `Path` 比较，不做大小写折叠。在大小写敏感 APFS 上，`/work/Repo` 和 `/work/repo` 可以是两个不同目录，但 Thread 搜索会把它们当成同一个工作区。

源码：[`thread_registry.py`](https://github.com/civaapple-alt/mini-agent-web/blob/b7a4390e925db00874f8d0eaf28ccf1794b03999/server/control/thread_registry.py#L235-L367) 和 [`project_registry.py`](https://github.com/civaapple-alt/mini-agent-web/blob/b7a4390e925db00874f8d0eaf28ccf1794b03999/server/control/project_registry.py#L320-L364)。现有 Gateway 测试未覆盖只改变路径大小写、但底层目录不同的卷。

最小复现：在大小写敏感 APFS 卷创建 `/work/Repo` 和 `/work/repo` 两个项目，在后一个项目保存 Session，再对没有 `project_id` 的 Thread 查询触发跨项目搜索。检查第二个目录是否被去重逻辑跳过。此发现是源码审查结果，本轮没有在大小写敏感卷上复现。

## CI 与测试证据

### Harness

2026-09-23 的[最新 CI 运行](https://github.com/civaapple-alt/mini-agent-harness/actions/runs/35934084548)总体失败。

| Runner | 结果 | 证据 |
| --- | --- | --- |
| `macos-latest` | workspace 测试、release 构建和 CLI smoke 通过 | Runner 镜像日志显示 `macos-26-arm64`。这是 Apple Silicon 运行证据。 |
| `windows-latest` | 121 项通过，Docker Alpine 用例失败 | release 构建和 CLI smoke 被跳过。 |
| `ubuntu-latest` 测试 | 两个 App Server 测试因 `session session is locked by another process or a stale lock` 失败 | `child_wakeup_source_is_live_replayable_and_persisted_on_thread_items` 和 `completed_child_follow_up_starts_a_new_turn_on_the_same_session`。 |
| `quality` | Clippy 失败 | `sandbox.rs:227` 的 `sandbox` 变量只在 `#[cfg(windows)]` 断言中使用，在 Ubuntu 下触发 `unused variable`。 |

Release 工作流为 macOS x86_64 和 aarch64 构建目标，但它是交叉编译矩阵，不提供 Intel Mac 的运行时测试证据。[release.yml](../../../../.github/workflows/release.yml)

### Web

2026-09-23 的[最新 Web CI 运行](https://github.com/civaapple-alt/mini-agent-web/actions/runs/35934104938)在 `ruff format --check .` 失败，报告 12 个文件需要格式化。`python-test` 和 `frontend-test` 都依赖 lint 成功，因此 Windows/macOS Python 矩阵和前端测试均被跳过。

2026-09-15 的[较早绿灯运行](https://github.com/civaapple-alt/mini-agent-web/actions/runs/34956843105)通过了 Windows Python 3.12、macOS Python 3.12 和 Ubuntu 前端测试。该运行针对较早的 Web 提交，不覆盖本次审查时的 HEAD 或工作区中的目录选择修改。

Web CI 的前端单元与组件测试只在 Ubuntu 运行，测试环境使用 Happy DOM。仓库中没有原生目录选择器的自动化测试。Windows CI 也没有原生文件夹面板或 Chrome GUI 测试。

## Mac mini M6 手动冒烟

本机架构为 `arm64`。本轮在隔离临时目录启动 Web Gateway，并使用本地 `mini-agent-app-server`。Gateway `/health` 返回成功，Studio 加载了空闲会话。

点击目录选择入口后，观察到 `osascript` 子进程等待交互，Gateway 健康检查仍成功。连接的桌面控制无法查看或操作原生系统面板，因此没有完成目录选择或用户取消。

随后在 Studio 手动输入临时目录并创建项目。页面显示项目创建成功，新项目的 `default` 会话处于空闲状态。本轮没有发送模型请求。临时项目、Gateway 状态和对应 Session 数据在冒烟结束后已清理。

这次浏览器检查使用 Codex 内嵌浏览器，不是 Chrome。桌面控制只暴露了内嵌浏览器，无法安全操作当时正在使用的 Chrome 窗口。因此 Chrome 实际交互未验证。Windows GUI 环境不可用，Windows 原生文件夹面板和 Chrome 交互未验证。

## 已排除的问题

- M6 上 Python SDK 能启动本地 App Server，Gateway 能完成协议初始化和 Thread 启动。
- Studio 手动输入目录后的项目创建和空 `default` 会话启动成功。
- 本次没有观察到 M6 上的 Gateway 健康检查或 App Server 子进程启动失败。
- Harness 最新 macOS ARM64 CI 的 workspace 测试、release 构建和 CLI smoke 通过。

这些结果不覆盖原生面板选择与取消、Chrome、Windows GUI、Intel Mac 运行时或大小写敏感 APFS。
