# 上下文字节硬限制与模型快照

状态：已实现（token 水位部分仍在提案中）；日期：2026-10-05；范围：Core、Protocol、Host、App Server、Python SDK、Web Studio

## 结果

移除 Harness 将 `max_context_bytes / 2` 当作日常主动压缩水位的行为。默认
64 MiB 现在只限制序列化请求大小：超过上限时，在发送 provider 请求前返回本地
`ContextBytes` 限制错误。若 provider 明确报告所选模型的上下文窗口溢出，Harness
仍压缩历史并对该模型步骤重试一次；普通 provider 错误不触发这条恢复路径。

这次改动没有把字节数换算成 token，也没有实现请求前 token 估算和百分比压缩水位。
这避免以 64 MiB 的一半解释 Kimi 的 256K/1M 模型窗口，但还不能在 token 占用到某个
百分比时主动压缩。

## 每次请求的模型配置快照

为了让最近一次 provider 用量与实际请求的模型配置对应，本批增加了可选的
`ModelContextSnapshot`，记录 provider/model ID、配置的上下文窗口和最大输出 token：

1. Host 从本次请求选择或项目/全局默认中解析模型 profile。
2. Core 在请求前读取该 profile 快照；成功的 `model_responded` 事件携带可选
   `model_context` 字段。
3. App Server 将快照与最近一次 usage 一起持久化到
   `contextUsage.modelContext`。旧 Session 缺少此字段仍可读取。
4. Python SDK 解析并导出该字段，Web Studio 根据最近一次请求快照计算显示预算。

Studio 的“可用输入预算”按配置窗口减去配置的最大输出计算；如果没有输出上限，
则仅以配置窗口作为预算。如果没有模型快照，窗口显示为未知。UI 保留高于 100% 的
实际输入数值：超过配置窗口时提示核对请求路由和模型；超过扣除输出预留后的预算时
提示实际输出上限可能需要下调。这个比较是对最近实际 usage 的解释，不是下一次
请求的估算，也不会触发 Core 压缩。按上下文来源分摊 token 的详情仍是字节占比估算。

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
  且不调用模型；明确 provider overflow 时仍只压缩重试一次。
- App Server bounded scenario 覆盖大历史上下文、模型快照事件和输出的完整路径。
- Rust 包测试：Protocol、Core、Host、Capabilities、App Server 通过。
- 强杀恢复 fixture 改为等待 marker 成为完整 JSON，避免把文件创建瞬间误判为已写完。
- Python SDK 模型快照解析测试通过；SDK Ruff 通过。SDK 全量测试有一项不在本批改动范围的
  `test_read_loop_forwards_user_question_notifications_to_thread_stream` 失败：它期望
  普通 dict 通知，实际收到 `UserQuestionNotification` typed object，同时测试记录了
  stream closed runtime error。该失败与本次模型快照改动无关，未改其行为。
- Web Studio 的 Node/Vitest 定向测试、lint、build 通过；build 有 Vite bundle 大于
  500 kB 的提示。未做手动浏览器 smoke。
- `cargo fmt --all`、受影响 Rust 包 Clippy、Harness docs link check 与 line budget
  通过。当前有效行数：Core + Protocol `6797/7000`，Control Plane `40964/45000`，
  Release `59299/65000`。

## 后续

Token 水位提案仍保持 `proposed`。晋级前需要针对保留的 Kimi Code 产品确认可用、准确且
覆盖实际请求形状的请求前估算途径，并补齐估算误差、上下文边界、压缩迟滞和多轮
bounded Scenario 证据。见 [Token 水位驱动的上下文压缩与字节上限解耦](../../proposed/architecture/2026-10-05-token-aware-context-compaction-waterline.zh.md)。
