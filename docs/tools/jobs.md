# 后台任务与进程表

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `jobs`

- **ctx**：`"jobs"`
- **模型工具**：—

进程表。bash `is_background`（以及仍然接受、但不在 schema 里登记的 `block_until_ms: 0`）用它，**前台 bash 也在这张表上**（`foreground: true`，只为让 TUI 边跑边读输出；不进 tasks pane、不进 `job` 的无参列表，结束即摘掉）。前台预算到点时 `Jobs::detach` 把 `foreground` 翻成 `false`——**进程不动**，那条命令就此变成一条普通后台任务（进 tasks pane、`job` 查得到），详见 [workspace](workspace.md) 的 `bash`。stdout / stderr **并发抽干**——顺序读会在任一侧写满 64KB 管道缓冲时把子进程永久堵死。输出流式累积，内存里留头 4KB + 尾 16KB（共 20KB，对齐 Grok `output_byte_limit`），不超就原样返回。**超出预算时边跑边把完整输出落盘**到 `$DOCK_HOME/tool-output/<job-id>.txt`，这时给模型的（bash 结果、`job` 工具，走 `Jobs::model_snapshot`）只有开头 2KB 预览，停在行边界上，同 Claude Code。截断提示报全文字节数、行数，给出接着读的 `read_file offset=N`，以及读最后 50 行的 offset（结论多在结尾）（#174；原先内联头 4KB + 尾 16KB 只报字节，模型不知道看过的是第几行，只好从第 1 行重读）。落盘失败时退回内联完整的头 4KB + 尾 16KB；中途追加失败（磁盘满）同样放弃落盘、删掉残缺文件、退回头尾。落盘**只试一次**：跨过阈值那一刻没开成、或追加失败放弃之后，都不再重开——那时中间的字节已经丢了，重开只写得下头尾，是一份残缺的「全文」。给人看的（TUI 任务行、输出查看器，走 `snapshot` / `list`）不缩，仍是头 4KB + 尾 16KB：人要看最新进度和报错。落盘**必须在跨过阈值之前发生**：`push` 里的 `tail.drain` 是即时丢弃，命令结束或超时时中间字节早已不在内存里，那时再落盘只能落到已经截断过的那一份。未超预算的命令不建文件


每条任务记下**起它的那一页**（执行期 ctx 的会话身份：`main` / `main#N`，子代理是它的 id；不在哪一页的一轮里起的是 `None`）。
后台任务（含转入后台的前台命令、monitor）跑完时发 `job/finished`（载荷 `JobFinished`：归属、怎么结束的 `JobEnd`、跑了多久、输出结尾至多 2KB、总行数、落盘路径）；前台命令照常跑完不发——结果已经随那次调用交回。
`Jobs::detach` 与收尾互斥（`settle` 锁）：已经跑完的命令转不了后台（返回 `false`，调用方照常交回结果），否则一条刚转后台的命令会既不随调用交回、也收不到跑完通知。

## `tool-jobs`

- **ctx**：→ `"tools"`
- **模型工具**：`job`；`kill_task`（按需）

列 / 查 / 等后台 bash 与 `monitor`，以及杀。**子代理不是作业**：这两颗工具看不见子代理（它们走 `send_message` / `list_agents` / `interrupt_agent`，见 [task](task.md)）。

`job` 是查询面，一颗工具三种用法：省略 `job_ids` 列全部（一条一行，带已运行多久，**不带输出正文**——清点不该把每条最多 20KB 的输出全倒进上下文）；给 `job_ids` 读它们的输出；再给 `timeout_ms` 就等它们跑完。`wait_tasks` 已并入——它自己的描述本来就写着「Prefer get_task_output with a positive timeout_ms」，等于官方劝退。

`kill_task` **刻意留在外面**：dock 的权限门与计划门是 tool-level 的（`gated_builtin` 里有 `kill_task`、没有查询工具），并成一颗就只剩两条路——要么整颗进门、连「看看还有什么在跑」都弹权限窗且计划模式下不可用，要么 kill 失去权限门。分开正好让工具边界与权限边界重合。

`kill_task` 的参数叫 `job_id`（与 bash / monitor 返回的 `job_id:` 同名）。旧名 `task_ids` / `task_id` 继续认（`ID_KEYS`）：`/resume` 回来的历史里全是旧参数名，模型照抄是常态，认下来比回一句「参数名错了」便宜。

`timeout_ms` 是**本次调用愿意等多久**，不是任务寿命：等到任务完成、或等到点返回当前快照（`[running 12m04s]` + 已产出输出）——**不设上限、不中止任务**，长任务跑完会自动通知（见 `job-notify`），不必再查。等到点仍未完成时正文补一句「这是快照不是结论」，免得 `[running …]` 被当结果读掉。`timeout_ms: 0` 或省略 = 不等待的即时快照

`job` 的等待还会提前交回两种情况，正文都补一句「快照不是结论、跑完会通知你」：

- 用户插话了：和前台 bash 一样再给 2 秒（`STEER_GRACE`），还没跑完就交回，让插话尽快送达（以前要等满整段 timeout）。
- 用户停止了这一轮：立刻交回。

`job` 交回了某条任务**跑完**的结果，就记它「已交代」（`Jobs::mark_reported`）并发 `job/reported`：跑完通知不再为它叫醒这一页，已经进了信箱、还没送到的撤掉。只看了一眼还在跑的不算。`kill_task` 停的同样记「已交代」（模型自己停的，它知道）；已经跑完的不记，那条通知它还没看到。

## `job-notify`

- **ctx**：—（听 `job/finished` / `job/reported`，live `"subagents"` `"jobs"`）
- **模型工具**：—

后台任务跑完，主动告诉起它的那一页，同 Claude Code 的后台命令完成通知。
通知走**子代理信箱**（`Subagents::notify_job_done`），不另开通道：

- 那一页在跑：下一步开始、采样之前送到（`drain_parent_mailbox`），不夹在工具调用和结果中间。
- 那一页闲着：叫醒它开一轮（会话 actor 的信箱轮），不用用户再发话。

不叫醒的情况：

- 模型已经拿到结果（见上 `job` / `kill_task`）：不发；已经排进信箱的撤掉（`retract_job_notice`）。
- 被别处停掉的（任务面板）：送，但不叫醒，那一页下一次采样带上。
- 子代理起的：按子代理的身份进信箱，在它下一步带上；不叫醒（子代理只由父级叫醒），子代理收掉时一起丢掉。
- 没有归属的（不是在哪一页的一轮里起的）：不发。

同一页一秒内跑完的几条攒成一次送（`BATCH_WINDOW`），免得前后脚跑完的任务让那一页连开几轮。

正文（`render_notice`）写明：哪条、怎么结束的（`exit N` / 被信号终止 / 被停掉 / 没能启动）、跑了多久、命令和描述、输出结尾（至多 2KB，从一行开头起，报总行数），以及完整输出去哪读（落了盘给文件路径，否则 `job` 工具）。
开头一句说明这是后台任务的完成通知、不是用户的消息。

**不推进展**：普通后台命令只在退出时通知一次；要看进展用 `job` 读，或者用 `monitor`。

## `tool-monitor`

- **ctx**：→ `"tools"`（live `"jobs"`）
- **模型工具**：`monitor`（按需）

长命令 stdout 盯梢。按需由预设 `on_demand_tools` 决定
