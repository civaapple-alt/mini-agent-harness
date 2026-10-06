# macOS 开发环境能力探测

状态：implemented；日期：2026-10-06；范围：Host、App Server、Web Studio

## 决策

Host 沿用有界 `WorldState` 探测，不扫描整台机器，也不新增 RPC。macOS 专有命令包括 `brew`、`swift`、`swiftc`、`xcrun` 和 `xcodebuild`。普通命令检查可执行文件；`xcodebuild` 还必须在两秒内通过 `xcodebuild -version`，避免把 Command Line Tools 的占位程序报告为完整 Xcode 能力。

检测到 `python3` 后，Host 无网络运行 `python3 -m pip --version`，最多等待两秒。只有该解释器能导入 pip 时，才报告 `python3 -m pip`。探测失败或超时不会触发安装。

Blender 优先从 `PATH` 查找；macOS 只检查 `/Applications/Blender.app` 和 `~/Applications/Blender.app` 的固定 CLI 路径。应用能力携带可调用的 CLI 路径。工作区根目录含 `.blend` 文件时，项目类型增加 `blender` 标记，不递归扫描子目录。

## 所有权与限制

Host 执行探测并生成有界模型上下文；App Server 暴露现有 `world/state` 与 `world/refresh` 结果；Web Studio 分开显示 `PATH` 命令和带 CLI 路径的应用能力。探测只描述环境，不扩大工具权限或执行能力。

固定的 Blender 路径范围可预测且测试稳定，但不会发现用户自定义目录中的 `.app`。本轮没有引入通用应用扫描。

## 验证证据

- `world.rs` 覆盖命令目录、Xcode 失败与超时、pip 模块检查、Blender 固定路径和根目录 `.blend` 标记。
- App Server 场景 `world_state_probe_projection_reaches_the_model_through_app_server` 验证新增结果进入模型上下文，并检查上下文仍有长度界限。
- `docs/world-state.md` 记录探测范围和不可用条件。对应 Web 展示位于 `docs/workspaces.md` 与 `EnvironmentToolsCard`。
- 实现提交：Harness `12b705c`，Web `2fdbbd5`。本文补录只核对了代码、测试和文档，没有重跑测试。
