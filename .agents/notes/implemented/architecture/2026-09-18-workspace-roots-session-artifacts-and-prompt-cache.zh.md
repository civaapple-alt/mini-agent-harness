# Workspace Roots、Session Artifacts 与 Prompt Cache

状态：implemented

## 决策

Project Workspace Root、Session Artifact 和 Skill Root 是三个不同的权限
边界。主目录是 `primary/read_write`；关联目录分别进入
`associated_read_roots` 或 `associated_write_roots`；启用 Skill 继续使用独立
的 `skill_read_roots`。当前 Thread 的 Gateway 附件只进入
`session_read_roots`，不再混入 Workspace Root，也不允许写入。

Session 的物理目录不是模型可自由访问的目录。Runtime 只注入稳定的逻辑
`session_capabilities`：

- `plan.md`：由 Plan Runtime 控制的 living Plan；
- `goal/...`：由 Goal Runtime 控制的受控目录；
- `notebook`：仅通过 Notebook 工具访问；
- `current_turn_attachments`：当前 Turn 的只读附件能力。

`session.jsonl`、`summary.json`、`prompt_context.json`、审批证据和其他
SessionStore sidecar 不通过普通 `read_file` 开放。未来如需查询历史，增加有界
Session 查询能力，而不是开放原始 JSONL。

## Prompt Cache 语义

`world_state` 和 `session_capabilities` 是可替换的 Context slot，而不是每次
变更都追加的新消息。Workspace 根集合发生变化时，Host 计算确定性的
`root_fingerprint`，重绑 Runtime 并替换 `world_state`；同一 Runtime 的普通 Turn
不会重复追加根目录或 Session 能力。

稳定区域包含根角色、访问边界、工具规则和逻辑 Session 能力。当前 Turn 的附件
物理路径、临时外部路径和工具输出属于动态输入。Session ID、附件随机 ID、mtime、
sidecar 大小不进入稳定 Prompt Context。这样既保留模型对能力的感知，也避免内部
运行状态变化破坏 Provider 的稳定请求前缀。

## 外部路径

用户在 Prompt 中明确写出外部路径只表达意图，不自动改变权限。Host 依次判断主目录、
关联读写目录、当前 Turn 附件和未注册路径：未注册文件读取沿用一次性审批；未注册
写入默认拒绝，加入 Project 关联目录才会改变根 fingerprint 并重绑 Runtime。只读
关联目录和附件目录不能通过 `apply_patch` 修改，脚本读取也不等于脚本执行。

## Child Session

Child Runtime 继承同一 Project 根分类，但拥有自己的 Session Artifact 能力和附件
根。Child 不能通过父 Session 的物理路径读取父 `session.jsonl`；父 Notebook 若
允许访问，仍只能通过现有的验证后只读 Notebook scope。Child 的 Session 恢复不会
改变父 Thread 的 Context slot。

## 验证证据

- `mini-agent-capabilities` 验证附件根可以读取、不能写入，且不能读取相邻
  `session.jsonl`；
- Core 验证替换 Context slot 会去除旧的重复 `world_state`；
- Host 验证 Session 根只出现在 `status_json`，不出现在模型的
  `<workspace_roots>` 或 `<session_capabilities>`；
- Web Gateway 验证附件根进入 `MINI_AGENT_SESSION_READ_ROOTS`，不进入
  `MINI_AGENT_EXTRA_READ_ROOTS`；
- Prompt Context UI 显示工作区根与会话附件根数量，完整 XML 仍按需展开。
