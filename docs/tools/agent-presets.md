# Agent 预设与名册

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `agent-presets`

- **ctx**：`"agentPresets"`
- **模型工具**：—

人设追加进系统提示时包在 `<agent-preset>` 里，不再写 `# Agent 预设：名字`。`minimal` 的 `replace_prompt` 关闭，base 仍在。

组装 Agent：YAML 定义人设 + 工具允许名单，运行时只过滤 live `"tools"`。**正在运行的动态包**用 `Tools::register_dynamic` 登记的 extra 工具、以及 **已开启的 MCP 工具**（`register_mcp`，公名 `mcp_{server}__{tool}`，经 `use_tool` 调度）会穿过允许名单。层：crate `presets/<id>/agent.yml` + `agents/*.yml` < `~/.dock/presets` < 项目 `.dock/presets`。仍可读旧 `<id>.yml`（目录优先）。加一个目录就是一个 Agent。内置 `code` / `minimal` / `cordis` / `warden`（守望）。`code`/`cordis` 的 `agents/` 名册是 `general-purpose` / `explore` / `plan`（项目层可加，如 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`）；`warden` 是 `岑` `锁` `甲` `乙` `丙` `衡` `验` `观` `突击`（不要用拼音 id）。发给模型的 `task` 把 `subagent_type` 收成当前名册 enum。名册本身（角色 id + 显示名 + **角色说明** + 写路径 + `reload_roster` 用法）走历史 `<system-reminder>`（`agent/step-start` 槽位 `ORDER_STEP_START_ROSTER=13`，`task` 这一页叫得动才发），工具 description 保持静态。**系统提示不再有名册段**（`ORDER_ROSTER` 已撤掉）。省略 `tools` = 全部已注册工具。`agent.yml` 可选 `icon`（客户端图标名，lucide 名如 `rocket`；TUI 不用）。新建模式默认写**当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` n/d 有项目层时落到这里；名册 reminder 注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`；id 必须 `[a-z0-9][a-z0-9-]*`，汉字目录只叠内置）。新建子代理默认写 `.dock/presets/<当前模式 id>/agents/<type>.yml`。空名册仍注入这两处路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后 `task` 校验立刻重读；本轮刚写完时用 `task`（`reload_roster: true`）刷新 enum。新建模式写完后用 `/preset` 应用该 id。改 crate `presets/` 要重新编译

### 按需工具（`on_demand_tools`）

- 哪些工具常驻、哪些按需，**只看预设 YAML**；改它不用改代码。
- 预设 `agent.yml` 与子代理 `agents/<id>.yml` 都可以写，写法同 `resident_tools`：

  ```yaml
  on_demand_tools:
    - canvas_*                # 以 * 结尾 = 前缀
    - workflow                # 全名
  ```

- 作用：列在这里的工具不进模型工具表，模型只在 `search_tool` 描述末尾的
  「Built-in on-demand tools: …」里看到名字，用时按精确名搜一次拿 schema，再经 `use_tool` 调。
  - 名单按**这一页**实际藏起来、又在允许名单里的本地工具生成（`Tools::specs_for_model_on`）。
- 不写 / `[]` = 全部常驻（MCP 与动态包照旧藏着）。
- 子代理角色不写就继承所属预设的（`task` 开子代理时补上）。
- 用户 / 项目层覆盖内置预设而没写这一项：沿用内置的名单。要全部常驻写 `[]`。
- 新建预设（GUI「新建」、TUI `n`）从 `code` 的名单抄一份写进自己的 YAML，
  另补 `code` 不准的 `scheduler_*` / `cordis_*`（新预设什么工具都准）。
- 允许名单外的工具写进来也叫不动：不藏、不列、`search_tool` 也不搜出来。
- `resident_tools` 优先：两边都命中时常驻。
- 守卫（代码里，不可配）：
  - `search_tool` / `use_tool` 不能写进来（`search_*`、`use_*` 这种会盖住它们的前缀也不行）。
  - 这一页的工具表上没有 `use_tool`（只读档子代理、`minimal`、`/btw`）时名单不生效：藏起来就等于拿走。
  - 坏掉的预设不藏任何东西。
- 内置：`code` / `cordis` 写了低频工具（canvas_* / workflow / kill_task / list_agents / interrupt_agent /
  monitor / update_goal / lsp；`cordis` 另有 cordis_*）；`warden` 不写，它的调度工具全部常驻。
- 判断标准：低频，而且不是只读子代理离不开的（`web_*`、`memory_*`、`send_message` 留常驻）。
- 核对：`cargo test -p cordis-spine --test round -- search_tool_names_every_builtin_on_demand_tool`。

### 常驻工具（`resident_tools`）

- 预设 `agent.yml` 与子代理 `agents/<id>.yml` 都可以写：

  ```yaml
  resident_tools:
    - mcp_browser__*          # 以 * 结尾 = 前缀
    - lsp                     # 全名
  ```

- 作用：本来藏在 `search_tool` 后面的工具直接进模型工具表（`specs_for_model`）。
  - 覆盖 MCP、按需本地工具（`on_demand_tools` 或第三方 `register_deferred`）、运行中的动态包。
- MCP 与动态包本来就绕过允许名单，命中就上表。
- 按需本地工具仍要在 `tools` 允许名单里，常驻不会把名单外的工具带进来。
- 子代理能力档位照样挡（只读档看得到 `mcp_browser__browser_snapshot`，看不到 `…_click`）。
- 坏掉的预设不常驻任何东西。
- 不收单独的 `*`（等于把整个按需目录塞回工具表），也不收中间带 `*` 的。
- 代价：常驻的工具每轮都随工具表发送；MCP 连上 / 断开时工具表会变。
- TUI `/preset` 画布暂不编辑它；改 yml 或走网关 `preset/update`。

### 常驻角色（`keep_alive`）

- 子代理 `agents/<id>.yml` 可以写：

  ```yaml
  keep_alive: true
  ```

- 作用：这个角色空闲下来也不参与回收清扫。普通角色跑完一轮 park idle，**空闲满 15 分钟**
  就被回收（`list_agents` 不再列它，`send_message` 报 `is done`）；`keep_alive: true`
  的角色整场会话都留着，可以一直接着聊。
- 观察者一类的角色要的就是这个：内置 `warden` 的 `观` 默认开着。
- 写错了也不要紧：被回收的角色还能用 `task`（`resume_from: "<agent_id>"`）从盘上的对话
  接着聊——复活后这份 `keep_alive` 仍然生效。
- **派活时定下**：改了预设只对**之后派出的**子代理生效，已经在跑的不跟着变。
- 同一场会话里要复用同一个常驻角色（观察者），用 `send_message` 接着聊，不要反复
  `task` 派新的——每次派都会多一个常驻实例。
- **有上限**：同一页最多留 8 个常驻孩子。常驻孩子不参与回收清扫，不封顶会一直涨；
  超过上限的那一个按普通孩子算（照常 15 分钟后回收）。
- **没写这一项就沿用下层**：内置角色的用户层覆盖文件里没写 `keep_alive`（例如升级前
  落盘的那份），不会把内置的常驻标记悄悄抹掉；写 `keep_alive: false` 才是显式不要。

### GUI 预设编辑器

- 经网关 `preset/get` / `preset/update` / `tool/catalog` 编辑同一批文件。
- `AgentPresets::update` 整份写下。
- 内置预设写成用户层覆盖。
- 损坏的预设整份重写。
- AI 起草：`agent::preset_assist`（`draft_preset` / `rewrite_persona` / `suggest_tools`）。
- 起草只采样一次、不写盘；模型输出按工具目录和 id 规矩过滤后才回。
- 侧聊：`ask_preset`（网关 `preset/ask`）。无状态，客户端带之前的问答、当前草稿和所在步骤。
  - 只回参考答案（Markdown），不改草稿、不写盘、不给工具。
  - 历史只带最近 20 条；草稿 ≤32KB；`step` 是 basic / prompt / tools / agents。

### `propose_preset`（会话里起草预设）

- 用户说「帮我做个 xx agent」时模型调它：只起草，用户在编辑器里逐项审、自己保存。
- 参数同 `preset/draft` 的模型输出（`name` `description` `persona` `tools[]` `subagents[]`），走同一个 `parse_draft` 校验。
- 不写盘。回 JSON：`status` + `note` + `draft`。
  - 网关在监听（有图形界面）：`awaiting_review`，桌面端把这次调用画成草稿卡。
  - 否则（终端）：`no_reviewer`，`note` 让模型照旧写 `.dock/presets/<id>/agent.yml`。
- 低频：`code` / `cordis` 都写在 `on_demand_tools`；`task` 的名册提示点了它的名字。
