# 后台任务与进程表

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `jobs`

- **ctx**：`"jobs"`
- **模型工具**：—

进程表。bash `is_background` / `block_until_ms: 0` 用它，**前台 bash 也在这张表上**（`foreground: true`，只为让 TUI 边跑边读输出；不进 tasks pane、不进 `get_task_output` 的无参列表，结束即摘掉）。前台预算到点时 `Jobs::detach` 把 `foreground` 翻成 `false`——**进程不动**，那条命令就此变成一条普通后台任务（进 tasks pane、`get_task_output` 查得到），详见 [workspace](workspace.md) 的 `bash`。stdout / stderr **并发抽干**——顺序读会在任一侧写满 64KB 管道缓冲时把子进程永久堵死。输出流式累积，上限 20KB（头 4KB + 尾 16KB，中间截断并在正文标明省略字节数），对齐 Grok `output_byte_limit`。**超出预算时边跑边把完整输出落盘**到 `$DOCK_HOME/tool-output/<job-id>.txt`，截断提示给出路径（可用 `read_file` / `grep` 取回省略的那段）。落盘**必须在跨过阈值之前发生**：`push` 里的 `tail.drain` 是即时丢弃，命令结束或超时时中间字节早已不在内存里，那时再落盘只能落到已经截断过的那一份。未超预算的命令不建文件

## `tool-jobs`

- **ctx**：→ `"tools"`
- **模型工具**：`get_task_output` `wait_tasks` `kill_task`

查/等/杀后台 bash **或子代理**（同一 id 空间）。`timeout_ms` 是**本次调用愿意等多久**，不是任务寿命：等到任务完成、或等到点返回当前快照（`[running]` + 已产出输出）——**不设上限、不中止任务**，长任务下次调用接着查。等到点仍未完成时正文补一句「这是快照不是结论」，免得 `[running]` 被当结果读掉；只有仍在跑的是子代理才加「等它推回合结束」那半句（bash / monitor 不推通知）。`get_task_output` 的 `timeout_ms: 0` 或省略 = 不等待的即时快照，`wait_tasks` 省略时默认等 30s

## `tool-monitor`

- **ctx**：→ `"tools"`（live `"jobs"`）
- **模型工具**：`monitor`（按需）

长命令 stdout 盯梢。`register_deferred`
