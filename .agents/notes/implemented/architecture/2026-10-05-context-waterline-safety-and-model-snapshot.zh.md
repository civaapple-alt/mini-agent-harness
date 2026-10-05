# 上下文字节硬限制与模型快照

状态：已实现（窗口中间档位与校准仍待定）；日期：2026-10-05；范围：Core、Protocol、Host、App Server、Python SDK、Web Studio

## 结果

移除 Harness 将 `max_context_bytes / 2` 当作日常主动压缩水位的行为。默认
64 MiB 现在只限制序列化请求大小：超过上限时，在发送 provider 请求前返回本地
`ContextBytes` 限制错误。若 provider 明确报告所选模型的上下文窗口溢出，Harness
仍压缩历史并对该模型步骤重试一次；普通 provider 错误不触发这条恢复路径。

Core 还会使用成功模型响应中的 provider `input_tokens` 作为反馈水位：profile
窗口不超过 262,144 tokens 时为完整窗口的 80%，窗口至少 1,000,000 tokens 时为
50%；中间窗口暂不设百分比。若报告的输入 tokens 加有效 `maxOutputTokens` 超过完整
窗口，也会触发压缩。压缩在报告该用量的响应之后执行，位于下一次模型请求之前；若这是
最终响应，则在该 Turn 完成前压缩。没有实际 usage 或窗口快照时跳过水位判断。该行为不
预估正在发送的请求；provider 拒绝超窗时仍使用一次恢复重试。

水位使用 provider 报告的实际 `input_tokens`，不把字节数或字符数换算成 token，也不需要
另一个 provider 估算端点。请求前 token 估算仍未实现；所以对刚越过水位的成功请求，压缩
会在响应后才发生，不能保证拦住这次请求。64 MiB 只表示序列化请求上限，不代表 Kimi 或
其他模型的 token 窗口。

## 每次请求的模型配置快照

为了让最近一次 provider 用量与实际请求的模型配置对应，本批增加了可选的
`ModelContextSnapshot`，记录 provider/model ID、模型 profile 配置的上下文窗口和最大输出 token。
其中 `contextWindow` / `context_window_tokens` 是按 token 填写的模型上下文窗口元数据，
不是 Harness 的字节限制，也不是供应商实时返回或验证的值：

1. Host 从本次请求选择或项目/全局默认中解析模型 profile。
2. Core 在请求前读取该 profile 快照；成功的 `model_responded` 事件携带可选
   `model_context` 字段。
3. App Server 将快照与最近一次 usage 一起持久化到
   `contextUsage.modelContext`。旧 Session 缺少此字段仍可读取。
4. Python SDK 解析并导出该字段，Web Studio 用最近一次请求快照计算完整窗口占用率，并单独展示输出预留后的可用输入预算。

Studio 的窗口占用率按最近实际 `input_tokens / contextWindow` 计算；“可用输入预算”
仍按配置窗口减去配置的最大输出计算，如果没有输出上限则仅以配置窗口作为预算。
如果没有模型快照，窗口显示为未知。UI 保留高于 100% 的实际输入数值：超过配置窗口时
提示核对请求路由和模型；超过扣除输出预留后的预算时提示实际输出上限可能需要下调。
这个比较是对最近实际 usage 的解释，不是下一次请求的估算，也不会触发 Core 压缩。
按上下文来源分摊 token 的详情仍是字节占比估算。

## Kimi Code 的边界

保留 Kimi Code provider 的 Base URL、Key 和模型配置。当前[官方模型配置](https://www.kimi.com/code/docs/kimi-code/models.html)列有 `k3`、
`k3-256k`、`kimi-for-coding` 与高速版，窗口依套餐和模型而异；本实现只记录本地
profile 配置，不声称该配置已经被 provider 验证。provider 返回明确超窗错误时仍走
一次恢复重试。

[Kimi API 开放平台有独立的 token estimate API](https://platform.kimi.com/docs/api/estimate)，但它使用开放平台 `MOONSHOT_API_KEY`，
与 [Kimi Code Key 不通用](https://www.kimi.com/help/kimi-api/api-troubleshooting)；目前公开估算模型 ID 也不等同于所有当前 Kimi Code model
ID。为了保留用户选定的 Kimi Code 配置，本批没有新增第二个 provider 密钥或将模型
名映射到另一产品。没有执行真实 provider 请求或产生付费调用。

## 验证与限制

- Core 单测覆盖：超过旧半字节门槛但低于硬上限时继续发送；超过硬上限时不做预压缩
  且不调用模型；明确 provider overflow 时仍只压缩重试一次；窗口尺寸水位边界和输出预留
  提前触发。
- App Server bounded scenario 覆盖 1M 窗口 50% 水位：响应先报告 500K 实际输入用量，随后
  在 Turn 完成前发出压缩事件；另有大历史上下文、模型快照和输出路径场景。
- Rust 包测试：Protocol、Core、Host、Capabilities、App Server 通过。
- 强杀恢复 fixture 改为等待 marker 成为完整 JSON，避免把文件创建瞬间误判为已写完。
- Python SDK 模型快照解析测试通过；SDK Ruff 通过。SDK 全量测试有一项不在本批改动范围的
  `test_read_loop_forwards_user_question_notifications_to_thread_stream` 失败：它期望
  普通 dict 通知，实际收到 `UserQuestionNotification` typed object，同时测试记录了
  stream closed runtime error。该失败与本次模型快照改动无关，未改其行为。
- Web Studio 的 Node/Vitest 定向测试、lint、build 通过；build 有 Vite bundle 大于
  500 kB 的提示。未做手动浏览器 smoke。
- `cargo fmt --all`、受影响 Rust 包 Clippy、Harness docs link check 与 line budget
  通过。当前有效行数：Core + Protocol `6903/7000`，Control Plane `41117/45000`，
  Release `59558/65000`。

## 后续

候选比例已按模型 profile 窗口大小在 Core 落地，与 provider/model ID 无关。下一步需要确定
262,145–999,999 窗口的策略，并通过更多多轮 Scenario 校准响应后反馈水位、输出预留和
摘要效果。见 [Token 水位驱动的上下文压缩与字节上限解耦](../../proposed/architecture/2026-10-05-token-aware-context-compaction-waterline.zh.md)。
