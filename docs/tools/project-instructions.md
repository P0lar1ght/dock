# 工作区工程规约注入

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `project-instructions`

- **ctx**：→ 挂 `agent/step-start`
- **模型工具**：—

把工作区工程规约读进**历史尾部 `<system-reminder>`**，**不进系统提示**：`~/.dock/AGENTS.md`（用户层）+ `{cwd}/AGENTS.md`（项目层，追加不覆盖）。`AGENTS.md` 是仓库内容（clone 陌生的仓库，那份文件就是陌生人写的），系统提示是 harness 自己的声音，所以它降级成「工作区提供的数据」：注入前按 Grok `neutralize_reminder_tags` 中和内容里的 `<system-reminder>` 变体（大小写 / 带斜杠 / 带空格都算），来源标注（`## AGENTS.md`）由 harness 写、不被文件内容顶掉。槽位 `ORDER_STEP_START_INSTRUCTIONS = 5`，排在所有提醒最前——它是这一步的规则，其余提醒是在这些规则之下的催办。**一条判据覆盖四种情况**：渲染出来的规约与本会话历史里最近一份不一致才注入，认 `<system-reminder>` + `# 工作区工程规约` 开头（会话开始 / 中途改文件 / 压缩吃掉旧副本 / `/resume` 回放旧版）。只追加、不原地改写，前缀一个字节不动。预算窗口 token ×4 ×5%，**只管文件内容**（框架行必须完整，否则副本认不出来、标签闭合不上），超出从尾部按字符边界截断并附说明。**只读工作区根目录**，子目录嵌套 `AGENTS.md` 不读（优先级与预算都不好界定，要看用 `read_file`）。文件缺失 / 读失败 / 全空白都 fail-open（插件仍 Active，不贡献提醒）。子代理跑隔离 ctx，按**它自己的**历史与窗口判断该不该注（handler 拿不到调用方 ctx，走循环挂的 task-local）。`/context` 单列一行 **「工程规约」**，数是**实际注入的那份**，不是磁盘当前内容
