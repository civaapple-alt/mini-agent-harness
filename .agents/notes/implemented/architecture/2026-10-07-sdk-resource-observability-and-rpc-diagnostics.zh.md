# SDK 资源观测与 App Server JSON-RPC 诊断

状态：implemented；日期：2026-10-07；范围：Python SDK、App Server JSON-RPC stdio

## 决策

MiniAgentClient 为 Gateway 提供受管 App Server 的 PID 和只读 JSON-RPC 聚合指标。SDK 按方法统计请求/通知数量、字节、成功/错误/超时、待处理请求数、延迟分位数及序列化、读写、解析、通知分发耗时。方法维度最多保留 64 项，超出的名称归入 other；只存聚合数值，不保存请求或响应正文。

优雅停止返回是否已确认子进程退出。调用方请求非强制停止且超时未确认时，Client 继续持有关联进程，避免在旧进程仍可能写入 Session 时启动第二个 App Server。

App Server 的 JSON-RPC 阶段诊断默认关闭。设置 MINI_AGENT_JSON_RPC_DIAGNOSTICS=1 后，stderr 输出方法名、读取、解析、分发、队列等待、序列化、写入耗时和字节数，不输出协议正文。协议维持 JSON-RPC V2，不新增公开 RPC 方法。

## 边界

SDK 指标只用于 Gateway 的观测和资源呈现，不拥有 Thread、Turn 或 Session 状态。SessionStore 和 App Server 仍是执行与持久化状态的权威来源。进程采样、查看租约和 Park/Wake 编排由 sibling mini-agent-web 的 Gateway 管理；Studio 只消费 Gateway 投影。

## 验证证据

- Python SDK Ruff 检查通过，SDK 测试 106 项通过；覆盖指标固定上限、无正文留存、阶段统计、分位数以及未确认停止时保持原进程关联。
- mini-agent-app-server 的格式检查、Clippy 和包测试通过；库测试 103 项、二进制测试 1 项通过。Rust 行数门禁通过。
- JSON-RPC 诊断开关的本地空闲进程测量和阶段样本记录在 sibling mini-agent-web 的 docs/resource-manager.md。测量用于确认诊断开销及定位阶段耗时，不代表模型 Turn 或生产环境性能。
- 未运行 Rust 全工作区测试，未调用真实模型供应商；本次没有变更 JSON-RPC 协议。
