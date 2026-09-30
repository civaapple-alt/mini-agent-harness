# 运行中 Skill 发现刷新与稳定 Prompt 前缀

状态：已实现。该记录说明 App Server、Host、Capabilities、Python SDK、Gateway
与 Web Studio 如何让对话中安装的 Skill 在下一轮可用，并保持稳定 system prompt
不随 Skill 目录变化。

## 问题与根因

Skill 目录只在 Host 组装 Runtime 时扫描一次。运行中的 App Server 保留旧
`Discovery` 与 `capability_manifest`；Gateway 的 `/api/skills` 和 Web Studio
输入解析继续读取启动快照，Host 显式激活校验也会拒绝启动后安装的名字。
因此，安装命令虽然成功，Skill 仍要等 Runtime 重启才会进入目录。

## 变更边界

- App Server 增加线程限定的 `skills/list` RPC。Management 通过 Host 保存的
  `SkillDiscoveryRefresh` 重扫有效目录，并在同一 Runtime Actor 操作内更新
  Skill 元数据和 Capabilities 的只读 Skill 根集合。
- 每轮开始前由 App Server 再刷新一次，使输入框短暂落后时 Host 仍以最新
  有效清单校验；未变化的发现指纹不会替换 catalog Context。
- Python SDK 与 Gateway 转发 `skills/list`；Web Studio 在读取目录及 Turn
  结算后刷新清单，并按当前 Thread/Project 丢弃迟到响应。
- 已发现 Skills 的 metadata 与显式激活 body 进入两个有界 Context slot。
  激活 body 按 Turn 替换，不再拼入 system prompt；稳定 system prompt 与工具
  schema 保持运行时配置不变。

## 缓存考虑

LLM API 前缀缓存依赖请求前缀保持一致。Skill 目录刷新或 `$skill` 选择变化时，
现在不会重写稳定 system prompt；catalog 只有在发现指纹变化时才更新，激活内容
只更新自己的 bounded context item。Context item 仍属于模型请求历史，因此更新
它可能令该位置之后的请求前缀失配；实现不承诺具体缓存命中率，也不通过重复累积
旧 Skill body 来追求前缀一致。没有发起真实 Provider 请求，缓存命中率未测量。

## 变更准入问题

1. 归属：发现和刷新策略由 Host 提供；Skill 读取授权由 Capabilities 持有；App
   Server 暴露刷新 RPC 并在 Turn 边界调用；SDK、Gateway、Studio 只请求和展示。
2. 既有责任：沿用 `Discovery`、`RuntimeManagementState`、Runtime Actor、已有
   `SkillReadRoots` 权限检查及 bounded Context slot，没有增加 Gateway 目录扫描。
3. 替换或移除：替换启动时静态 Skills 清单和 system-prompt 拼接方式；不增加文件
   watcher、重启流程或第二套授权缓存。
4. 行数：无 Core 新概念；Protocol 增加单一 list 请求/结果和能力位，Control
   Plane 增加刷新工厂、Runtime RPC 与动态读取根句柄。实际预算由
   `python scripts/line_budget.py` 验证。
5. 可见面：新增 `skills/list` 公共 JSON-RPC 方法；目录与激活 Context 保持既有
   有界输入规则，不返回物理路径或 Skill body。
6. 证据：App Server RPC 场景验证运行中安装后清单刷新；Capabilities 用例验证
   同一 Workspace Tool 的 Skill 只读授权随 Host 根集合更新；App Server 场景
   验证选中 Skill 不改变 system prompt，Skill body 位于独立 Context slot。

## 验证

- `cargo fmt --all` 与受影响包的 Clippy 通过。
- `cargo test -p mini-agent-capabilities -p mini-agent-host
  -p mini-agent-app-server-protocol -p mini-agent-app-server`：295 项通过。
- `python3 scripts/test_line_budget.py`：17 项通过；最终预算为 Core + Protocol
  `5,588/6,000`、Control Plane `34,786/38,000`、Release Rust `50,424/55,000`。
- Gateway 与 SDK 定向 pytest：46 项通过；Ruff check 与 format check 通过。
- Web Studio API 用例：3 项通过；前端 ESLint 与生产构建通过。
- 两仓 `git diff --check` 通过。测试使用本地 mock model，不调用收费 Provider；
  真实 API 缓存命中率和浏览器端完整对话流程未测量。
