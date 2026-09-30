# Web Studio 会话上下文可视化与缓存友好注入

- status: implemented
- date: 2026-09-30

## 决策与边界

会话上下文来源由 Host 发现，Core Session 保持有界追加历史，App Server 持久化事件和
presentation，Gateway 只投影元数据，Web Studio 展示时间线和当前用量。Core 不读取文件，
Gateway 不持有授权状态，也不复制来源正文。

1. **所属层：** Host 加载工作区规则并在结构化文件操作前检查嵌套规则；Core 维护有界、
   可压缩的消息序列和请求字节拆分；Protocol 定义注入及用量事件；App Server 持久化
   presentation；SDK、Gateway 和 Web Studio 投影和呈现。
2. **现有职责：** `SessionState` 继续作为模型消息唯一来源；来源元数据从带 metadata
   envelope 的 Context 消息恢复。会话 presentation 仅用于界面活动记录和最近一次用量。
3. **替代旧概念：** 移除了 Context slot 原位替换接口，动态快照只追加；指纹相同则不重复
   添加，版本更新通过 `supersedes` 关联。压缩按来源 slot 保留最新有效快照。
4. **预算：** Core + Protocol `5,588 -> 6,101`（`+513`），Control Plane
   `34,786 -> 35,614`（`+828`），Release Rust `50,424 -> 51,809`（`+1,385`）。Release
   `+1,000` 是审查参考值，不是硬门禁。
5. **可见面：** 新增仅含来源类别、工作区、相对路径、作用范围、字节、指纹和版本关系的
   `ContextInjected` 事件；`ModelResponded` 附带请求前输入字节拆分。Host 单条注入上限为
   64 KiB，工作区根和适用规则数量及聚合字节另有限制。浏览器投影不包含 Context 正文。
6. **边界证据：** Harness mock scenario 验证主工作区、附加工作区、嵌套 `AGENTS.md`，
   结构化读取先追加规则再重试，Shell 不扫描路径；Gateway 和 Web 单测覆盖恢复、元数据
   过滤、普通文件读取区分及 Provider 缓存用量状态。

## 实施

- 会话启动时加载主工作区和配置的读取根目录 `AGENTS.md`；`read_file`、`read_image`、
  `apply_patch` 的已授权目标路径可确定适用子目录规则时，Host 先将规则注入，再让模型
  重新评估并重试。Shell 不做路径预扫描，现有授权和执行边界保持在 Host。
- 项目规则、Skill 目录与激活 Skill 正文、工作区状态和其他动态上下文按事件顺序追加。
  稳定 system prompt 和 tool schema 保持不变，以避免上下文更新改写已有请求前缀。
- 来源元数据随 Context 消息、Thread presentation 和检查点保留。Gateway 从历史检查点恢复
  元数据时过滤 Context/system 正文；旧会话没有来源元数据时不合成来源。
- Web Studio 的上下文来源卡片区分 Host 注入和普通工具读取。侧栏展示来源字节构成；输入区
  展示最近一次模型请求输入和 Provider 回报的 cached token 总量。来源 token 数按字节占比
  估算并标为估算值；无模型窗口或 Provider 用量时显示未知，缓存 token 不按来源分摊。

## 验证

- Rust：
  `cargo test -p mini-agent-protocol -p mini-agent-core -p mini-agent-capabilities -p mini-agent-host -p mini-agent-app-server-protocol -p mini-agent-app-server -p mini-agent-cli -- --test-threads=1`：通过；CLI 在事件装箱调整后另跑 `cargo test -p mini-agent-cli -- --test-threads=1`：16 个单测、11 个交互测试通过。
- `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check`：通过。
- 前端 `npm test`：86 个 Node 测试、29 个 Vitest 文件中的 160 项 UI 测试通过；`npm run lint` 和 `npm run build` 通过。Build 报告现有单个约 772 KiB chunk 的大小提示。
- Gateway 新增 4 项上下文投影测试通过，`tests/sdk/test_sdk_events.py` 的 44 项通过；相关 Python 文件 Ruff 检查和格式检查通过。
- 另跑完整 `tests/gateway/test_session_manager.py` 与 SDK 组合时，结果为 155 通过、34 失败；失败集中在未修改的 SessionManager 广播和 Child 控制测试。上下文新增的 4 项 Gateway 测试和 44 项 SDK 测试单独通过。
- 未发起 Provider 请求，也未做浏览器手工冒烟检查。

## 影响与剩余风险

Provider 是否命中缓存仍以其实际用量字段为准，追加式上下文不保证固定命中率。界面中的
类别 token 拆分是字节比例估算。上述 Gateway 全文件测试失败需要独立排查；本次新增路径有
定向测试覆盖。
