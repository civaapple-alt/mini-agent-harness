# Web Studio 页面配置与首次使用

状态：已实施

## 决策

Web Studio 是供应商、凭据与模型默认值的日常配置入口。Host/App Server 将机器级模型目录保存在 `~/.mini-agent/model_catalog.json`，项目默认模型按项目 ID 保存，Thread 模型与推理等级继续保存在 Session。Gateway 只转发目录操作；Python SDK 和 Studio 使用同一个 App Server 配置。

首次启动不显示向导。没有可用默认模型时，输入框显示“先配置模型”，点击后打开现有模型设置。本地预设提供 DeepSeek、Kimi、GLM、Volcengine 与建议模型 ID，也允许自定义 Responses API 供应商和手动填写模型。页面不回显 API Key；页面加载和保存都不请求 Provider。用户手动测试连接时，Host 发送一条固定提示、禁用工具、限制输出和响应体，并只返回有限状态与短消息。

旧 `OPENAI_*`、`VERIFIER_OPENAI_*` 和 `MINI_AGENT_WEB_SEARCH` 环境变量不配置或覆盖模型目录，旧 `.env` 值也不会自动迁移。供应商搜索能力改由模型目录控制：默认按端点推断，也允许显式开启或关闭。Goal Verifier 保持可选，不影响普通对话；Goal 循环、步数与超时仍是部署配置。`/api/settings` 只保存界面偏好，项目默认值与 Thread 选择写回各自的配置范围。

## 六问准入

1. **责任层。** Host 持有供应商、凭据、默认模型和连接测试；App Server 暴露目录操作；SDK 提供便捷方法；Gateway 做 HTTP 映射；Studio 呈现配置。项目默认值按项目 ID 保存，Thread 选择仍归 Session。
2. **现有责任。** Host 的 `ModelCatalogStore` 是配置真相源；本次扩展既有存储、`model/catalog/manage`、SDK 与现有设置弹窗，没有在 Gateway 或 Studio 创建第二份模型目录。
3. **删除或替换。** 删除 Host 对 `OPENAI_*`、`VERIFIER_OPENAI_*` 和搜索环境变量的模型配置读取、迁移及 CLI 依赖；由 Host 模型目录和页面设置取代。Goal 安全上限、Gateway 启动参数与运行时绑定仍使用环境变量。
4. **行数预算。** 初始估算为 Core + Protocol 不超过 30 行、Control Plane 不超过 150 行、Release Rust 不超过 200 行。实际有效行增量为 Core + Protocol `+0`、Control Plane `+293`、Release Rust `+293`；当前总量分别为 `5,450/6,000`、`33,850/35,000`、`49,312/50,000`。单次 Release Rust 增量检查为 `+293/1,000`，所有门禁通过。
5. **对外扩展。** 模型目录操作增加 `test_connection`，供应商 profile 增加可选搜索策略字段。连接测试使用固定输入、无工具执行、有限输出与超时；结果为封闭状态集合和短消息，不包含 Key、原始请求或响应。无新增模型事件。
6. **边界证据。** Host/App Server 假 Provider 测试覆盖成功、无效凭据、错误端点、超时与传输错误分类；Gateway、SDK、Studio 测试覆盖映射、显式触发、项目默认值和首次配置入口。隔离 HOME 启动验收证明空白目录可启动并创建默认项目和空白 Session，且不调用 Provider。

## 验证

- 受影响 Rust 包执行 `cargo fmt --all`、Clippy 与包级测试：`mini-agent-host`、`mini-agent-app-server-protocol`、`mini-agent-app-server`、`mini-agent-capabilities`、`mini-agent-cli` 均通过。未运行完整 workspace 测试。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json` 通过；Core + Protocol、Control Plane 和 Release Rust 均低于硬上限。
- Web Studio Node/Vitest 测试共 234 项通过；Vite 生产构建通过。构建仍提示主 chunk 超过 500 kB。
- `uv run --locked --group dev pytest` 的 Gateway 与 SDK 相关测试共 49 项通过；Python Ruff 检查通过。
- 使用临时 HOME 且空的 `.mini-agent` 启动真实 Gateway 与 App Server；未设置旧模型环境变量。健康检查、默认项目、空模型目录、默认 Thread 和仅包含 UI 偏好的设置响应均符合预期。没有发起 Provider 请求。

## 后果与限制

升级后，旧 `.env` 中的供应商配置不会继续生效；用户需在 Web Studio 重新录入并保存。连接测试可能产生供应商费用，因此只由用户明确点击触发。Host 仍沿用既有本地凭据文件存储权限模型，不在 App Server、Gateway 或 UI 响应中回显 Key。
