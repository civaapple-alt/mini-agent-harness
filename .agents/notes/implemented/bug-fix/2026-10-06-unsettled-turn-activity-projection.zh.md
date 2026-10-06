# 重启后的未结 Turn 活动投影

状态：implemented；日期：2026-10-06；范围：App Server、Python SDK、Gateway、Web Studio

## 问题

App Server 在工具调用期间重启后，事件回放文件只有有界事件元数据，无法单独还原思考文本、助手消息和工具活动。Studio 因而可能只显示检查点和一张待核对卡，用户看不到调用前后的执行过程。

## 决策

`turn/read` 从最近的执行检查点和待处理工具批次恢复一个有界活动投影，最多重建 256 项，包括当前 Turn 输入、思考、助手消息、已完成的工具结果和未决调用。它只补足显示需要的执行轨迹，不修改 `thread/items/list` 的规范历史和游标。若规范 ThreadItems 已包含活动，Web 优先使用它们，避免重复呈现。

工具结果仍由 App Server 执行日志持有。调用期间重启而无法确认副作用时，恢复必须保留待核对状态；用户需确认真实结果后，才能继续原 Turn。显示检查点不会自动重跑命令，也不等于确认工具成功。

## 验证证据

- Harness 提交 `ea88d7a` 增加恢复活动投影；App Server 单测与 JSON-RPC 场景覆盖检查点读取和有界活动内容。
- Web Studio 提交 `3c034f2` 恢复会话流中的活动，`docs/session-history.md` 记录合并顺序和 256 项上限。
- 相关测试位于 App Server `tests.rs`、`json_rpc_tests.rs` 以及 Web 的 `ChatArea.test.jsx` 和 `message_state.test.js`。本文补录没有重跑这些套件。

## 后续

若检查点本身缺少活动正文，回放元数据不能补出这些正文。此时界面应保留明确的恢复限制和工具核对入口，不应虚构缺失的会话流。
