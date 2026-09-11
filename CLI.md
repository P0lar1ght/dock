# CLI / TUI

对照当前 `cordis-tui` 斜杠目录与 overlay，不是愿望列表。硬规则见 [AGENTS.md](AGENTS.md)，架构见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。**模型工具**记在 [TOOLS.md](TOOLS.md)；本文件只记人操作的命令、快捷键、overlay、底栏。

用户可见文案中文；Grok 底栏那种短 hint（`Enter:send`）保持英文无空格。

进程入口（`cargo run -p cordis-app -- …`）：`--resume` 恢复本 cwd 最近一次落盘会话，`--resume <id>` 指定 id，`--help` 看用法。对话在 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` + `chat_history.jsonl`），不是项目 `.dock/`。

---

## 斜杠

目录在 `cordis-tui/src/slash/mod.rs` `CATALOG`。别名也能提交。动态插件可以通过 `"slash"` **追加**命令（工厂 `slash` 或 Rhai `host.register_slash`），不能替换本表里的内建项。追加的命令出现在下拉补全和 `/help`；`prompt` 种会填入或发送模板（`{args}` 换成键入的参数），`overlay` 种打开只读标题+正文 overlay，`slot` 种打开已登记的 `tui.slots` id（`text` = 插槽 id），`tool` 种直接跑 live 工具（`text` = 工具名；参数：空=`{}`、以 `{` 开头=原始 JSON、否则 `{"args":"…"}`），结果进 Notice，不经模型。停掉该动态 Plugin 或退出进程后，**会话**额外命令消失；磁盘永久插件会在下次启动时再挂上。

浏览器 `embed-sdk` **不要**为每个斜杠单独适配。Gateway 暴露 `slash/list`（补全目录：TUI `slash_catalog()` + `"slash"` 追加 + 浏览器 `/screenshot*`）和 `slash/execute`（接到 spine / session.port）。JS 只做前缀过滤、截图采集、以及把 `{ kind: filled|notice|menu|capture|submitted|applied }` 画出来。TUI overlay 类命令（`/pair` `/theme` `/preset` `/settings` …）和 **`/cd`** execute 会回 Notice「请在 Dock 终端使用」——list 标 `terminal` 的名字不会改进程状态（`/settings timestamps` 也不会）。`/timestamps` `/think` `/model` `/effort` 仍可从浏览器直接改。未知 `/foo` 为 `passthrough`，当作普通 prompt 发给模型。`slash/execute` 的 `threadId` 只能是 `live` 或省略。

下拉打开时 Tab 把当前高亮项填进输入框（命令名阶段填 `/命令 `，参数阶段填 `/命令 参数 `）。Enter 只在**命令名**阶段填入；空格之后（参数下拉仍开着）Enter 发送并解析。无参数命令在名字后空格关闭下拉。有参数目录的命令（`/lsp` `/theme` `/model` `/effort` `/loop` `/settings`）空格后继续列出参数：`/lsp s` 可 Tab 到 `status` / `setup`，再 Enter 执行。

| 命令 | 行为 |
|---|---|
| `/settings`（`config` `prefs`） | 设置 overlay；有参数则直接改（`timestamps` / `theme` / `model`）。浏览器 companion 一律拒绝（含带参），请用 `/timestamps` `/think` `/model` `/effort` |
| `/new` | 归档当前会话并清空。账本用量一并清零。归档写入 `$DOCK_HOME/sessions/<cwd>/` |
| `/model` `/m` | 切换当前模型。目录 `[model.<id>].api_backend`（`chat_completions` / `responses` / `messages`）决定 `"llm"` 走哪条 HTTP 线；`api_model` 是发给上游的 slug（省略=目录 id）；`auth_scheme` 默认 Bearer，messages 默认 `x-api-key`（OpenRouter 的 /messages 要写 `bearer`） |
| `/resume` | 打开会话 picker，恢复本工作区已落盘的会话（进程重启后仍在）。用量账本不随归档恢复（Grok：新进程 resume 清零）。进程入口 `--resume` / `--resume <id>` 启动时直接恢复 |
| `/pair`（`pairing`） | 浏览器 Origin 配对：第一行开启/关闭回环网关（默认不监听；首选 `127.0.0.1:18991`，占用往上找，同端口再试 `[::1]`）。开启后待批请求可批准，已绑来源可撤销。overlay 显示实际监听地址。首次连接弹出「允许浏览器连接？」；Enter 批准 / `x` 拒绝或撤销（在网关行上 `x` 关闭监听）。浏览器 companion 的 list/execute 限制见上文。CORS 反射任意 Origin：鉴权靠配对 + 回环。Approved 的 poll **不**回 ticket 明文；`POST /v1/pairing/exchanges` 校验 TTL、一次性消费 |
| `/loop` `/cron` | 空命令在输入框留下用法（`用法: /loop [间隔] <提问>` + `/loop `）。有参数则用户气泡是 `/loop {参数}`，模型看到 `loop_schedule_instruction`（须 `scheduler_create`，`fire_immediately: true`，不要当场执行提问）。没有间隔就问用户，不要自己编。7 天后自动过期。查看 / 关闭：`/tasks` Watchers，`x` 或 `[✗]` |
| `/plan [说明]` | 开计划模式；无说明只切模式（Pending，发第一条 prompt 后变 Active）。有说明则 Active 并提交 |
| `/view-plan`（`show-plan` `plan-view`） | 查看 `.dock/plan.md`（打开时读一次，pretty markdown）；若 `exit_plan_mode` 正在等待批准则打开审批 chrome（`a` 批准 / `s` 修改 / `q` 放弃） |
| `/goal` | 输入框留下用法（`用法: /goal <目标>` + `/goal `），不会清空。再发送即为目标 |
| `/goal <目标>` | 开目标；用户气泡是目标文本；`goal_instruction` 走历史尾部 `<system-reminder>`（不改系统提示）。模型只回一句文本时不会结束目标：注入隐藏 continuation（Grok `Goal NOT complete`），继续采样直到 `update_goal(completed)` / 暂停 / 取消。整轮结束后若目标仍在进行，再塞一条隐藏 GoalSummary 开下一轮。模型也可用 `update_goal(objective)` 自己开目标 |
| `/goal status\|edit\|pause\|resume\|clear` | 打开目标 overlay / 暂停 / 继续 / 清除 |
| `/tasks` | Grok 分组 pane：Workflows → Subagents → Tasks → Watchers。子代理行显示当前模式名册的角色名（如守望下的「岑」而不是 `Cen`）。Enter / 点击子代理或后台任务打开 **Grok 同款全屏边框**（子代理：工具卡折叠循环 + 框底输入 `send_message`）。Esc 从全屏回到本列表。子代理第一轮结束后显示 **idle**（不是 done）；idle 不算 running。Watchers 里的 loop：`x` 或点 `[✗]` 关闭（`scheduler_delete` / `cron.cancel`） |
| `/workflow` / `/workflow runs` | Grok `Workflow Runs` overlay |
| `/workflow <name> [参数]` | 立刻用 `workflow` 工具按注册名启动（不经模型）。参数：纯文本 → `args.query`/`args.objective`；以 `{` 开头 → 原始 JSON。可选 `--agent-budget N`。`pause`/`resume`/`stop`/`save` 仍打开 overlay |
| `/deep-research <查询>` | 内置 Rhai 工作流 extra（`kind: tool` → `workflow`，`source.type=name`）。其它已发现工作流同样登记成 `/<name>`。不可盖 `RESERVED_SLASH`；与技能撞名时技能 extra 优先 |
| `/<工作流名> [参数]` | 项目 `.dock/workflows/<name>.rhai`、用户 `~/.dock/workflows/<name>.rhai`（以及 `~/.dock/bundled/workflows/`）里 `meta.name` 与文件名一致的脚本。不能覆盖内置 `deep-research` |
| `/mcps` | Grok 分组 pane：标题「MCP 服务器」、分组「本地 (N)」、徽章 `[就绪]` / `[需认证]` / `[不可用]` / `[已禁用]`、右侧 `(本地)`。Space 开关当前服务器或工具（写入 `config.toml`：`[mcp_servers.<name>].enabled` 与 `[disabled_mcp_tools.<server>]`），并立刻从 `"tools"` 注销/追加注册（MCP 不进 sampler 工具表，走 `search_tool` / `use_tool`；不重写系统提示；目录变化用服务器级 `<system-reminder>`）。`i` 对 HTTP 服务器打开浏览器 OAuth（PKCE，token 写 `~/.dock/mcp_credentials.json`，不是 grok.com 登录）；Enter 展开/收起工具（`N 个工具` / `N 个工具（M 个已启用）`）；Esc 关闭。stdio 或 Streamable HTTP。工具表跟 `tools/list` 翻页和 `tools/list_changed`；HTTP 跟 GET SSE，session 404 会重新握手 |
| `/lsp` | 探测 PATH 上的 `rust-analyzer` / `typescript-language-server` / `gopls` / `pyright-langserver`，按工作区标记（含子目录，跳过 `node_modules` / `target`）把缺的服务器写入 `.dock/lsp.json`，不覆盖已有条目。Notice 显示保留 / 添加 / 跳过。当前会话会尝试启动新服务器。空命令或 `/lsp setup` 写项目配置；`/lsp status` 只看不写；`/lsp user` 把 PATH 上有的默认服务器写入 `~/.dock/lsp.json`（不要求工作区标记）。输入 `/lsp ` 后 Tab / Enter 补全 `status` / `setup` / `user`（`/lsp s` 可滤到 status）。浏览器 companion 拒绝（请在 Dock 终端用） |
| `/browser` | 额外命令（`"slash"` extras，不是 CATALOG）：完整 TUI 驾驶舱 overlay（live-lookup `"browser"`）。显示未连接/已连接、**显示（有头/无头）**、标签页、截图路径、wait/对话框、最近 evaluate/network、P0–P2 能力摘要、审批提示；不渲染网页。按 **`h`** 切换有头/无头（写入 `config.toml` `[browser].headed`，默认无头）；**不**重启已开 Chromium，需 `browser_close` 后再 `browser_open`。`DOCK_BROWSER_HEADED`（任意非空）覆盖为有头。BUA 由 chromiumoxide CDP 驱动；`browser_open`=会话+首 URL，`navigate`/`navigate_back`=同会话跳转；P1 drag/dialog/upload/resize；P2 `browser_evaluate`（**与 bash 同级 permissions 门** + 计划模式挡）/`console_messages`/`network_requests`、同域 iframe（`frame`/`frame_selector`，跨域 clear error）。`browser_*` 仍 `register_deferred`（不进 sampler / `specs_for_model`），走 `search_tool` / `use_tool` |
| `/computer` | 额外命令（同 PR 薄驾驶舱，Cordis/TUI）：本机 **cua-driver** MCP 状态（已连接 / 未装 driver）、最近审批提示；**不**嵌真桌面。桌面键鼠走 MCP 公名 `mcp_cua-driver__*`（`search_tool` / `use_tool`），**全部**与 bash 同级 permissions / 计划门。安装与 `config.toml` 样例见 [TOOLS.md](TOOLS.md)「Computer / CUA」与 [config.toml.example](config.toml.example)。勿与 Dock `browser_*`（chromiumoxide）混淆 |
| `/cordis`（`plugins`） | Notice「Cordis 插件」：磁盘永久层（项目 `.dock/plugins/<id>/` 覆盖用户 `~/.dock/plugins/<id>/`）和本会话内存插件。Esc 关闭。写成永久用模型工具 `cordis_promote` |
| `/preset`（`presets` `agent` `agents`） | 打开 Agent 预设名册。定义全是 YAML 目录：内置 < `~/.dock/presets/<id>/agent.yml` < 项目 `.dock/presets/<id>/agent.yml`（后写覆盖；仍可读旧 `<id>.yml`）。**加一个目录就是一个 Agent**，不必改代码。子代理写在 `agents/<type>.yml`（人设 + 工具允许名单）。**`n` 新建 / `d` 复制默认写当前工作区** `.dock/presets/<id>/`（有项目层时 origin 为项目）；改内置模式仍写 `~/.dock/presets/` 覆盖。新建人设默认写 `.dock/presets/<当前模式>/agents/`。系统提示注入工作区相对路径 `.dock/presets` 与 `.dock/presets/<模式>/agents`（空名册也会注入，不用绝对 `{cwd}`）；只有用户明确要求保存到全局才写 `~/.dock/presets/`。省略 `tools` = 当前已注册全部工具；`[]` = 空；列表 = 允许名单（Dock 工具名）。`replace_prompt: true` 时 `persona` 整份替换系统提示。手写 `agents/<type>.yml` 后，`task` 校验、`/preset` 和下一采样步系统提示都会重读；本轮要立刻出现在 enum 里时用 `task`（`reload_roster: true`），不必新开会话。新建模式写完后用 `/preset` 应用该 id（`reload_roster` 不切模式）。名册列出子代理 id；`n` 新建、`d` 复制、`a` 应用、`x` 删除（内置不能删；删覆盖则恢复内置） |
| `/history` | 搜索提示词历史 |
| `/copy [N] [file]` | 把上一条回复复制到剪贴板或文件 |
| `/find` | 搜索对话 |
| `/usage`（`cost`） | 本会话用量 overlay（用量 tab）：输入 / 输出 / 缓存命中与占比 / 思考 / 调用次数 / API 耗时。接口若带 `cost_in_usd_ticks` 才显示费用，缺省为「未上报」（不是免费）。**Tab** 切到占用。**没有** grok.com 账号额度、`/usage manage` |
| `/context` | 打开占用 overlay：菱形条按系统提示 / 消息 / 推理开销 / 空闲拆分，下面列出工具定义、**技能**、工作流、MCP、本地按需。点「系统提示」按段展开（基座 / Cordis / 技能 / 工作流 / 人设 / 子代理）。**工具定义只含模型可见项**（`search_tool` / `use_tool` 等）。MCP extras 与 `register_deferred` 的本地工具 **不计入** `used`；点开只看目录。`search_tool` 返回的 schema 记在消息历史里直到压缩。技能 listing 给模型看所以仍在系统提示正文里；图例单独占一行（标「已计入系统提示」），不把同一段再加进 `used`。工作流同理。点顶栏右上角「上下文」同样打开。点分类行或色块看该类明细；Esc 返回总览。**Tab** 切到用量。占用 live-lookup `"context"` |
| `/compact [说明]` | 压缩旧对话为摘要发给模型（Grok 同款 structured `<summary>` 九段）。滚动区保留原对话，末尾加「已压缩上下文。」（Grok pager 是 SessionEvent，不擦 scrollback）。可选说明并进摘要。上下文达到窗口 **85%** 时自动压缩；失败或压完仍超阈值则等到下一条用户消息再自动。手动 `/compact` 不受此限制 |
| `/theme` `/t` | 切换配色 |
| `/timestamps` | 开关滚动区时间戳 |
| `/effort` | 设置推理强度 |
| `/export [path]` | 把对话导出到文件 |
| `/cd` | 切换工作目录（进程级 cwd）。只在 Dock 终端生效；浏览器 companion 会拒绝 |
| `/help` | 显示斜杠命令 |
| `/skills` | 额外命令（`"slash"` extras，不是 CATALOG）：Notice 列出已发现技能的斜杠名、来源层、路径。撞名技能 `skills` 时本 overlay 优先，该技能只能用 `skill` 工具 |
| `/<技能名> [参数]` | 每个 `user-invocable` 技能登记成 extra（`kind: prompt`，发送）。用户气泡保持 `/name args`（skill 色）；`agent/pre-step` 把 SKILL.md 全文注入 `SystemReminder`（`$ARGUMENTS` / `$SKILL_DIR`），不先调 `skill` 工具。不可盖内建 `RESERVED_SLASH`（如 `/help`）。`disable-model-invocation` 的技能仍可斜杠，但不进 listing / `skill` 工具 |
| `/quit` `/exit` | 退出 |

---

## Agent 预设 YAML

层（后者覆盖前者）：crate 内置 `code` / `minimal` / `cordis` / `warden`（`presets/<id>/agent.yml` + 可选 `agents/*.yml`）→ `~/.dock/presets/<id>/` → 项目 `.dock/presets/<id>/`。仍可读旧 `<id>.yml`；同名目录优先。**预设目录名**即模式 id（`[a-z0-9][a-z0-9-]*`）。也可以用内置模式的**显示名**做目录：项目 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`，`编码/` 叠到 `code`，`守望/` 叠到 `warden`。子代理文件名是 `subagent_type`（ascii 或汉字，如守望的 `甲`）。**新建模式默认写当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` `n`/`d` 有项目层时落到这里；系统提示注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`）。**新建人设默认写** `.dock/presets/<当前模式 id>/agents/`。`/preset` 切换后下一轮系统提示更新模式目录后缀；`/cd` 不再改写这些路径以免打掉 prompt cache。空名册（新建模式、`minimal`）仍注入写路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后当前会话生效（`task` 校验立刻重读；本轮 enum 用 `task` 的 `reload_roster: true` 刷新）；新建模式写完后用 `/preset` 应用该 id。改 crate 内置 `presets/` 要重新编译。`/preset` 打开时也会重读。`warden`（守望）主代理只调度，子代理见 `agents/`。

```yaml
# <repo>/.dock/presets/review/agent.yml
# 用户明确要求保存到全局时才写 ~/.dock/presets/review/agent.yml
name: 评审
description: 只读仓库
# 省略 tools = 当前进程全部已注册工具；[] = 不用工具
tools:
  - read_file
  - grep
  - glob
persona: |
  只读代码，不要改文件、不要跑会改状态的命令。
replace_prompt: false
order: 10
```

子代理另放 `agents/<type>.yml`（人设 + 工具集），默认路径是项目 `.dock/presets/<当前模式 id>/agents/<type>.yml`。主代理用**唯一**的 spawn 工具 `task` 委派当前模式名册角色，`subagent_type` 为该文件名（不含 `.yml`）。发给模型的 `task` 参数带名册 `enum`（名册多出来的角色必须出现在这个 enum 里才能调）。本轮新写 `agents/<id>.yml` 后，用 `task` 的 `reload_roster: true` 刷新（不 spawn），下一采样步的 enum 才带新 id。新建模式写 `{cwd}/.dock/presets/<id>/agent.yml`（id 不能是汉字），然后 `/preset` 应用；`reload_roster` 只刷新当前模式名册。`task` 只接受 prompt / description / subagent_type / run_in_background / resume_from / reload_roster——`cwd` / `isolation` / `model` 已从参数里删掉（dock 不实现）。每个子代理都是可续跑的：一轮跑完 park idle，父代理收到回合结束通知（未 `report` 时带上该轮正文），所以后台 spawn 不需要轮询 `get_task_output`。持续交流：父→子 `send_message`（idle 时 queued / urgent 都会立刻开下一轮；urgent 只在 running 时才是 send-now）；子→父 `report`（可多轮多次）。`list_agents` 是状态源。`interrupt_agent` 停本轮；要整个撤掉用 `kill_task`。用户 Stop / 新会话会取消本会话子代理。

`tools` 里写 Dock 已挂上的名字（`bash` `read_file` `cordis_define` …），不是 DSH 包名。未挂上的名字在画布右侧显示「未挂载」。

---

## Overlay / 快捷键 / 底栏

| 面 | 行为 |
|---|---|
| `g`（输入框空、有目标、且当前没在生成） | 打开/关闭目标 overlay，可改标题、暂停、清除 |
| 提问 overlay | 听 `ask/pending`，和权限 overlay 同款 |
| 浏览器配对 overlay | 听 `gateway/pairing`。`/pair` 第一行开启或关闭监听；首次 Origin 请求弹出「允许浏览器连接？」；下列待批与已绑来源（Enter 批准 / `x` 拒绝或撤销） |
| MCP elicitation | 听 `mcp/elicit`。权限 / 提问 overlay 会抢前台（队列仍在）。表单逐步填：选项带「其他」、自由输入空内容闪「请输入具体内容」。URL 模式 Enter 开浏览器，等 `notifications/elicitation/complete` 或 Esc 取消 |
| 动态插槽 `Overlay::Slot` | `"tui.slots"` 登记的纯文本 pane（复用 Notice 布局）。Esc 关闭；↑/↓ 滚动并把规范化键名转给 `on_key`（`esc` / `enter` / `up` / `down` / `char:x`）。脚本 `open_slot` 或 slash `kind: slot` 打开 |
| `/cordis` | Notice：永久（磁盘）与会话（内存）插件一览 |
| 插槽 HUD | `hud: true` 的插槽每帧 `render` 第一行，live-look 进快捷键条，不替换 status bar |
| `/preset` 画布 | 左侧完整目录（当前 `"tools"`，右侧工具简介）、右侧本预设工具集。身份区列出本模式 `agents/` id。Enter 左加右删。名册 `n`/`d` 新建或复制默认写项目 `.dock/presets/<id>/`；改内置会写到 `~/.dock/presets/<id>/agent.yml` 覆盖。Esc 从画布回名册 |
| 欢迎页 | 空会话是 **Grok hero box**：圆角方框、左边 braille logo、右边 `Dock` + 版本 + 一句说明 + 菜单（新会话 / 恢复 / 退出）。窄窗改成框内上下叠。点击菜单行仍走原来的快捷键。 |
| prompt 底栏 | **右对齐**画在输入框底边：模型 · **当前 Agent 预设名** · 权限（询问/始终允许）。计划模式加 `计划`；目标进行中或暂停加 `目标`。生成中输入框有字：`Enter:queue` / `Ctrl+Enter:send now`。空输入且生成中、没有排队：`Esc:cancel`。空输入且有排队：排队条钉在输入框上方（`#1` 正文 `[发送]`）；`Enter:send now` 立即发出最早一条，点 `[发送]` 发那一行；`Esc:edit` 把最新一条收回输入框改。**Esc 取消且本轮还没有模型/工具输出**时完整收回刚发送的正文和图片；约 1s 内再按 Esc 不会清空收回的草稿（Grok 双击 Esc 宽限）。有目标、输入框空、且没在生成时快捷键条加 `g:goal`。子代理 `report` 续跑父会话时也算 working，期间发的消息会排队，不会被丢掉。取消或立即发送时会给未完成的 tool call 补上「已中断。」结果，避免下一条采样 400 |
| 目标状态条 | 有目标时钉在子代理头像和排队条之间（紧挨输入框上方）：标题 + `[暂停]`/`[继续]` `[修改]` `[关闭]`；第二行是进度备注和彩色扫光波。运行中随 80ms 刷新换色；暂停后冻结变灰。点标题打开 overlay，点 `[修改]` 直接改标题 |
| 子代理头像条 | 未结束的子代理钉在输入框上方横排**正方形**头像（框里是显示名首字，无名字行）。运行中边框扫光并呼吸。点头像打开 **Grok 同款全屏边框**：标题栏 + 子代理自己的滚动区（工具卡折叠循环与主界面相同：收起 → 截断 → 展开）。框底可输入，Enter 经 `send_message`（urgent）发给该子代理以调整。Esc / q（输入为空时）/ [✗] 返回 |
| `Ctrl+t` | 切换待办卡片折叠：未完成（默认）→ 全部 → 一行概要。列表非空时快捷键条出现 `Ctrl+t:todo`；没有列表时闪「当前没有待办列表」 |
| 待办卡片 | 钉在滚动区末尾，live-lookup `"todos"`。头行 `◆ 待办  ◆◆◇◇◇◇◇◇ 2/7`（进度条 + 已关闭/总数），点头行或 `Ctrl+t` 循环折叠。默认只列未完成项（最多 6 行，超出 `… 还有 N 项`），已完成折成 `✓ 已完成 N 项`；「全部」连已完成一起列；「概要」只留头行并把进行中的标题带上。`todo_write` 调用本身画成 `◆ 待办 写入 N 项 ✓2 ▶1 □1` 卡片，展开是本次写入的条目，不铺 JSON |
| 滚动区 | 用户气泡灰带铺满行宽（Grok `with_background`）；图标抄 Grok `todo_pane`（`□` `▶` `✓` `✗`）。**`subagent` 是独立卡片**：当前模式名册角色名 + type id，始终折叠，点击打开全屏对话。Grok `task` 仍画成原来的子代理块。后台 bash / `monitor` 画成任务卡片。`update_goal` 画成 **Goal 卡**（设定 / 进展 / 完成 / 受阻）。`scheduler_create` / `scheduler_list` / `scheduler_delete` 画成 **Loop 卡**（设定 / 列表 / 关闭）。`search_tool` 画成 **Search Tools** 卡（关键词 + 条数；展开为「动作  服务器」列表，不把 JSON schema 铺开）。MCP 调用（`use_tool` 或遗留 `mcp_{server}__{tool}`）画成 **Server Action** 卡（参数 kv + 输出；`use_tool` 标题用内层 `tool_name`）。运行中 ◆ 会脉冲，活动与耗时随 80ms 刷新 |
| 顶栏 | 右上角 `上下文 {used}/{window}`：当前窗口占用（系统提示 + 消息 + 模型可见工具定义 + 图片 + 推理；有官方 prompt 则取较大值）。MCP extras 与按需本地工具不进采样工具表，也不计入占用。点击打开占用 overlay，再点分类看明细。不是 `/usage` 的会话累计账本 |
| `enter_plan_mode` / `exit_plan_mode` 卡 | scrollback：`◆ Plan: Enter\|Exit`；Exit 展开 markdown。`exit` 会 park 审批，写闸保持到用户决定 |

---

## `/usage` 数据

抄 Grok `UsageLedger` + `session_usage_block_text`，不接 `x.ai/billing`。

- 只把 SSE **官方** `usage` 折进账本（`prompt_tokens_details.cached_tokens`，缺省再认 `prompt_cache_hit_tokens` / `cache_read_input_tokens`；思考认 `completion_tokens_details.reasoning_tokens`）。开转前的本地估算只更新顶栏占用，不入账（Grok fail-closed：缺费用 ≠ 免费）。
- **缓存占比** = 缓存命中 / 完整输入（Grok：`cached_prompt_tokens` 是 `prompt_tokens` 的子集，不要相减）。会话累计用总量相除，不是各轮百分比再平均。另显示 **上一轮命中**（最近一次主循环调用）。超过 100% 钳到 100%；输入为 0 显示 `-`。格式抄 Grok `/context` 的 `percent_of_window`（不足 10% 一位小数，否则整数）。
- 主循环每次 `finish_llm` 记一笔；子代理 isolate 结束时 `record_subagent` 折进父会话，不增加 `numTurns`。
- `/new` / `clear` / `/resume` 清零账本。

---

## 仍比 Grok 薄（CLI 面）

- `/goal`：模型可用 `update_goal(objective)` 自己开目标；continuation 已接上（内循环 + 整轮结束后隐藏 GoalSummary）。尚未自动 spawn Grok 的 `goal plan writer` / classifier / strategist
- 一轮采样安全上限 256 步（Grok 默认不限 `max_turns`）；撞上限时滚动区留下说明，而不是静默停
- 提问 overlay：有 Other，自由输入比 Grok pager 简单
- `/usage`：占用 + 用量两 tab；无 grok.com 账号额度条、无 Session info
- `/compact`：Grok full-replace 一轮（structured 九段摘要 + 85% 自动；只换发给模型的历史，滚动区不擦）。无 two-pass / segments / `updates.jsonl` 旁路；模型前缀落 `compact.json`
