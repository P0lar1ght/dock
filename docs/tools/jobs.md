# 后台任务与进程表

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `jobs`

- **ctx**：`"jobs"`
- **模型工具**：—

进程表。bash `is_background` / `block_until_ms: 0` 用它，**前台 bash 也在这张表上**（`foreground: true`，只为让 TUI 边跑边读输出；不进 tasks pane、不进 `job` 的无参列表，结束即摘掉）。前台预算到点时 `Jobs::detach` 把 `foreground` 翻成 `false`——**进程不动**，那条命令就此变成一条普通后台任务（进 tasks pane、`job` 查得到），详见 [workspace](workspace.md) 的 `bash`。stdout / stderr **并发抽干**——顺序读会在任一侧写满 64KB 管道缓冲时把子进程永久堵死。输出流式累积，上限 20KB（头 4KB + 尾 16KB，中间截断并在正文标明省略字节数），对齐 Grok `output_byte_limit`。**超出预算时边跑边把完整输出落盘**到 `$DOCK_HOME/tool-output/<job-id>.txt`，截断提示给出路径（可用 `read_file` / `grep` 取回省略的那段）。落盘**必须在跨过阈值之前发生**：`push` 里的 `tail.drain` 是即时丢弃，命令结束或超时时中间字节早已不在内存里，那时再落盘只能落到已经截断过的那一份。未超预算的命令不建文件

## `tool-jobs`

- **ctx**：→ `"tools"`
- **模型工具**：`job` `kill_task`

列 / 查 / 等后台 bash **或子代理**（同一 id 空间），以及杀。

`job` 是查询面，一颗工具三种用法：省略 `job_ids` 列全部（一条一行，带已运行多久，**不带输出正文**——清点不该把每条最多 20KB 的输出全倒进上下文）；给 `job_ids` 读它们的输出；再给 `timeout_ms` 就等它们跑完。`wait_tasks` 已并入——它自己的描述本来就写着「Prefer get_task_output with a positive timeout_ms」，等于官方劝退。

`kill_task` **刻意留在外面**：dock 的权限门与计划门是 tool-level 的（`gated_builtin` 里有 `kill_task`、没有查询工具），并成一颗就只剩两条路——要么整颗进门、连「看看还有什么在跑」都弹权限窗且计划模式下不可用，要么 kill 失去权限门。分开正好让工具边界与权限边界重合。

旧名 `task_ids` / `task_id` 继续认（`ID_KEYS`）：`/resume` 回来的历史里全是旧参数名，模型照抄是常态，认下来比回一句「参数名错了」便宜。

`timeout_ms` 是**本次调用愿意等多久**，不是任务寿命：等到任务完成、或等到点返回当前快照（`[running 12m04s]` + 已产出输出）——**不设上限、不中止任务**，长任务下次调用接着查。等到点仍未完成时正文补一句「这是快照不是结论」，免得 `[running …]` 被当结果读掉；只有仍在跑的是子代理才加「等它推回合结束」那半句（bash / monitor 不推通知）。`timeout_ms: 0` 或省略 = 不等待的即时快照

## `tool-monitor`

- **ctx**：→ `"tools"`（live `"jobs"`）
- **模型工具**：`monitor`（按需）

长命令 stdout 盯梢。`register_deferred`
