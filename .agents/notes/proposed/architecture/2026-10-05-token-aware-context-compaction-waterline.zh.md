# Token 水位驱动的上下文压缩与字节上限解耦

状态：部分实现；候选比例已落地，窗口中间档位与运行校准待完成
日期：2026-10-05
范围：Harness Core、Protocol、Host/模型适配器、App Server、SDK/Gateway、Web Studio
初始证据基线：本提案实现前的 Harness 与 Web Studio checkout；用户提供的上下文用量截图

## 一页结论

**建议把自动压缩水位改为模型 token 预算，把 64 MiB 保留为与供应商无关的序列化字节硬上限。** 两个上限回答不同问题，不应再用 64 MiB 的一半来代表模型上下文百分比。

提案最初的实现基线中，默认 `max_context_bytes` 为 `64 * 1024 * 1024` 字节，`Harness::prepare_context` 在序列化上下文达到其一半时尝试压缩，因此主动压缩水位实际为 **32 MiB**。这衡量 JSON/UTF-8 字节大小，不等于 256K 或 1M tokens。超过供应商窗口时，Harness 另有一次错误识别后的压缩重试；这不是预先可解释的水位。

2026-10-05 的实现已删除这条 32 MiB 预压缩路径。Harness 现在只在序列化上下文超过字节硬上限时拒绝请求，并保留明确识别的 provider 窗口溢出压缩重试。`model_responded` 与持久化的 `contextUsage` 会保存该次请求所选模型的窗口和最大输出配置，Studio 按完整模型窗口显示最近实际输入占比。Core 也使用成功响应的实际 `input_tokens` 作为反馈水位：窗口不超过 262,144 时 80%，至少 1,000,000 时 50%；输出预留超窗也会触发。压缩在报告用量的响应之后、下一次模型请求之前执行。此行为不预测正在发送的请求，具体运行限制见 [实现记录](../../implemented/architecture/2026-10-05-context-waterline-safety-and-model-snapshot.zh.md)。

候选比例由模型 profile 的上下文窗口大小决定，与 provider/model ID 无关。实现复用成功响应已有的 provider `input_tokens`，不需要把 Kimi Open Platform estimate API 套用到 Kimi Code Key，也不新增凭据或模型映射。请求前 token 估算仍未实现：水位只会在 provider 成功返回 usage 后触发；如果响应没有 usage/窗口快照则跳过比例水位。provider 超窗错误的单次恢复重试保留为后备。

2026-10-05 用户明确该比例按**模型配置的上下文窗口大小**分档，而不是 Kimi 专属：窗口约 100,000 和 256,000 tokens 使用 **80%**；1,000,000 tokens 使用 **50%**。百分比分母是完整上下文窗口，`contextWindow`/`context_window_tokens` 是模型 profile 中配置的窗口 token 数，不是 Harness 字节上限，也不是供应商实时返回或验证的事实。窗口占用率展示与反馈水位因此使用相同分母。触发后复用现有摘要/机械裁剪逻辑，目标按 serialized bytes 至少减少 10%；这不是压缩后 token 数低于特定比例的保证，仍待多轮 Scenario 校准。

当前实现与待校准行为：

1. `max_context_bytes` 继续只负责限制序列化上下文的资源大小，不再触发日常预压缩。
2. 对 `context_limit_behavior=Compact` 的请求，Core 在收到成功响应后使用该请求报告的实际 input usage 评估反馈水位，并在继续模型循环或结束 Turn 前压缩。候选窗口档位为 `context_window_tokens <= 262,144` 时 **80%**（覆盖 100,000、256,000 和 262,144），`context_window_tokens >= 1,000,000` 时 **50%**。中间区间暂不使用百分比水位。
3. 若报告的 `input_tokens + max_output_tokens` 大于完整窗口，Core 独立触发压缩；有效输出预留不改变百分比的完整窗口分母。Core 独占水位判断，provider 超窗错误重试仍作为后备。
4. 没有成功响应 usage 或配置窗口快照时不触发比例水位；上报 usage 只描述刚完成的请求，不当作下一次请求的预估。模型配置的 `contextWindow` 不因单次用量自动改写。profile 与 provider 实际路由不匹配时，水位也会受该 metadata 影响。
5. Web Studio 主入口参考 Codex 的单一水位读法，显示“最近请求已用 tokens / 本次模型完整窗口 + 百分比”；详情再列最大输出预留、可用输入预算、最近请求 usage、缓存和来源估算。usage 必须绑定产生它的模型及当时窗口，不能用当前选择的模型窗口去除上一模型请求的 usage。数据不匹配时保留超 100% 数值并提示。

非目标：此提案不改变模型上下文长度、不把 64 MiB 换算成 token、不调整 provider API Key 或 Kimi Coding Base URL，也不引入面向用户的水位配置项。若两个行为确实需要共存，再按仓库默认优先规则讨论配置项。

### 候选预算公式

```text
trigger_ratio = 0.80 when context_window_tokens <= 262,144
trigger_ratio = 0.50 when context_window_tokens >= 1,000,000
trigger_ratio = unknown for 262,145..999,999
after_successful_response compact when usage.input_tokens >= ceil(trigger_ratio * context_window_tokens)
after_successful_response compact when usage.input_tokens + valid_max_output_tokens > context_window_tokens
compact with the existing summary/mechanical reducer before the next model request
```

`valid_max_output_tokens` 指不大于窗口的配置值；缺失或大于窗口时，跳过输出预留检查，不把无效配置当作零预算。候选水位示例：100,000 窗口为 80,000 tokens；256,000 窗口为 204,800；262,144 窗口为 209,716；1,000,000 窗口为 500,000；1,048,576 窗口为 524,288。百分比按完整窗口计算，最大输出预留是独立的提前触发条件。因为实际 usage 在请求成功后才可得，反馈水位不能阻止刚越过阈值的当前请求；provider 显式超窗恢复仍覆盖该路径。

## 现状证据与问题

| ID | 观察与证据 | 影响 / 根因 | 反例或待验证项 |
| --- | --- | --- | --- |
| E1 | 初始实现证据：`crates/mini-agent-core/src/harness.rs` 的 `HarnessConfig::default` 将 `max_context_bytes` 设为 64 MiB；`prepare_context` 用 `max_context_bytes / 2` 触发压缩。 | 初始触发点为 32 MiB 字节，不是模型上下文窗口的 50%。本批已删除这条字节比例预压缩路径。 | 若运行时覆盖了默认值，仍需从实际配置 trace 核实；字节硬上限仍可配置。 |
| E2 | 初始 `docs/limits.md` 将 64 MiB 定义为 provider-neutral serialized-context safety ceiling，并说明 profile 的 `contextWindow` 是 Studio token metadata。 | 当时文档区分字节与 token，但运行时却把字节预算的一半作为水位，UI 又按 token 显示百分比。当前规范已明确分开字节拒绝、provider overflow 恢复和模型 profile 展示。 | 可靠的请求前 token estimator 尚未实现。 |
| E3 | `harness.rs::compact_context_after_window_error` 在识别到 provider 超窗后按当前字节量的 90% 目标压缩，并只重试一次。 | 可恢复 provider 错误，但只有请求失败后才发生，不能解释“达到什么比例会主动压缩”。 | provider 错误分类和一次重试已有测试，仍需留作后备行为。 |
| E4 | 初始实现证据：Web `InputBar.jsx::ContextUsageControl` 用最近一次 provider `inputTokens` 除以当前 `effectiveEntry.model.contextWindow`；Protocol `ModelResponded` 不携带 model ID；类别估算按字节占比摊分 token。 | 模型切换后分子可能仍属于上一个请求/模型。本批已将 selection、窗口和输出上限快照随 `ModelResponded` 持久化，并让 Web 绑定该快照；类别估算仍只是展示分摊，不能驱动 Core 压缩。 | 需用真实 provider 请求确认 provider usage 与模型 profile 的对应关系；当前证据为 bounded fixture。 |
| E5 | 用户截图先后显示 350,429 / 262,144 = 133.7%，以及当前模型选择 `k3-256k`、配置窗口 262,144、最大输出 64,000、最近输入 431,483 = 164.6%；缓存输入为 431,232。[Kimi Code 模型配置](https://www.kimi.com/code/docs/kimi-code/models.html) 将 `k3-256k` 标为固定 262,144；[错误参考](https://www.kimi.com/code/docs/en/kimi-code/error-reference.html) 说明超限会返回模型 token limit 错误。 | 如果这些 usage 真对应同一 `k3-256k` 请求，成功返回与文档约束冲突；但当前 trace 缺少请求 model ID，截图不能证明 numerator 来自当前模型。缓存命中也不应从总上下文中扣除；[Kimi 缓存用量说明](https://www.kimi.com/academy/best-practices-for-context-caching) 将缓存 tokens 作为总输入的组成部分。 | 检查同一请求的 model ID、HTTP status、request ID 和原始 usage；确定是模型切换后 UI 分母错配、usage 投影问题，还是 provider 行为与文档不符。 |
| E6 | 用户提供的 Codex 上下文指示器截图展示 `68% used` 和 `176K / 258K`。 | 这种“用量 / 预算 + 百分比”的呈现比单独百分比更容易解释压缩水位。截图只能支持视觉结构，不足以推断 Codex 的内部预算口径。 | Mini Agent 必须使用自身同一请求、同一模型的预算来源，不能照抄 Codex 的数字语义。 |

根因不是 MiB 与 token 之间缺少一个全局固定换算率，而是把资源字节限制用于模型 token 水位。UTF-8 字节、JSON/tool schema、多语言文本、图像输入和不同 tokenizer 之间不存在可用于所有模型的稳定换算。即使 provider API 返回了最近一次准确用量，它也不是下一次请求的准确预估。

数量级示例：按用户引用的 [DeepSeek token 用量说明](https://api-docs.deepseek.com/zh-cn/quick_start/token_usage) 中英文字符比例粗估，32 MiB 若全为 ASCII 英文文本约为 10.1M tokens，若全为中文 UTF-8 文本约为 6.7M tokens；这只是纯文本估算，忽略 JSON/tool schema 等结构，也不能套用到 Kimi。它说明 32 MiB 不能解释成 256K 或 1M 窗口的固定占比，不是建议的运行时换算规则。

## 设计选择

| 方案 | 结果 | 结论 |
| --- | --- | --- |
| A. 使用已完成请求的 provider input usage 作反馈水位；字节上限独立 | 无需另一个 tokenizer；水位按同次模型窗口解释；跨 provider 通用。阈值只能在成功响应后确认，刚越线的当前请求可能已被接受。 | **已实现。** 缺 usage/window 时跳过水位，保留 provider overflow 后备。 |
| B. Provider/model 级预请求 token 估算 + Core 水位决策 | 能在发送前预测并提前压缩；需要模型级估算能力、覆盖输入形状并校准误差。 | 可作为后续增强；不以通用字符/字节比例代替估算器。 |
| C. 只在 provider 拒绝超窗后压缩重试 | 不需预估算器，现有实现已有一次恢复路径。 | 仅作后备；失败请求会增加延迟，可能产生费用。 |

不建议把 64 MiB 改成某个更小的固定值或按 DeepSeek 的中英文字符/token 经验比例换算。这样会将一种不通用的估算扩展成全局运行策略，而且无法覆盖 Kimi Coding、工具定义、多模态内容和不同 tokenizer。

## 所有权与契约

| 对象 | 唯一权威 | 允许的职责 | 禁止的职责 |
| --- | --- | --- | --- |
| 模型上下文窗口、输出预留、tokenizer 能力 | Host 所选 model/provider profile 与 adapter | 提供有来源标记的模型级估算能力和配置值。 | Web/Gateway 根据名称猜测上下文长度；用另一模型的 tokenizer 兜底。 |
| provider token usage 与窗口配置 | Host 所选 model/provider adapter 与 model profile | 响应报告实际 usage；profile 快照附上本次模型的窗口和最大输出配置。 | 将最近 usage 当作下一个请求的精确估算，或猜测 profile 未配置的窗口。 |
| 预算判断、压缩状态及是否继续 | Core Harness | 根据成功响应的实际 usage 和同次 profile 快照执行反馈水位；超窗错误重试仍是后备。 | Host、App Server、Gateway 或 Web 再实现一套压缩阈值。 |
| 64 MiB 序列化上下文硬限制 | Core Harness | 在预估压缩之后仍限制可保留/发送的序列化上下文。 | 把字节比例显示成 model-window 百分比。 |
| 压缩 trace | Protocol Event → App Server 持久化/投影 | 沿用现有压缩开始/结束事件记录反馈水位和 provider overflow 引发的压缩。 | 下游根据 usage 重算 Core 的触发决定。 |
| 最近请求 usage 与 mismatch 呈现 | Web Studio | 展示最近 provider 实际 usage 和其模型窗口 metadata；窗口来源缺失时显示未知。 | 静默截断超过 100% 的数值、自动改写窗口 metadata 或把类别估算说成实际值。 |

### UI 参考：Codex 的“已用 / 总预算”

用户提供的 Codex 截图显示 `68% 已用`、`已用 176K tokens，共 258K`。Mini Agent 借用其信息层级：主入口显示最近实际输入 tokens 与本次请求模型的完整配置窗口；详情再展示缓存率和来源构成。输出预留单独显示为可用输入预算。Codex 截图不作为其内部 token budget 算法的证据。

实现复用现有 `ModelUsage` 与 `ModelContextSnapshot`，没有增加 token estimate API、事件字段或持久化字段。现有 `ContextCompactionStarted/Finished` 沿用在模型响应之后观察自动压缩。

## 可证伪验收标准

| ID | 给定 / 操作 | 可观察结果 | 失败反例 | 证据 |
| --- | --- | --- | --- | --- |
| AC-01 | 成功响应分别报告 100,000 / 256,000 / 262,144 / 1,000,000 窗口的输入 usage，边界在 `80,000 / 204,800 / 209,716 / 500,000` tokens 前后；另测窗口 262,144、输出预留 64,000、输入 usage 198,145。 | <=262,144 的窗口按 80%、>=1,000,000 的窗口按 50% 完整窗口水位触发；中间窗口不套比例；输出可行性独立触发。压缩事件在成功响应之后、Turn 完成之前发生。 | 绑定供应商或模型 ID 决定比例，以扣除输出预留后的预算代替水位分母，或把真实 usage 说成下次请求的预估。 | Core 单元测试 + bounded Harness Scenario。 |
| AC-02 | 两个供应商的模型配置相同 `context_window_tokens`；provider 返回达到水位的成功响应。 | 相同窗口值命中相同档位，与 provider/model ID 无关；Core 使用既有 summary/mechanical reducer，并在继续下一步前压缩。 | 同一窗口因供应商或模型 ID 不同而采用不同比例，或把压缩决策复制到 Host/Web。 | Core 单测及连续多步 Scenario trace。 |
| AC-03 | 成功响应缺 usage、窗口快照缺失或中间窗口无档位。 | Core 跳过比例水位；字节硬限制和明确识别的 provider overflow 恢复仍有效。`Reject` 行为不自动压缩。 | 把 unknown 当 0、默认 64 MiB/2，或静默用通用字符比例。 | Core 边界测试 + App Server Scenario。 |
| AC-04 | 使用用户截图中的 usage 350,429、配置窗口 262,144。 | 最近请求仍显示 133.7% 和实际数值，并明确提示 usage 与配置 metadata 不一致；不自动把窗口改成 350,429。 | 百分比被截成 100% 或 UI 声称 provider 已被证实超窗。 | SDK/Gateway/Studio 投影 fixture 与组件测试。 |
| AC-05 | profile 配置 1,000,000 或 1,048,576 窗口，成功响应报告输入 usage 达到 500,000 或 524,288。 | 分别按完整窗口的 50% 在响应后触发；压缩后继续或结束 Turn，profile 不随 usage 自动改写。 | 误用 80%、可用输入预算为分母，或只因 provider ID 而选择阈值。 | Core 单测 + App Server bounded Scenario。 |
| AC-06 | 序列化请求超过 64 MiB；另构造已成功响应 usage 达水位但字节远低于硬上限。 | 前者在 provider 请求前由字节硬限制拒绝；后者由 token usage 水位触发响应后压缩，两个限制彼此独立。 | 把字节数换算成 token 或用 64 MiB 比例决定窗口水位。 | Core boundary tests + bounded Scenario。 |
| AC-07 | provider 明确返回窗口溢出，且没有可用成功 usage。 | 当前最多一次 overflow compaction retry 继续工作；其他 provider 错误不触发该路径。 | 网络/认证错误触发压缩重试，或一次重试失败后无限继续。 | 现有 Core retry tests 保留并补 reason trace 断言。 |

跨层 scenario 至少覆盖中英文混合、较大的工具 schema、工具结果增长和多个连续 model step。CI 不调用付费 provider；真实 provider 用量对照须另行授权，并不能替代本地确定性 scenario。

## 实施批次与停止条件

### Batch 1：Core 反馈水位（已实现）

- 范围：Core `Harness` 与现有 `ModelContextSnapshot`/`ModelUsage`。
- 删除/替换：不恢复 byte/token 换算；复用 Core 现有压缩 reducer 和 provider overflow 后备。
- 契约变化：无公共协议新增；成功响应 usage 达阈值后压缩。
- 证据：窗口边界单测、输出预留边界单测、App Server bounded scenario。
- 停止条件：没有成功 usage 或窗口快照时跳过水位，不合成估算值。

### Batch 2：跨层投影与当前文档（部分实现）

- 范围：Protocol、App Server、Python SDK、Gateway、Web Studio、`docs/limits.md`。
- 删除/替换：无新的公共事件字段；复用 `ModelResponded` 中 usage/profile 快照和已有压缩事件。
- 契约变化：展示完整窗口占比；自动压缩是响应后的反馈动作，不是预请求估算。
- 证据：SDK/UI fixtures、已有 App Server profile scenario 和本次压缩 scenario。
- 停止条件：UI 不把 profile metadata 描述成 provider 验证值，也不把上一请求 usage 展示成下一请求预测。

### Batch 3：中间窗口与多轮行为校准（待完成）

- 范围：Core/App Server bounded Scenarios 与 `docs/limits.md`。
- 删除/替换：无；中间窗口目前不触发百分比水位。
- 契约变化：只有用户确认档位后才增加中间窗口规则；不引入逐 provider ID 的阈值。
- 证据：工具结果突增、连续 tool/model 步骤、输出 reserve、无可裁剪前缀和缺失 usage。
- 停止条件：若比例在这些 Scenario 中导致重复压缩或 provider overflow 增多，先校准窗口档位/压缩目标。

## PR 六问与预算

1. **归属？** Core `Harness` 使用当前成功响应的 usage 和其 `ModelContextSnapshot` 决定是否压缩；Host 提供 profile 快照，App Server 只持久化既有事件，Web 展示。
2. **已有所有者？** `Harness` 已拥有压缩与超窗重试；`Model::context_snapshot` 和 `ModelResponse.usage` 已分别提供配置窗口与真实请求 usage。复用这些类型，不在 Host/Gateway/Web 建第二个判断。
3. **可删除概念？** 日常阈值不再依赖 `max_context_bytes / 2`；64 MiB 保留为序列化硬限制，显式 fork compaction 和 provider overflow 的既有恢复路径保留。
4. **行数预算？** Core + Protocol `6,797 -> 6,903`（`+106`）；Control Plane `40,964 -> 41,117`（`+153`）；Release Rust `59,299 -> 59,558`（`+259`）。Core + Protocol 余量 97 行；实现 delta 低于 1,000 行参考值，仍需每次验证硬预算。
5. **可见面变化？** 不新增模型输入、公共字段、持久化字段或协议版本；沿用现有 `ModelResponded` usage/profile 和压缩开始/结束事件。自动压缩仍会改写同一 Session 历史，遵循现有 compaction 规则。
6. **测试和缺失证据？** Core 覆盖 100K、256K/262,144、1M/1,048,576 边界及输出预留；App Server bounded scenario 覆盖 1M 窗口的 50% 响应后压缩。未用真实付费 provider；profile 是否与供应商实时路由限制一致仍无法由 fixture 证明。

## 风险与未决项

- **响应后反馈**：真实 usage 只在 provider 成功响应后得到；压缩不会阻止刚越过水位的这次请求。没有 usage/profile 时不触发比例水位，保留 provider 明确超窗恢复。
- **metadata 与 usage 语义**：截图中 133.7% 可能来自窗口配置过小/过期、模型 ID/版本不匹配或 usage 语义差异。实现应报告 mismatch，不自动修正 profile，也不把一次成功响应解释成上下文上限已被突破。
- **多模态内容**：以 provider 报告的 `input_tokens` 作为已完成请求的事实，因此不依赖文本 tokenizer 覆盖；不同 provider 对 usage 字段的定义仍需检查。
- **模型摘要自身预算**：压缩请求也必须按目标模型的输入/输出预算预检，并保持无工具调用、原有尾部保留和 byte ceiling。不能只校验用户下一条主请求。
- **水位校准**：<=262,144 窗口 80%、>=1,000,000 窗口 50% 按用户要求落地；中间窗口的比例待定。压缩目标以当前 serialized-byte reducer 回落 10%，而不是 token 目标保证；还需多轮场景证据判断是否足够。
- **字节上限解释**：64 MiB 仍可能高于典型 token 窗口可容纳的实际内容；它只防止序列化请求无限增长。若还需降低内存/传输占用，应另开资源预算决策，不要借 token 水位的名义修改。

剩余提案范围是中间窗口档位、多轮/工具输出突增行为和压缩目标校准。若这些结果需要新档位，按模型配置窗口值更新通用规则，不为 Kimi 或其他 provider 单独命名比例。
