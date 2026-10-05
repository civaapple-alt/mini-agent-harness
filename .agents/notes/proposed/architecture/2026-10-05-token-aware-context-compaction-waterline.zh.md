# Token 水位驱动的上下文压缩与字节上限解耦

状态：提案中
日期：2026-10-05
范围：Harness Core、Protocol、Host/模型适配器、App Server、SDK/Gateway、Web Studio
证据基线：当前 Harness 与 Web Studio checkout；用户提供的上下文用量截图

## 一页结论

**建议把自动压缩水位改为模型 token 预算，把 64 MiB 保留为与供应商无关的序列化字节硬上限。** 两个上限回答不同问题，不应再用 64 MiB 的一半来代表模型上下文百分比。

当前默认 `max_context_bytes` 是 `64 * 1024 * 1024` 字节。`Harness::prepare_context` 在序列化上下文达到其一半时尝试压缩，因此当前主动压缩水位实际为 **32 MiB**。这衡量 JSON/UTF-8 字节大小，不等于 256K 或 1M tokens。超过供应商窗口时，Harness 另有一次错误识别后的压缩重试；这不是预先可解释的水位。

建议的目标行为：

1. `max_context_bytes` 继续只负责限制序列化上下文的资源大小，不再触发日常预压缩。
2. 对有可靠模型级 token 估算器的请求，在 Core 发送模型请求前按 token 预算评估并压缩。初始候选水位为可用输入预算的 **80%**；压缩目标候选为 **65%**，形成迟滞，避免连续请求反复压缩。阈值需由 bounded scenario 校准后才能成为当前规范。
3. 估算器由 Host 中所选 provider/model adapter 提供；Core 独占水位判断、压缩和是否继续请求的决定。无可靠估算器时，显示水位未知，保留字节硬上限和现有供应商超窗恢复，不伪造 token 百分比。
4. Provider 返回的实际 `input_tokens` 用于展示最近一次请求，并用于检验估算偏差；不能当作下一次请求的预估值。模型配置的 `contextWindow` 不因单次用量自动改写。
5. Web Studio 主入口参考 Codex 的单一水位读法，显示“已用 tokens / 本次模型可用预算 + 百分比”；详情再列最近请求 usage、缓存和来源估算。usage 必须绑定产生它的模型及当时预算，不能用当前选择的模型窗口去除上一模型请求的 usage。数据不匹配时保留超 100% 数值并提示。

非目标：此提案不改变模型上下文长度、不把 64 MiB 换算成 token、不调整 provider API Key 或 Kimi Coding Base URL，也不引入面向用户的水位配置项。若两个行为确实需要共存，再按仓库默认优先规则讨论配置项。

### 候选预算公式

```text
available_input_tokens = context_window_tokens - reserved_output_tokens
compact_before_request when estimated_input_tokens >= ceil(0.80 * available_input_tokens)
compact toward estimated_input_tokens <= floor(0.65 * available_input_tokens)
```

`reserved_output_tokens` 优先取本次模型配置的输出上限；缺失或超过窗口的配置须在模型配置边界显式处理，不可静默变成零预算。初始例子：窗口 262,144、输出预留 16,384 时，可用输入预算为 245,760，候选触发水位为 196,608；窗口 1,000,000、输出预留 384,000 时，可用预算为 616,000，候选触发水位为 492,800。例子用于解释算法，不代表对任何具体供应商配置的确认。

估算输入必须覆盖实际发出的 system prompt、messages、tool definitions 和当前模型支持的多模态内容。未被估算器覆盖的内容应使结果标记为不可用或低置信度，并进入明确的安全策略；不得把类别字节分摊值冒充预请求 token 估算。

## 现状证据与问题

| ID | 观察与证据 | 影响 / 根因 | 反例或待验证项 |
| --- | --- | --- | --- |
| E1 | `crates/mini-agent-core/src/harness.rs`：`HarnessConfig::default` 将 `max_context_bytes` 设为 64 MiB；`prepare_context` 用 `max_context_bytes / 2` 触发压缩。 | 触发点为 32 MiB 字节，不是模型上下文窗口的 50%。 | 若当前运行时覆盖了默认值，仍需从实际配置 trace 核实；代码默认值本身明确。 |
| E2 | `docs/limits.md` 将 64 MiB 定义为 provider-neutral serialized-context safety ceiling，并说明 profile 的 `contextWindow` 是 Studio token metadata。 | 文档已经区分字节与 token，但运行时主动压缩仍把字节预算的一半作为水位，UI 又按 token 显示百分比，概念容易混淆。 | 无。 |
| E3 | `harness.rs::compact_context_after_window_error` 在识别到 provider 超窗后按当前字节量的 90% 目标压缩，并只重试一次。 | 可恢复 provider 错误，但只有请求失败后才发生，不能解释“达到什么比例会主动压缩”。 | provider 错误分类和一次重试已有测试，仍需留作后备行为。 |
| E4 | Web `InputBar.jsx::ContextUsageControl` 用最近一次 provider `inputTokens` 除以当前 `effectiveEntry.model.contextWindow`；`App.jsx` 从 `model_responded` 写入 usage，Protocol 的 `ModelResponded` 不携带 model ID；窗口字段可在模型设置中编辑。`contextUsage.js::estimateContextCategoryTokens` 按类别字节占比摊分 token。 | 模型切换后分子可能仍属于上一个请求/模型，分母却已是当前模型。类别估算也不能驱动 Core 压缩。 | 需要把请求 model ID、窗口快照与 usage 关联起来，并用跨层 fixture 覆盖切换前后。 |
| E5 | 用户截图先后显示 350,429 / 262,144 = 133.7%，以及当前模型选择 `k3-256k`、配置窗口 262,144、最大输出 64,000、最近输入 431,483 = 164.6%；缓存输入为 431,232。[Kimi Code 模型配置](https://www.kimi.com/code/docs/kimi-code/models.html) 将 `k3-256k` 标为固定 262,144；[错误参考](https://www.kimi.com/code/docs/en/kimi-code/error-reference.html) 说明超限会返回模型 token limit 错误。 | 如果这些 usage 真对应同一 `k3-256k` 请求，成功返回与文档约束冲突；但当前 trace 缺少请求 model ID，截图不能证明 numerator 来自当前模型。缓存命中也不应从总上下文中扣除；[Kimi 缓存用量说明](https://www.kimi.com/academy/best-practices-for-context-caching) 将缓存 tokens 作为总输入的组成部分。 | 检查同一请求的 model ID、HTTP status、request ID 和原始 usage；确定是模型切换后 UI 分母错配、usage 投影问题，还是 provider 行为与文档不符。 |
| E6 | 用户提供的 Codex 上下文指示器截图展示 `68% used` 和 `176K / 258K`。 | 这种“用量 / 预算 + 百分比”的呈现比单独百分比更容易解释压缩水位。截图只能支持视觉结构，不足以推断 Codex 的内部预算口径。 | Mini Agent 必须使用自身同一请求、同一模型的预算来源，不能照抄 Codex 的数字语义。 |

根因不是 MiB 与 token 之间缺少一个全局固定换算率，而是把资源字节限制用于模型 token 水位。UTF-8 字节、JSON/tool schema、多语言文本、图像输入和不同 tokenizer 之间不存在可用于所有模型的稳定换算。即使 provider API 返回了最近一次准确用量，它也不是下一次请求的准确预估。

数量级示例：按用户引用的 [DeepSeek token 用量说明](https://api-docs.deepseek.com/zh-cn/quick_start/token_usage) 中英文字符比例粗估，32 MiB 若全为 ASCII 英文文本约为 10.1M tokens，若全为中文 UTF-8 文本约为 6.7M tokens；这只是纯文本估算，忽略 JSON/tool schema 等结构，也不能套用到 Kimi。它说明 32 MiB 不能解释成 256K 或 1M 窗口的固定占比，不是建议的运行时换算规则。

## 设计选择

| 方案 | 结果 | 结论 |
| --- | --- | --- |
| A. Provider/model 级预请求 token 估算 + Core 水位决策；字节上限独立 | 能提前解释阈值；估算能力随模型适配器声明；未知时明确降级；需要校准估算器和跨层契约。 | **推荐。** 可靠估算器是启用 token 水位的前提，不把通用字符比例包装成精确值。 |
| B. 用最近请求的 token/字节比率推算下一次用量 | 省去 tokenizer，但内容语言、工具定义、图像和历史结构变化会让比例漂移；历史请求也可能不是同一模型配置。 | 不作为压缩权威；可作为离线诊断指标，不输出“实际占用”。 |
| C. 只在 provider 拒绝超窗后压缩重试 | 不需预估算器，现有实现已有一次恢复路径。 | 保留为后备；不满足提前压缩与可解释水位，且失败请求会增加延迟，可能产生费用。 |

不建议把 64 MiB 改成某个更小的固定值或按 DeepSeek 的中英文字符/token 经验比例换算。这样会将一种不通用的估算扩展成全局运行策略，而且无法覆盖 Kimi Coding、工具定义、多模态内容和不同 tokenizer。

## 所有权与契约

| 对象 | 唯一权威 | 允许的职责 | 禁止的职责 |
| --- | --- | --- | --- |
| 模型上下文窗口、输出预留、tokenizer 能力 | Host 所选 model/provider profile 与 adapter | 提供有来源标记的模型级估算能力和配置值。 | Web/Gateway 根据名称猜测上下文长度；用另一模型的 tokenizer 兜底。 |
| 请求 token 估算 | provider/model adapter（Host 实现，Protocol 定义有界输入输出契约） | 在发送前估算完整 `ModelRequest`，返回 token 数、来源/能力状态和置信/限制信息。 | 把类别字节分摊或最近一次 usage 当作本次精确估算。 |
| 预算判断、压缩状态及是否发送 | Core Harness | 使用模型预算估算执行水位和迟滞；超窗错误重试仍是后备。 | Host、App Server、Gateway 或 Web 再实现一套压缩阈值。 |
| 64 MiB 序列化上下文硬限制 | Core Harness | 在预估压缩之后仍限制可保留/发送的序列化上下文。 | 把字节比例显示成 model-window 百分比。 |
| 压缩原因与预算 trace | Protocol Event → App Server 持久化/投影 | 用有界数值字段说明 `token_watermark`、`provider_overflow` 等触发原因；SDK/Gateway 透传。 | 下游根据 usage 重算 Core 的触发决定。 |
| 水位、最近请求 usage 与 mismatch 呈现 | Web Studio | 分栏展示估算水位与 provider 实际 usage；显示窗口 metadata 来源/不可用状态。 | 静默截断超过 100% 的数值、自动改写窗口 metadata 或把类别估算说成实际值。 |

### UI 参考：Codex 的“已用 / 总预算”

用户提供的 Codex 截图显示 `68% 已用`、`已用 176K tokens，共 258K`。提案借用其信息层级：主入口先展示可解释的占用分数与百分比，详情再展示缓存率和来源构成。Mini Agent 的分子应是当前请求模型对应的实际 usage（若尚无成功请求则明确标记估算值/未知），分母应是同一模型按 provider 约束算出的可用上下文预算；不能把当前模型下拉框的值套到一个未绑定模型 ID 的旧 usage 上。Codex 截图不作为其内部 token budget 算法的证据。

候选契约由两个独立值构成：`ContextTokenEstimate`（预请求估算及来源/可用状态）和 `ContextTokenBudget`（窗口、输出预留、水位/目标）。字段必须有硬范围、缺失语义和版本策略。具体 Rust 类型在实现批次开始前确定，避免把可用状态压成 `Option<u32>` 后丢失“不支持”“未配置”和“内容无法估算”的差异。

现有 `Model` trait 只有 provider 超窗错误识别，没有预估算方法；Host profile 有可选 `context_window` 和 `max_output_tokens`，但当前运行时路径是否完整传入所选 profile 仍需实现前追踪确认。现有 `ContextCompactionStarted` 只携带 `before_bytes`，需要评估扩充有界触发原因字段，或在既有事件体系中用不重复的方式记录预算 trace。若扩充公共事件，须同步 App Server、SDK/Gateway、Studio fixture 和协议文档。

## 可证伪验收标准

| ID | 给定 / 操作 | 可观察结果 | 失败反例 | 证据 |
| --- | --- | --- | --- | --- |
| AC-01 | 给定窗口 262,144、输出预留 16,384、候选水位 80%，先发送预估 196,607、再发送 196,608 tokens 的同一请求。 | 前者不因 token 水位压缩；后者在 provider request 之前产生带 `token_watermark` 原因的压缩 trace。 | 只因 32 MiB 字节阈值而压缩，或在 provider request 之后才发现水位。 | Core 单元测试 + bounded Harness Scenario。 |
| AC-02 | 水位触发后模型摘要能使预估值降到可用预算的 65% 以下；另构造摘要只略微缩小但仍高于目标的输入。 | 第一种不在相邻请求重复压缩；第二种继续有界机械裁剪，或在无可裁剪前缀时给出明确的预算失败，不能静默反复重试。 | 仅要求字节变小，后续每个 model step 都重复触发摘要。 | Core 单测及连续多步 Scenario trace。 |
| AC-03 | adapter 未提供可靠估算器，或模型窗口/output reserve 缺失、无效。 | 自动水位显示 unknown/unavailable；Core 不把 bytes 或最近一次 usage 伪装成 preflight estimate；字节硬限制及明确识别的 provider overflow recovery 仍有效。 | 把 unknown 当 0、默认 64 MiB/2，或静默用通用字符比例。 | Protocol/Host 边界测试 + Web fixture。 |
| AC-04 | 使用用户截图中的 usage 350,429、配置窗口 262,144。 | 最近请求仍显示 133.7% 和实际数值，并明确提示 usage 与配置 metadata 不一致；不自动把窗口改成 350,429。 | 百分比被截成 100% 或 UI 声称 provider 已被证实超窗。 | SDK/Gateway/Studio 投影 fixture 与组件测试。 |
| AC-05 | 配置 1M 窗口与 384K 输出预留，使用该模型的受支持 estimator。 | 计算可用输入 616K、触发候选约 492.8K；相同逻辑同时覆盖 system prompt、messages、工具定义及已支持模态。 | 只估 messages 文本，遗漏 system/tools，或把缓存命中率当成窗口容量折扣。 | Provider adapter fixture + Harness Scenario；估算结果与对应模型 tokenizer/官方计数证据对照。 |
| AC-06 | 序列化请求超过 64 MiB，但 token 估算仍低于水位；另构造 token 预算不足而字节较小的输入。 | 前者由独立字节硬限制拒绝；后者由 token 水位提前压缩，表明两个阈值彼此独立。 | 任一条件错误地换算成另一个条件，或实际请求无边界。 | Core boundary tests + bounded Scenario。 |
| AC-07 | provider 明确返回窗口溢出，且请求未触发预估水位。 | 当前最多一次 overflow compaction retry 继续工作，trace 显示 `provider_overflow`；其他 provider 错误不触发该路径。 | 网络/认证错误触发压缩重试，或一次重试失败后无限继续。 | 现有 Core retry tests 保留并补 reason trace 断言。 |

跨层 scenario 至少覆盖中英文混合、较大的工具 schema、工具结果增长和多个连续 model step。CI 不调用付费 provider；真实 provider 用量对照须另行授权，并不能替代本地确定性 scenario。

## 实施批次与停止条件

### Batch 1：确认 Kimi/DeepSeek 等 adapter 的估算能力

- 范围：Host provider/model adapter、模型 profile、Protocol 候选契约。
- 删除/替换：无；先验证可用 tokenizer/官方计数路径、模态覆盖和模型 ID 对齐。
- 契约变化：内部估算结果候选；暂不改公共事件。
- 证据：每个目标模型建立 fixture，记录 tokenizer/计数方法、输出预留来源和已知误差范围。
- 停止条件：目标模型没有可验证且覆盖请求形状的估算方法时，不启用其 token 水位，也不使用通用字符比率假装精确。

### Batch 2：Core token 水位与字节限制分离

- 范围：Core/Protocol。
- 删除/替换：移除 `max_context_bytes / 2` 作为默认主动压缩水位；保留序列化字节硬上限和一次 provider overflow 恢复。
- 契约变化：Core 消费 bounded estimate/budget；压缩 trace 能区分 token 水位与 provider overflow。无估算器时有明确状态。
- 证据：AC-01、02、03、06、07 与 Scenario。
- 停止条件：估算不完整时可能使模型请求越过窗口，或增加行为后 Core + Protocol 超过硬预算且没有等量删除旧概念。

### Batch 3：预算 trace 跨层投影与 Studio 提示

- 范围：App Server、Python SDK、Gateway、Web Studio。
- 删除/替换：复用最近请求 usage 展示，不创建第二套前端阈值决策；将 usage 与估算水位分开展示。
- 契约变化：只增加解释水位所必需的有界事件字段；更新协议文档、SDK 类型、Gateway projection 和 Web fixture。
- 证据：AC-04，检查历史事件缺字段时可读且明确显示 unknown。
- 停止条件：字段需引入无界 payload、前端复制压缩决策，或无法对齐请求和所选 model profile。

### Batch 4：校准阈值并更新当前文档

- 范围：Harness Scenario/Eval、`docs/limits.md`、notes 状态晋级。
- 删除/替换：将“半个 64 MiB 即主动压缩”的当前描述改为独立 token 水位与字节硬上限；旧实现记录保持冻结。
- 证据：至少一个 256K 级与一个 1M 级 profile 场景，覆盖估算误差、连续压缩和无法裁剪情况。
- 停止条件：80%/65% 不能在 scenario 中稳定避免超窗或重复压缩时，调整候选值并重跑，不晋级提案状态。

## PR 六问与预算

1. **归属？** token 估算由 Host provider/model adapter 提供，Core 决定预压缩与否；Protocol 定义有界契约/事件；App Server 保留 trace；SDK/Gateway 透传；Web 只呈现。64 MiB 继续由 Core 作为字节硬限制。
2. **已有所有者？** `Harness::prepare_context` 已拥有压缩决策，`Model::is_context_window_error` 已有 provider 超窗后备；Host profile 已保存 `context_window`/`max_output_tokens`；Web 已呈现 provider usage。应替换字节预压缩水位并补齐估算契约，不在 UI 或 Gateway 新增权威决策。
3. **可删除概念？** 删除“64 MiB 一半等于预压缩水位”的含义；保留真正保护序列化资源的字节上限和 provider overflow 后备重试。不引入全局 byte/token 比率。
4. **行数预算？** 本提案仅改 Markdown，Rust 行数 delta 为 0。当前基线：Core + Protocol 6,703/7,000（余量 297），Control Plane 40,832/45,000，Release Rust 59,073/65,000。实现粗估 Core + Protocol 净增 100–250 行、其他 release 源净增 150–300 行；这些是规划值，第一批实现前需重算。Core + Protocol 目标不得超过现有 297 行余量；若超出必须先删/简化旧分支或缩小契约。每批运行 `python scripts/line_budget.py`，提交前报告实际 delta。
5. **是否扩展可见输入/事件/持久化/协议？** 不修改模型 prompt、tool schema 或历史内容。新增的是有界内部预算计算；若为了 UI 解释扩展 `ContextCompactionStarted`，它会影响公共事件、App Server 持久化投影、SDK/Gateway 和 Web fixtures，须显式做兼容评估。Provider tokenizer 不应把 token 或原文持久化到 history。
6. **测试和缺失证据？** Core boundary tests、Host estimator fixtures、App Server/SDK/Gateway/Web projection fixtures、bounded Harness Scenario/Eval 均可覆盖；当前缺各 provider tokenizer 的可靠性/覆盖证据、真实 `contextWindow` metadata 与 provider usage 的同源证明，以及不同模态估算误差。除非另行授权，CI 不发付费 provider 请求。

## 风险与未决项

- **估算器覆盖**：Kimi Coding 与其他 OpenAI-compatible adapter 是否提供可验证的本地 tokenizer/计数 API，需逐模型确认。没有证据的模型必须保持水位未知；该提案不承诺所有 provider 都能预压缩。
- **metadata 与 usage 语义**：截图中 133.7% 可能来自窗口配置过小/过期、模型 ID/版本不匹配或 usage 语义差异。实现应报告 mismatch，不自动修正 profile，也不把一次成功响应解释成上下文上限已被突破。
- **多模态内容**：图片等输入可能没有按文本 tokenizer 计数；估算器需声明支持范围，超范围请求必须明确降级或拒绝 token 水位判断。
- **模型摘要自身预算**：压缩请求也必须按目标模型的输入/输出预算预检，并保持无工具调用、原有尾部保留和 byte ceiling。不能只校验用户下一条主请求。
- **水位校准**：80%/65% 是便于评审和 scenario 起步的候选值，不是供应商保证。应由误差和连续压缩证据决定是否接受。
- **字节上限解释**：64 MiB 仍可能高于典型 token 窗口可容纳的实际内容；它只防止序列化请求无限增长。若还需降低内存/传输占用，应另开资源预算决策，不要借 token 水位的名义修改。

提案状态保持 `proposed`，直到模型级 estimator 有可复核证据、bounded Scenario 覆盖水位边界和降级路径、跨层事件投影通过 fixture，且当前 `docs/limits.md` 与实现一致。
