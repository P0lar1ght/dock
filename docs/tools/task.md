# 子代理

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-task`

- **ctx**：`"subagents"` + `"tools"`
- **模型工具**：`task` `send_message` `list_agents` `interrupt_agent`

Grok `ChannelBackend` + coordinator actor；dock `ChildRunner` isolate `"sessions"`+`"turn"`+`"agentPresets"`。提供 named `"subagents"`。**只有一个 spawn 工具**：`task`。`subagent_type` = 当前模式 `agents/<id>.yml` 角色 id（模型侧参数 enum 即这份名册）。参数只有 prompt / description / subagent_type / run_in_background / resume_from / reload_roster；`cwd` / `isolation` / `model` 已删（dock 不实现，别再加回来当摆设）。子代理**每轮结束**都会 park idle 并把回合结束推到父级（`<system-reminder>`，本轮没给父级发过消息时带上回合正文）；前台 spawn 内联拿到结果时会消费掉自己那条通知。

**子代理不是作业**：`job` / `kill_task` 只看后台命令，列不出、处置不了子代理。子代理只走 `send_message` / `list_agents` / `interrupt_agent`，结果靠回合结束通知送达，不轮询。整个撤掉一个子代理是用户的事（TUI `/tasks` 里 `x`，走 `Subagents::kill`），或随会话 Stop / 结束。

**`send_message({agent_id, message})` 方向中立**：父子拿同一份定义（`report` 已并入）。服务层按调用方会话身份（`Tools::execute_on` 递下来的 exec ctx）授权**相邻的一条边**：父（`main` / `main#2`）只能发给自己启动的直接子，子只能发给启动它的会话（槽位上记的 `parent`）；兄弟、别的分页、自己一律拒绝。没有优先级参数：目标在跑就在它下一步边界读到（`drain_parent_mailbox` 在子代理的每一步开头取走队列，包成 `Agent <parent> sent a message:`），idle 就开下一轮。子→父的消息进父信箱；workflow 的孩子进 run 自己的队列（见 [workflow](workflow.md)）。`list_agents` / `interrupt_agent` 同样只看调用方自己的孩子。父信箱按会话分账（见下「父信箱按页」）。旧参数名 `subagent_id` 继续收。TUI 框底输入是用户在说话，不走相邻授权，仍用 urgent（打断本轮插话）。

`send_message` 在能力档位里归元工具（所有档位放行）：只读子代理也得能回话；受限子代理派不出孙代理，相邻授权又挡住了兄弟之间互发，所以放开是安全的。用户自己写的 `agents/<type>.yml` 里还列着 `report` 的，按 `send_message` 的别名放行。

**「做完要回报」写进子代理的初始任务**（`format::append_reply_instruction`，带 JSON 编码的父级 id），不进人设、不进工具描述、也不再每轮追加提醒：人设和工具排在请求头里，子代理专属的一段会让它的请求头和父级分叉；工具描述只在模型已经想到那颗工具时才起作用。角色的工具集里没有 `send_message` 时不写这段。

**父信箱按页**：子代理的回话与回合结束通知记在启动它的会话（`ChildSlot::parent`，`main` / `main#N`）名下。
- 每页的循环只取自己的（`Subagents::has_parent_notices(page)` / `drain_parent_notices(page)`）。
- 唤醒也按页（`parent_wake(page)`）：共用一个 `Notify` 时许可会被别的页吃掉。
- 分页派的孩子不再落到第 1 页，第 1 页也不会拿别页孩子的回报开一轮。
- workflow 的收尾通知仍交给 drain 循环所在的第 1 页（workflow 还没按页挂）。
- 页关掉（会话 actor dispose）时 `forget_parent`：丢掉那一页的唤醒和送不到的通知。

**给宿主的事件**：子代理的会话不发 `session/event`，改发 `session/child-event`（`ChildLogEvent`）；
出现或状态变了发 `subagent/changed`（`SubagentChanged { id }`）。
`SubagentSnap` 带：
- `parent`（启动它的会话身份）、`tool_call_id`（派它的 `task` 调用）；
- `failed`（最近一轮以失败收尾，含模型请求出错）、`settled_at`（停下的时刻，`elapsed()` 算到这里）。
网关据此投影 `subagent/*`（见 `cordis-gateway/README.md`「子代理」）。

id 参数一族一个名字：子代理这族（`send_message` / `interrupt_agent`，以及 `task` 返回的 `agent_id:`）叫 `agent_id`，作业那族叫 `job_id` / `job_ids`。parent session id = `session::ROOT_IDENTITY`，所以用户 Stop / 新会话会取消本会话子代理，下一次 prompt 重新开放准入
