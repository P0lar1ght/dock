# 子代理

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-task`

- **ctx**：`"subagents"` + `"tools"`
- **模型工具**：`task` `send_message` `list_agents` `interrupt_agent` `report`

Grok `ChannelBackend` + coordinator actor；dock `ChildRunner` isolate `"sessions"`+`"turn"`+`"agentPresets"`。提供 named `"subagents"`。**只有一个 spawn 工具**：`task`。`subagent_type` = 当前模式 `agents/<id>.yml` 角色 id（模型侧参数 enum 即这份名册）。参数只有 prompt / description / subagent_type / run_in_background / resume_from / reload_roster；`cwd` / `isolation` / `model` 已删（dock 不实现，别再加回来当摆设）。子代理**每轮结束**都会 park idle 并把回合结束推到父级（`<system-reminder>`，未 `report` 时带上回合正文）——后台 spawn 不必轮询 `get_task_output`；前台 spawn 内联拿到结果时会消费掉自己那条通知。`send_message`：idle 时 queued 与 urgent 都立刻开下一轮并在返回前把状态打成 running；urgent 只在 running 时才是 send-now。`report` 是子代理和主代理的多轮通道（同轮可多次）。`kill_task` 对子代理有效（dispose），只想停本轮用 `interrupt_agent`。parent session id = `session::ROOT_IDENTITY`，所以用户 Stop / 新会话会取消本会话子代理，下一次 prompt 重新开放准入
