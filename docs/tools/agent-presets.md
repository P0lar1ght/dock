# Agent 预设与名册

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `agent-presets`

- **ctx**：`"agentPresets"`
- **模型工具**：—

组装 Agent：YAML 定义人设 + 工具允许名单，运行时只过滤 live `"tools"`。**正在运行的动态包**用 `Tools::register_dynamic` 登记的 extra 工具、以及 **已开启的 MCP 工具**（`register_mcp`，公名 `mcp_{server}__{tool}`，经 `use_tool` 调度）会穿过允许名单。层：crate `presets/<id>/agent.yml` + `agents/*.yml` < `~/.dock/presets` < 项目 `.dock/presets`。仍可读旧 `<id>.yml`（目录优先）。加一个目录就是一个 Agent。内置 `code` / `minimal` / `cordis` / `warden`（守望）。`code`/`cordis` 的 `agents/` 名册是 `general-purpose` / `explore` / `plan`（项目层可加，如 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`）；`warden` 是 `岑` `锁` `甲` `乙` `丙` `衡` `验` `观` `突击`（不要用拼音 id）。发给模型的 `task` 把 `subagent_type` 收成当前名册 enum，并在 description 尾部追加 `subagent_role_hint()`（角色 id + 显示名 + **角色说明** + 写路径 + `reload_roster` 用法）。**系统提示不再有名册段**：它原先是 `task` / `send_message` / `report` / `interrupt_agent` description 的中文重写，逐条重复，已整体删除（`ORDER_ROSTER` 一并撤掉）。省略 `tools` = 全部已注册工具。**工具表是 allowlist，列不存在的名字不报错**——只是静默地不给，所以一颗工具改名会让装过的老预设两头落空（旧名没了、新名不在表里），整条链上没有一处会出声。`RENAMED_TOOLS` 就是为此存在：加载时把旧名归一化成新名（当前只有 `get_task_output` / `wait_tasks` → `job`），**只改内存不回写 yml**（`persist_preset` 是整份序列化，会抹掉用户写的注释）。改工具名时记得往这张表里加一行。新建模式默认写**当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` n/d 有项目层时落到这里；`task` description 注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`；id 必须 `[a-z0-9][a-z0-9-]*`，汉字目录只叠内置）。新建子代理默认写 `.dock/presets/<当前模式 id>/agents/<type>.yml`。空名册仍注入这两处路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后 `task` 校验立刻重读；本轮刚写完时用 `task`（`reload_roster: true`）刷新 enum。新建模式写完后用 `/preset` 应用该 id。改 crate `presets/` 要重新编译
