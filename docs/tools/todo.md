# 待办

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-todo`

- **ctx**：`"todos"` + `"tools"`
- **模型工具**：`todo_write`

Grok merge/replace，含 Grok `effective_merge` 自动升级：`merge:false` 但每一项都只带已存在 id + status 时按合并处理，不会把 content 抹成 id。工具结果在列表后追加 `n/total done` 与状态提示（没有 in_progress / 多于一个 in_progress / 全部关闭）。`Todos` 另外暴露 `stats()`（TUI 折叠条）/ `revision()` / `gate_reminder()`。注册两条 waterfall：`agent/turn-end`（`ORDER_TURN_END_TODO = 10`）出文本收尾但还有未完成项时续跑一轮，每条用户消息最多 2 次（`keep_working_with` 只在真的胜出时扣），配额在主会话的 `agent/pre-step` 清零，被 live job / running subagent 托底的 `in_progress` 不算；`agent/step-start`（`ORDER_STEP_START_TODO = 10`）列表连续 6 步没动且仍有未完成项时中途提醒勾选 / 调整，每轮最多 3 次，watchdog 在 `step == 0` 重置。**注意 `"todos"` 不随子代理 isolate**，两条都靠载荷里的 `identity` 判断，只在主会话生效
