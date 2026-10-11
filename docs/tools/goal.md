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
- 请求数：`requests()` 目标在跑时本页每次采样记一次（`agent/step-start`），子代理的不算。

## 跑多久（`pace.rs`）

不设总上限：复杂、持续迭代的任务要能一直跑，不该隔一阵就要人来点继续。
整轮结束后会话 actor 还会再排一次 GoalSummary，所以 64 轮只是一轮的上限。
只在三种情况下停：

- **空转**：连续 `STALL_LIMIT`（3）轮都没调干活的工具（首轮加催了 2 次），就在第 3 次续跑前自动暂停。
  `update_goal` / `todo_write` 只算报进度，不算干活。
  只看工具名：报错或被权限拒掉的调用也算干活（失败重试多半是真在调试）；
  反复撞同一堵墙靠自查提醒里的 `blocked_reason` 收敛。
  原因写进进展（`status()`），`/goal resume` 或用户发消息后从头数。
  判断在续跑之前（turn-end 投票前、GoalSummary 注入前）。
  只有这一票真被采用时（`keep_working_with` 的 `on_win`）才结算这段有没有干活。
  暂停本身在投票前：若同一轮别的 handler（order 更小）赢了这一票，目标也已停下。
  只会提前停，不会多跑。
- **自己收敛**：每 `CHECKIN_EVERY`（50）次请求，`agent/step-start` 插一条 `<goal-checkin>` 提醒
  （`ORDER_STEP_START_GOAL`）。让模型写出能检查的完成标准、逐条对照，满足就 `update_goal(completed)`，
  卡住就 `blocked_reason`，别为了更完美无限打磨。
- **预算**：`/goal budget 2h` / `90m` / `300`（请求次数），`off` 取消，空参数看用量。
  默认不限；`start` / `clear` 复位。到了就暂停，并把预算清掉（恢复后不再限，要限再设）。
  只在续跑处检查，正在跑的这一轮做完才停。
