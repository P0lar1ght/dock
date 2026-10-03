# 目标

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-goal`

- **ctx**：`"goal"` + `"tools"`
- **模型工具**：`update_goal`（按需）

Grok oneshot ack + drain。`objective` 可在无 `/goal` 时由模型自己开目标；无目标且只有 message/completed 时仍 `HarnessDisabled`。进度卡在滚动区。注册 `agent/turn-end`（`ORDER_TURN_END_GOAL = 20`）：目标没 completed 就续跑，最多 64 轮，用户已排队下一条时让路；`LoopHandle::continue_goal`（GoalSummary 入口）复用同一份 `continuation_reminder`，不走 waterfall。`register_deferred`

`"goal"` 服务还记两样给客户端显示（网关投影进 `environment.goal`）：

- 完成：`update_goal(completed)` 停跑但留着标题，`completed()` 为真；
  完成时附带的 `message` 记成最后一条进展；下一次 `start` / `clear` 复位。
- 计时：`elapsed()` 只算在跑的时段，暂停那段不算；完成后停表。
