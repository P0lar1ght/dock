# CLI / TUI

对照当前 `cordis-tui` 斜杠目录与 overlay，不是愿望列表。硬规则见 [AGENTS.md](AGENTS.md)，架构见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。**模型工具**记在 [TOOLS.md](TOOLS.md)；本文件只记人操作的命令、快捷键、overlay、底栏。

用户可见文案中文；Grok 底栏那种短 hint（`Enter:send`）保持英文无空格。

进程入口（`cargo run -p cordis-app -- …`）：`--resume` 恢复本 cwd 最近一次落盘会话，`--resume <id>` 指定 id，`--help` 看用法。`dock serve --origin <origin> [--application <id>] [--bind <addr>]` 是给父进程（桌面 GUI）用的**无头**入口：不挂 TUI，网关挂载即在回环上监听（默认 `127.0.0.1:0` 由 OS 分配端口）；stdout 一行一个 JSON——先 `{"event":"ready","ws":…,"ticket":…}`，之后 stdin 每写一行 `{"cmd":"ticket"}` 回一张新 ticket；stdin 关闭即退出。ticket 只经这条管道交给父进程，不开放 HTTP 领取，也不留下 Origin 绑定。对话在 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` + `chat_history.jsonl`），不是项目 `.dock/`。

---

## 斜杠

目录在 `cordis-tui/src/slash/mod.rs` `CATALOG`。别名也能提交。动态插件可以通过 `"slash"` **追加**命令（工厂 `slash` 或 Rhai `host.register_slash`），不能替换本表里的内建项。追加的命令出现在下拉补全和 `/help`；`prompt` 种会填入或发送模板（`{args}` 换成键入的参数），`overlay` 种打开只读标题+正文 overlay，`slot` 种打开已登记的 `tui.slots` id（`text` = 插槽 id），`tool` 种直接跑 live 工具（`text` = 工具名；参数：空=`{}`、以 `{` 开头=原始 JSON、否则 `{"args":"…"}`），结果进 Notice，不经模型。停掉该动态 Plugin 或退出进程后，**会话**额外命令消失；磁盘永久插件会在下次启动时再挂上。

浏览器 `embed-sdk` **不要**为每个斜杠单独适配。Gateway 暴露 `slash/list`（补全目录：TUI `slash_catalog()` + `"slash"` 追加 + 浏览器 `/screenshot*`）和 `slash/execute`（接到 spine / session.port）。JS 只做前缀过滤、截图采集、以及把 `{ kind: filled|notice|menu|capture|submitted|applied }` 画出来。TUI overlay 类命令（`/pair` `/theme` `/preset` `/settings` …）和 **`/cd`** execute 会回 Notice「请在 Dock 终端使用」——list 标 `terminal` 的名字不会改进程状态（`/settings timestamps` 也不会）。`/timestamps` `/think` `/model` `/effort` 仍可从浏览器直接改。未知 `/foo` 为 `passthrough`，当作普通 prompt 发给模型。`slash/execute` 的 `threadId` 只能是 `live` 或省略。

下拉打开时 Tab 把当前高亮项填进输入框（命令名阶段填 `/命令 `，参数阶段填 `/命令 参数 `）。Enter 在**命令名**阶段填入；空格之后（参数下拉仍开着）Enter 发送并解析 —— 但只要你用 ↑↓ 选过其中一行，Enter 就先把**那一行**填进输入框（再按一下才发），不会把光秃秃的 `/命令` 发出去。无参数命令在名字后空格关闭下拉。有参数目录的命令（`/lsp` `/theme` `/model` `/effort` `/loop` `/settings` `/tab`）空格后继续列出参数：`/lsp s` 可 Tab 到 `status` / `setup`，再 Enter 执行。

| 命令 | 行为 |
|---|---|
| `/settings`（`config` `prefs`） | 设置 overlay；有参数则直接改（`timestamps` / `theme` / `model` / `protocol`）。浏览器 companion 一律拒绝（含带参），请用 `/timestamps` `/think` `/model` `/protocol` `/effort` |
| `/new` | 归档当前会话并清空。账本用量一并清零。归档写入 `$DOCK_HOME/sessions/<cwd>/` |
| `/model` `/m` | 切换当前模型。**目录只来自 config.toml**，没写就是空的（会提示去 `config.toml.example` 抄）——内置那几条没端点的假条目已删。目录 `[model.<id>].api_backends`（列表，第一条=默认；单数 `api_backend`=只支持这一条；**都不写默认 `responses`**）声明这个端点支持哪几条 HTTP 线，声明多条时 picker 里会带出来；`api_model` 是发给上游的 slug（省略=目录 id）；`auth_scheme` 不写就跟着当前协议走（messages→`x-api-key`，另两条→Bearer；第三方网关的 /messages 多半仍要 `bearer`）。同一端点各协议入口不同时写协议块 `[model."<id>".<协议>]`，只覆盖连接三件套 `api_base_url` / `auth_scheme` / `api_model`（DeepSeek 的 Anthropic 侧是 `…/anthropic`）。能力与默认值不随协议变，只写模型级：`context_window` / `max_output_tokens` / `reasoning` / `reasoning_effort` / `reasoning_efforts` / `supports_images` / `prompt_cache`，**不写就不发那个参数**。切模型会按新模型重新播种推理默认值与协议 |
| `/protocol`（`proto` `wire`） | 在**当前模型声明过的**协议之间切（`responses` / `chat_completions` / `messages`）。空参数或名字打错开 picker，不猜一条发出去；切到没声明的那条会被挡下并提示去 config 里补 `api_backends`。一个端点同时开着 /responses 和 /chat/completions 时用这个，不用再配一个只有协议不同的重复模型条目。运行时状态，不落盘；`/model` 换模型即按新模型的默认重新播种。浏览器 companion 也支持：空参数回一条 notice 列出可选协议 |
| `/resume` | 打开会话 picker，恢复本工作区已落盘的会话（进程重启后仍在）。用量账本不随归档恢复（Grok：新进程 resume 清零）。进程入口 `--resume` / `--resume <id>` 启动时直接恢复 |
| `/pair`（`pairing`） | 浏览器 Origin 配对：第一行开启/关闭回环网关（默认不监听；首选 `127.0.0.1:18991`，占用往上找，同端口再试 `[::1]`）。开启后待批请求可批准，已绑来源可撤销。overlay 显示实际监听地址。首次连接弹出「允许浏览器连接？」；Enter 批准 / `x` 拒绝或撤销（在网关行上 `x` 关闭监听）。浏览器 companion 的 list/execute 限制见上文。CORS 反射任意 Origin：鉴权靠配对 + 回环。Approved 的 poll **不**回 ticket 明文；`POST /v1/pairing/exchanges` 校验 TTL、一次性消费 |
| `/loop` `/cron` | 空命令在输入框留下用法（`用法: /loop [间隔] <提问>` + `/loop `）。有参数则用户气泡是 `/loop {参数}`，模型看到 `loop_schedule_instruction`（须 `scheduler_create`，`fire_immediately: true`，不要当场执行提问）。没有间隔就问用户，不要自己编。7 天后自动过期。查看 / 关闭：`/tasks` Watchers，`x` 或 `[✗]` |
| `/plan [说明]` | 开计划模式；无说明只切模式（Pending，发第一条 prompt 后变 Active）。有说明则 Active 并提交 |
| `/view-plan`（`show-plan` `plan-view`） | 查看本页计划文件（打开时读一次，pretty markdown）；若 `exit_plan_mode` 正在等待批准则打开审批 chrome（`a` 批准 / `s` 修改 / `q` 放弃） |
| `/goal` | 输入框留下用法（`用法: /goal <目标>` + `/goal `），不会清空。再发送即为目标 |
| `/goal <目标>` | 开目标；用户气泡是目标文本；`goal_instruction` 走历史尾部 `<system-reminder>`（不改系统提示）。模型只回一句文本时不会结束目标：注入隐藏 continuation（Grok `Goal NOT complete`），继续采样直到 `update_goal(completed)` / 暂停 / 取消。整轮结束后若目标仍在进行，再塞一条隐藏 GoalSummary 开下一轮。模型也可用 `update_goal(objective)` 自己开目标 |
| `/goal status\|edit\|pause\|resume\|clear` | 打开目标 overlay / 暂停 / 继续 / 清除 |
| `/tasks` | Grok 分组 pane：Workflows → Subagents → Tasks → Watchers。子代理行显示当前模式名册的角色名（如守望下的「岑」而不是 `Cen`）。Enter / 点击子代理或后台任务打开 **Grok 同款全屏边框**（子代理：工具卡折叠循环 + 框底输入 `send_message`）。Esc 从全屏回到本列表。子代理第一轮结束后显示 **idle**（不是 done）；idle 不算 running。能停的行右端有 `[✗]`，选中按 `x` 或点它即可——与主界面任务条同一条 `kill_task_target`：Watchers 里的 loop 是关闭排程（`scheduler_delete` / `cron.cancel`），Workflows 里在跑的 run 是取消本次运行并收掉它的子代理（`Workflows::stop`），**Tasks（Job）** 走 `Jobs::kill`，**Subagents** 走 `Subagents::kill`（不只 Watchers/Workflows） |
| `/dashboard`（别名 `/overview`，或点右上角 `[Agents]`） | **会话面板**：同屏观察并**驱动**多个会话。不是 `/resume` 的换皮——`/resume` 是「挑一个历史会话恢复到当前页」，一次性选择器；这里是长期待着的工作面。画成**占满整屏的独立面板**（抬头行 + `+ 新会话` 动作行 + 分组列表 + peek 面板 + 底部快捷键条），不套边框：边框会让它读起来像弹出来的一张表。行只有会话——开着的分页（`"tui.tabs"`）与跨 cwd 的磁盘会话（`"roster"`），子代理归 `/tasks`；已开着的那条不会在「历史」里重复（按 `Sessions::live_session_id()` 去重），`/btw` 旁问页不列。三组：**进行中** / **空闲** / **历史**，右上角汇总 `◇ N 空闲 · M 进行中`（只数在场的，不数历史）。<br><br>**两个焦点，Tab 切**。焦点在**列表**：↑↓ 选行（打字进搜索，认标题 / 摘要 / cwd，抬头计数跟着过滤走），`Enter` 抬头折叠该组、分页行切过去、历史行开成新的一页（当前页留着），`x` 删除选中的历史会话，`Ctrl+n` 开新会话，`Esc` 关闭。焦点在 **peek 输入框**：打字进输入框，`Enter` 把消息**发给选中的那个会话**——不切页、不离开面板，所以能盯着三个会话轮流派活（走那一页自己的 `"session.port"` `SessionRef::submit`）。焦点在输入框时 ↑↓ 仍然换选中行，于是「换目标 → 打字 → 回车」不用来回 Tab。底部快捷键条跟着焦点变。<br><br>peek 面板显示选中会话的对话尾巴，**两种来源走同一套 `child_transcript` 渲染**——画出来和正常对话一模一样（用户气泡、思考行、markdown、工具卡、配色全在），不是灰字摘要。开着的分页读它自己的 `"sessions"`（实时）；历史会话走 `Roster::transcript`（按 id 读一份，只缓存最近一个：同一选中项每帧命中缓存，换选中项才重读，那是人手速度）。抬头右上角标时距。peek 占屏高约**四成**（下限 10 行、上限 24 行），并始终给列表留 5 行；挤不出下限就整块让掉。滚轮落在列表上只移动选中行，到顶和到底都停下，不绕回另一头；落在 peek 对话上只滚这段对话，到顶停下，不会带着列表一起走。换选中行时 peek 回到该会话的尾巴。历史会话还没有自己的分页，输入框提示「Enter 开成一页后再派活」——它没有活着的循环。Enter 把它装进一张新的常驻页（自己的会话、循环和输入框）并切过去，当前页不动；已经开着的再按一次只是切到那一页。历史会话**只有在当前工作目录下才开得了页**（`Sessions::adopt_archived` 只认 `load_cwd(当前 cwd)`），别的目录的行尾标「其它目录」，Enter 提示先 `/cd` 过去 |
| `/workflow` / `/workflow runs` | Grok `Workflow Runs` overlay。Enter 开选中 run 的详情页：左栏阶段（脚本 `meta.phases`，带完成计数），右栏该阶段的子代理行 + 各自最新一条发给父级的消息。`↑↓` 换阶段、`x` 停这条 run、`esc` 回列表。没有 pause/resume（要 journal 持久化，尚未实现） |
| `/workflow <name> [参数]` | 立刻用 `workflow` 工具按注册名启动（不经模型）。参数：纯文本 → `args.query`/`args.objective`；以 `{` 开头 → 原始 JSON。可选 `--agent-budget N`（真正生效：耗尽时 run 以 `BudgetExceeded` 收尾）。`pause`/`resume`/`save` 仍打开 overlay |
| `/workflow stop <name\|run_id>` | 取消一次在跑的 run 并收掉它的子代理。显示名与 run id 都认，活跃的优先；不带参数仍打开 overlay |
| `/deep-research <查询>` | 内置 Rhai 工作流 extra（`kind: tool` → `workflow`，`source.type=name`）。其它已发现工作流同样登记成 `/<name>`。不可盖 `RESERVED_SLASH`；与技能撞名时技能 extra 优先 |
| `/<工作流名> [参数]` | 项目 `.dock/workflows/<name>.rhai`、用户 `~/.dock/workflows/<name>.rhai`（以及 `~/.dock/bundled/workflows/`）里 `meta.name` 与文件名一致的脚本。不能覆盖内置 `deep-research` |
| `/mcps` | Grok 分组 pane：标题「MCP 服务器」、分组「本地 (N)」、徽章 `[就绪]` / `[需认证]` / `[不可用]` / `[已禁用]`、右侧 `(本地)`。Space 开关当前服务器或工具（写入 `config.toml`：`[mcp_servers.<name>].enabled` 与 `[disabled_mcp_tools.<server>]`），并立刻从 `"tools"` 注销/追加注册（MCP 不进 sampler 工具表，走 `search_tool` / `use_tool`；不重写系统提示；目录变化用服务器级 `<system-reminder>`）。**打开 pane 就重读一遍 `config.toml` 并与已连服务器对账**（静默，没变化不提示），`Ctrl+R` 手动重载：新增 / 删除 / 改命令行 / 改 `enabled` / 改 `[disabled_mcp_tools.*]` 都不用重启 dock，模型在会话里自己写的 MCP 配置按一下就生效，目录变化仍走服务器级 `<system-reminder>` 告诉模型。**配置没变且连着的服务器不重连**——重连 stdio 会杀掉子进程，连带丢掉它自己的会话状态（cua-driver 的浏览器）；上次连不上的每次重载都会重试，所以改完命令行重载即自愈。`Ctrl+R` 走 Ctrl 分支，搜索框里照样能打 `r`。`i` 对 HTTP 服务器打开浏览器 OAuth（PKCE，token 写 `~/.dock/mcp_credentials.json`，不是 grok.com 登录）；Enter 展开/收起工具（`N 个工具` / `N 个工具（M 个已启用）`）；Esc 关闭。stdio 或 Streamable HTTP。工具表跟 `tools/list` 翻页和 `tools/list_changed`；HTTP 跟 GET SSE，session 404 会重新握手 |
| `/lsp` | 探测 PATH 上的 `rust-analyzer` / `typescript-language-server` / `gopls` / `pyright-langserver`，按工作区标记（含子目录，跳过 `node_modules` / `target`）把缺的服务器写入 `.dock/lsp.json`，不覆盖已有条目。Notice 显示保留 / 添加 / 跳过。当前会话会尝试启动新服务器。空命令或 `/lsp setup` 写项目配置；`/lsp status` 只看不写；`/lsp user` 把 PATH 上有的默认服务器写入 `~/.dock/lsp.json`（不要求工作区标记）。输入 `/lsp ` 后 Tab / Enter 补全 `status` / `setup` / `user`（`/lsp s` 可滤到 status）。浏览器 companion 拒绝（请在 Dock 终端用） |
| `/browser` | 额外命令（`"slash"` extras，不是 CATALOG）：完整 TUI 驾驶舱 overlay（live-lookup `"browser"`）。显示未连接/已连接、**显示（有头/无头）**、标签页、截图路径、wait/对话框、最近 evaluate/network、P0–P2 能力摘要、审批提示；不渲染网页。按 **`h`** 切换有头/无头（写入 `config.toml` `[browser].headed`，默认无头）；**不**重启已开 Chromium，需 `browser_close` 后再 `browser_open`。`DOCK_BROWSER_HEADED`（任意非空）覆盖为有头。BUA 由 chromiumoxide CDP 驱动；`browser_open`=会话+首 URL，`navigate`/`navigate_back`=同会话跳转；P1 drag/dialog/upload/resize；P2 `browser_evaluate`（**与 bash 同级 permissions 门** + 计划模式挡）/`console_messages`/`network_requests`、同域 iframe（`frame`/`frame_selector`，跨域 clear error）。`browser_*` 仍 `register_deferred`（不进 sampler / `specs_for_model`），走 `search_tool` / `use_tool` |
| `/computer` | 额外命令（薄驾驶舱，Cordis/TUI）：本机 **cua-driver** 状态机 —— `mcp-client 未挂载` / `未安装` / `缺授权` / `已禁用` / `未连上` / `已连接`，外加最近审批提示；**不**嵌真桌面。**引导键**：`i` 装 / 重装 driver（走官方 `https://cua.ai/driver/install.sh`），`p` 在 macOS 跑 `cua-driver permissions grant` 要辅助功能 + 屏幕录制，`Ctrl+R` 重新探测本机并重载 MCP 配置，`↑/↓` 滚动，`Esc` 关闭。`i` / `p` 都是**两步**：先进确认态列出要执行的每一步，`Enter` 才真的跑（`Esc` 只取消确认、不关窗）；进度一行行显示在驾驶舱里，装完自动重探 + MCP 重载，不用重启 dock。底栏 hint 随状态变（`i:install` / `i:reinstall` / `p:grant` 仅在可用时出现）。装好 driver 后**不用写 `config.toml`**：Dock 自己发现 `PATH` / `~/.local/bin` / macOS `.app`，并注入内置 `[mcp_servers.cua-driver]`；`DOCK_CUA_DRIVER=<绝对路径>` 指定、`=off` 关掉。桌面键鼠走 MCP 公名 `mcp_cua-driver__*`（`search_tool` / `use_tool`），**全部**与 bash 同级 permissions / 计划门。细节见 [TOOLS.md](TOOLS.md)「Computer / CUA」与 [config.toml.example](config.toml.example)。勿与 Dock `browser_*`（chromiumoxide）混淆 |
| `/cordis`（`plugins`） | Notice「Cordis 插件」：磁盘永久层（项目 `.dock/plugins/<id>/` 覆盖用户 `~/.dock/plugins/<id>/`）和本会话内存插件。Esc 关闭。写成永久用模型工具 `cordis_promote` |
| `/preset`（`presets` `agent` `agents`） | 打开 Agent 预设名册。定义全是 YAML 目录：内置 < `~/.dock/presets/<id>/agent.yml` < 项目 `.dock/presets/<id>/agent.yml`（后写覆盖；仍可读旧 `<id>.yml`）。**加一个目录就是一个 Agent**，不必改代码。子代理写在 `agents/<type>.yml`（人设 + 工具允许名单）。**`n` 新建 / `d` 复制默认写当前工作区** `.dock/presets/<id>/`（有项目层时 origin 为项目）；改内置模式仍写 `~/.dock/presets/` 覆盖。新建人设默认写 `.dock/presets/<当前模式>/agents/`。`task` 工具 description 注入工作区相对路径 `.dock/presets` 与 `.dock/presets/<模式>/agents`（空名册也会注入，不用绝对 `{cwd}`）；只有用户明确要求保存到全局才写 `~/.dock/presets/`。省略 `tools` = 当前已注册全部工具；`[]` = 空；列表 = 允许名单（Dock 工具名）。`replace_prompt: true` 时 `persona` 整份替换系统提示。手写 `agents/<type>.yml` 后，`task` 校验、`/preset` 和下一采样步的 `task` enum / role hint 都会重读；本轮要立刻出现在 enum 里时用 `task`（`reload_roster: true`），不必新开会话。新建模式写完后用 `/preset` 应用该 id（`reload_roster` 不切模式）。名册列出子代理 id；`n` 新建、`d` 复制、`a` 应用、`x` 删除（内置不能删；删覆盖则恢复内置）。名册与写路径**只**通过 `task` 工具的 description 发给模型，系统提示里不再另写一份 |
| `/history` | 搜索提示词历史 |
| `/copy [N] [file]` | 把上一条回复复制到剪贴板或文件 |
| `/find` | 搜索对话 |
| `/usage`（`cost`） | 本会话用量 overlay（用量 tab）：输入拆成命中 / 写入 / 未命中三段并画成 bar，加每轮命中率 sparkline、上一轮明细、未命中来源（主循环 / 子代理 / 压缩，只有真有子代理 / 压缩时才出现）、输出 / 思考 / 调用次数 / API 耗时。费用来源三态：上游带 `cost_in_usd_ticks`（目前只有 xAI）显示 `$X`；否则按 `[model.<id>.pricing]` 本地估算，显示 `约 $X（按 config 单价估算）`；都没有则「未上报」（**不是免费**）。**Tab** 切到占用。浏览器 companion 拿到的是同一份数据的文本版。**没有** grok.com 账号额度、`/usage manage` |
| `/context` | 打开占用 overlay：菱形条按系统提示 / 消息 / 推理开销 / 空闲拆分，下面列出工具定义、**技能**、工作流、**工程规约**、**记忆**、MCP、本地按需。点「系统提示」按段展开（基座 / Cordis / 人设 / 工作流 / 技能；子代理名册段已撤，改挂在 `task` 工具 description 上）。工作区规约不在系统提示里——它单列一行 **「工程规约」**，明细写「`AGENTS.md` · 已计入消息」，数是**实际注入的那一份**（文件刚改过、还没重注时与磁盘当前内容并不相等）。长期记忆（`[memory] enabled` 时）同样走消息流：单列一行 **「记忆」**，明细写「`MEMORY.md` · 已计入消息」（开着但本会话还没注入时写「待轮次注入消息」）；点开按全局 / 工作区列出 `MEMORY.md` 路径，下面是实际注入的原文。**工具定义只含模型可见项**（`search_tool` / `use_tool` 等）。MCP extras 与按需工具（`register_deferred` 的本地工具、动态插件经 `host.register_tool` 注册的工具）**不计入** `used`，分别归「MCP 服务器」「本地按需」；点开只看目录。`search_tool` 返回的 schema 记在消息历史里直到压缩；消息明细在有搜索时多一行 **「按需发现」**（`N 次 search_tool`，已含在「工具结果」里，单列出来是为了让重复搜索的代价可见）。技能 listing 给模型看所以仍在系统提示正文里；图例单独占一行（标「已计入系统提示」），不把同一段再加进 `used`。工作流同理。点顶栏右上角「上下文」同样打开。点分类行或色块看该类明细；「技能」「工作流」的明细是可点清单（行尾 ` ›`），再点一行看单项：来源、路径、token 估算、说明，以及 `SKILL.md` / `.rhai` 全文。Esc 或左上角 `‹` 逐层返回（单项 → 清单 → 总览 → 关闭）。**Tab** 切到用量。占用 live-lookup `"context"` |
| `/compact [说明]` | 压缩旧对话为摘要发给模型（Grok 同款 structured `<summary>` 九段）。滚动区保留原对话，末尾加「已压缩上下文。」（Grok pager 是 SessionEvent，不擦 scrollback）。可选说明并进摘要。上下文达到窗口 **85%** 时自动压缩；失败或压完仍超阈值则等到下一条用户消息再自动。手动 `/compact` 不受此限制。摘要调用**不带工具表**（摘要本来就不许调工具，带着白付几千 token）。压缩后的模型前缀里，**assistant 轮次原样保留推理内容**，且**不含**「已压缩上下文。」那条合成消息（它只进滚动区）：有的上游（DeepSeek thinking 模式）规定请求带 `tools` 时之前每一轮的推理都必须回传，缺了报 `The reasoning_text in the thinking mode must be passed back` |
| `/undo`（别名 `/rewind`） | 撤销上一轮**没产生模型输出**的用户消息：把它从会话日志里摘掉并还原回输入框（含图片）。闸与 cancel-rewind 同一条（`Sessions::rewind_inflight_user`）——最后一条 `User` 之后只要有非空 `text` / `reasoning` / `tool_calls` 就拒绝，成功的助手回合撤不掉；只有 `LlmOutput.error`（请求失败）、PreStep、Notice **不算**输出，所以provider 报错那一轮可以直接撤了重发，不必 `/new`。输入框为空、没有排队、当前没在生成时，**空闲 Esc** 走同一条路（撤不动就沉默，不闪提示）；`/undo` 撤不动会闪一条说明。底栏在可撤时显示 `Esc:undo`（判定与实际行为同一个谓词 `Sessions::has_undoable_send`，不克隆会话日志） |
| `/flush` | 把本会话要点写入 `$DOCK_HOME/memory/.../observations/`（需 `[memory] enabled` / `DOCK_MEMORY=1`）。压缩前达门槛时也会自动 flush；只写 memory，不写 compaction 段 |
| `/dream` | 手动 consolidate observations → topics（LLM） |
| `/memory` | 双栏浏览 memory（左列表 / 右 markdown 预览；`/` 过滤；`t` 会话开关；`DOCK_MEMORY=0` 仍强制关） |
| `/remember` / `/remember <note>` | 无参留下用法；有参写一条 global observation |
| `/theme` `/t` | 切换配色 |
| `/timestamps` | 开关滚动区时间戳 |
| `/effort` | 设置推理强度。**菜单来自当前模型的 `[model.<id>].reasoning_efforts`**，不写才列通用四档（low / medium / high / xhigh），写 `[]` 表示会推理但不接受档位参数（菜单空着、永不发 effort）——各家认识的档位不一样，列死一份会让人选到上游不认的值。不选就用 `reasoning_effort`，那个也没写就不发这个参数（上游默认）。`reasoning = false` 的模型这一项与 `/think` 都置灰，菜单只给一条说明、回车不会填进输入框 |
| `/export [path]` | 把对话导出到文件 |
| `/cd` | 切换工作目录（进程级 cwd）。只在 Dock 终端生效；浏览器 companion 会拒绝 |
| `/help` | 显示斜杠命令 |
| `/skills` | 额外命令（`"slash"` extras，不是 CATALOG）：Notice 列出已发现技能的斜杠名、来源层、路径。撞名技能 `skills` 时本 overlay 优先，该技能只能用 `skill` 工具 |
| `/<技能名> [参数]` | 每个 `user-invocable` 技能登记成 extra（`kind: prompt`，发送）。用户气泡保持 `/name args`（skill 色）；`agent/pre-step` 把 SKILL.md 全文注入 `SystemReminder`（`$ARGUMENTS` / `$SKILL_DIR`），不先调 `skill` 工具。不可盖内建 `RESERVED_SLASH`（如 `/help`）。`disable-model-invocation` 的技能仍可斜杠，但不进 listing / `skill` 工具。listing 段**只发给主会话**，子代理要在 `agents/<type>.yml` 写 `listings: true` 才带（内置只有 `general-purpose` 打开；工作流 listing 同一开关）；另外 header 点名的 `skill` / `workflow` 不在当前预设 allowlist 里时整段不发（`warden` 即如此） |
| `/quit` `/exit` | 退出 |

---

## Agent 预设 YAML

层（后者覆盖前者）：crate 内置 `code` / `minimal` / `cordis` / `warden`（`presets/<id>/agent.yml` + 可选 `agents/*.yml`）→ `~/.dock/presets/<id>/` → 项目 `.dock/presets/<id>/`。仍可读旧 `<id>.yml`；同名目录优先。**预设目录名**即模式 id（`[a-z0-9][a-z0-9-]*`）。也可以用内置模式的**显示名**做目录：项目 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`，`编码/` 叠到 `code`，`守望/` 叠到 `warden`。子代理文件名是 `subagent_type`（ascii 或汉字，如守望的 `甲`）。**新建模式默认写当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` `n`/`d` 有项目层时落到这里；`task` description 注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`）。**新建人设默认写** `.dock/presets/<当前模式 id>/agents/`。`/preset` 切换后下一轮 `task` description 更新模式目录后缀；`/cd` 不再改写这些路径以免打掉 prompt cache。空名册（新建模式、`minimal`）仍注入写路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后当前会话生效（`task` 校验立刻重读；本轮 enum 用 `task` 的 `reload_roster: true` 刷新）；新建模式写完后用 `/preset` 应用该 id。改 crate 内置 `presets/` 要重新编译。`/preset` 打开时也会重读。`warden`（守望）主代理只调度，子代理见 `agents/`。

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

子代理另放 `agents/<type>.yml`（人设 + 工具集），默认路径是项目 `.dock/presets/<当前模式 id>/agents/<type>.yml`。主代理用**唯一**的 spawn 工具 `task` 委派当前模式名册角色，`subagent_type` 为该文件名（不含 `.yml`）。发给模型的 `task` 参数带名册 `enum`（名册多出来的角色必须出现在这个 enum 里才能调）。本轮新写 `agents/<id>.yml` 后，用 `task` 的 `reload_roster: true` 刷新（不 spawn），下一采样步的 enum 才带新 id。新建模式写 `{cwd}/.dock/presets/<id>/agent.yml`（id 不能是汉字），然后 `/preset` 应用；`reload_roster` 只刷新当前模式名册。子代理默认不带技能 / 工作流 listing（`agents/<type>.yml` 的 `listings: true` 打开；打开还要求该角色的工具集里有 `skill` / `workflow`，否则那段仍不发）。`task` 只接受 prompt / description / subagent_type / run_in_background / resume_from / reload_roster——`cwd` / `isolation` / `model` 已从参数里删掉（dock 不实现）。每个子代理都是可续跑的：一轮跑完 park idle，父代理收到回合结束通知（本轮没给父级发过消息时带上该轮正文），不需要轮询。子代理不是作业：`job` / `kill_task` 只管后台命令。持续交流只有一颗 `send_message({agent_id, message})`，父子共用：父→直接子（在跑就在它下一步读到，idle 就开下一轮），子→启动它的会话（父级 id 写在子代理的初始任务里）；兄弟之间、别的分页发不了。`list_agents` 是状态源。`interrupt_agent` 停本轮；要整个撤掉在 `/tasks` 里按 `x`。用户 Stop / 新会话会取消本会话子代理。

`tools` 里写 Dock 已挂上的名字（`bash` `read_file` `cordis_define` …），不是 DSH 包名。未挂上的名字在画布右侧显示「未挂载」。

---

## Overlay / 快捷键 / 底栏

| 面 | 行为 |
|---|---|
| `g`（输入框空、有目标、且当前没在生成） | 打开/关闭目标 overlay，可改标题、暂停、清除 |
| 提问 overlay | 听 `ask/pending`，和权限 overlay 同款。单选画圆点 `(○)` / `(●)`，多选画方框 `□` / `☑`。选中「其他」时下方展开**自由输入框**：跟输入栏同一套圆角边框，聚焦走 `prompt_border_active`、失焦走 `prompt_border`；**终端真实光标跟进框里**（只画反显方块的话输入法候选条会锚在旧位置，打中文整条飘走），字超出框宽时窗口右滑、光标始终可见。`←→` 在框里移光标（此时不切题）。**Esc 逐级退**：有字先清空 → 空着退出「其他」（单选把高亮退回第一项，多选取消勾选）→ 再按才拒掉整题。**Space**：单选里等于「选定」，和 Enter 一样记下答案并前进到下一题（不写 `picked`——写了会让按过空格的行都留着实心点，看着像多选）；多选里是勾选 / 取消勾选；停在「其他」上时草稿为空则勾选 / 取消，有字则照常打空格，所以勾上之后取消得掉。**`←→` 切题**：`→` 停在第一道未答的题上时，把当前高亮当答案记下再前进（`Ask::navigate` 被 `max_reachable` 夹着，否则按下去毫无反应，而抬头明写着「← → 切换」）；**最后一题上 `→` 不做事**，提交只归 Enter。`←` 只回看已答过的题，不重新作答 |
| 浏览器配对 overlay | 听 `gateway/pairing`。`/pair` 第一行开启或关闭监听；首次 Origin 请求弹出「允许浏览器连接？」；下列待批与已绑来源（Enter 批准 / `x` 拒绝或撤销） |
| MCP elicitation | 听 `mcp/elicit`。权限 / 提问 overlay 会抢前台（队列仍在）。表单逐步填：选项带「其他」、自由输入空内容闪「请输入具体内容」。URL 模式 Enter 开浏览器，等 `notifications/elicitation/complete` 或 Esc 取消 |
| 动态插槽 `Overlay::Slot` | `"tui.slots"` 登记的纯文本 pane（复用 Notice 布局）。Esc 关闭；↑/↓ 滚动并把规范化键名转给 `on_key`（`esc` / `enter` / `up` / `down` / `char:x`）。脚本 `open_slot` 或 slash `kind: slot` 打开 |
| `/cordis` | Notice：永久（磁盘）与会话（内存）插件一览 |
| 插槽 HUD | `hud: true` 的插槽每帧 `render` 第一行，live-look 进快捷键条，不替换 status bar |
| `/preset` 画布 | 左侧完整目录（当前 `"tools"`，右侧工具简介）、右侧本预设工具集。身份区列出本模式 `agents/` id。Enter 左加右删。名册 `n`/`d` 新建或复制默认写项目 `.dock/presets/<id>/`；改内置会写到 `~/.dock/presets/<id>/agent.yml` 覆盖。Esc 从画布回名册 |
| 覆盖层边框 | 右上角 `[✗]` 关窗（点它 / Esc）。**鼠标停上去提亮**（`text_primary` + 粗体，Grok close 按钮同款），移开复位灰色。浮动框（`/tasks` `/mcps` `/workflow` `/usage` `/help` `/resume` `/history` `/find` `/goal` `/settings` `/pair` 与 Notice 类）与全屏框（`/preset`、欢迎页会话 picker）共用同一支 chrome，全屏边框视图（子代理 / 后台任务）的 `[✗]` 同样跟随 |
| 欢迎页 | 空会话是 **Grok hero box**：圆角方框、左边 braille logo、右边 `Dock` + 版本 + 一句说明 + 菜单（新会话 / 恢复 / 退出）。窄窗改成框内上下叠。点击菜单行仍走原来的快捷键。 |
| prompt 底栏 | **右对齐**画在输入框底边：模型 · **当前推理协议** · **当前 Agent 预设名** · 权限（询问/始终允许）。协议紧跟模型（该模型 `api_backends` 里声明的那条，`/protocol` 现场切；切模型会重新播种它）；没选模型（id 为空）时不写协议，那时兜底的 `dock` 不是一个端点。计划模式加 `计划`；目标进行中或暂停加 `目标`。生成中输入框有字：`Enter:queue` / `Ctrl+Enter:send now`。空输入且生成中、没有排队：`Esc:cancel`。空输入且有排队：排队条钉在输入框上方（`#1` 正文 `[发送]`）；`Enter:send now` 立即发出最早一条，点 `[发送]` 发那一行；`Esc:edit` 把最新一条收回输入框改。**Esc 取消且本轮还没有模型/工具输出**时完整收回刚发送的正文和图片；约 1s 内再按 Esc 不会清空收回的草稿（Grok 双击 Esc 宽限）。有目标、输入框空、且没在生成时快捷键条加 `g:goal`。**输入框空、没排队、没在生成、且上一轮没有模型输出**时加 `Esc:undo`（见 `/undo`）。子代理发来消息续跑父会话时也算 working，期间发的消息会排队，不会被丢掉。取消或立即发送时会给未完成的 tool call 补上「已中断。」结果，避免下一条采样 400 |
| 目标状态条 | 有目标时钉在子代理头像和排队条之间（紧挨输入框上方）：标题 + `[暂停]`/`[继续]` `[修改]` `[关闭]`；第二行是进度备注和彩色扫光波。运行中随 80ms 刷新换色；暂停后冻结变灰。点标题打开 overlay，点 `[修改]` 直接改标题 |
| 任务条 | 未结束的耗时任务钉在输入框上方，**五类共用一条**：后台 bash、`monitor`、子代理、`/loop` 定时任务、workflow。一行一条：状态符（running `●` / idle 与排程中 `○`）· 描述（与 `/tasks` 同一套文案与配色，直接复用 `TaskEntry`）· 右对齐耗时 `m:ss` · 最新一行输出（宽度不够时先让位给描述与耗时）· 能停的行右端 `[✗]`（与 `/tasks` 相同，点它走 `kill_task_target`：Job / Subagent / Watcher / Workflow 均可）。最多 3 条，其余折成一行 `+N · /tasks`。**前台 bash 不进任务条**（turn status 那行已经在显示进度，每条快命令闪一下只是噪音）；任务结束即消失，结果本来就落在滚动区。点子代理行开 **Grok 同款全屏边框**：标题栏 + 子代理自己的滚动区（工具卡折叠循环与主界面相同：收起 → 截断 → 展开），框底可输入，Enter 经 `send_message`（urgent）发给该子代理，Esc / q（输入为空时）/ [✗] 返回；点后台任务行开该任务的全屏输出；workflow / 定时任务没有单独全屏视图，点击退回 `/tasks` 整表（workflow 的阶段 + 子代理详情归 `/workflow`）。键盘一律走 `/tasks`，任务条不占键位 |
| 分页（标签栏） | 一个终端里并排跑多个会话。`Ctrl+N` 新开一页并切过去，`Alt+1..9` 直达标签上那个号的页，点标签也能切，`/tab close [页号]` 关页（第一页是主会话，关不掉）。输入 `/tab ` 会在下拉里补全子命令（`new` / `fork` / `back` / `promote` / `close`）；打页号时下拉自动让开，直接回车提交。**只有一页时不画标签栏**；多于一页时钉在最顶上：`1● 主线 │ 3○ 读文档`——号是**稳定编号**（关掉中间一页，其它页不改号），`●` 跑着 / `○` 闲着，当前页加粗。每页各有自己的会话、轮次、agent 循环、会话 actor、滚动区、输入框、状态栏、欢迎屏，以及**目标、待办、计划模式、模型、协议、权限模式、权限队列和提问队列**（各页一份，底栏和弹层只显示当前页的；另一页在等批准时标签标 `◆`）。计划文件也按页划分。**工具表、LLM、MCP 连接、浏览器、cua、后台任务是全局一份**，两页会真的抢同一个。MCP elicitation 盖上来源页，只在那一页弹出。同一台 MCP 服务器两页可以同时调用，各页弹各页的框。`Ctrl+F`（或 `/tab fork`）从当前页**分叉**一页：新页带着当前页的上下文快照（`model_history`，压缩后的那份），标签上标 `⑂`，并记住来源页；分叉之后两边各写各的，谁都污染不了谁。`Ctrl+B`（或 `/tab back`）把分叉页最近一条**有正文**的模型回复，按 `（来自第 N 页）` + 整段引用填进**来源页的输入框**并切过去——只填不发，改不改、发不发都还是你说了算，来源页的历史一个字没动。`Ctrl+W` 语义不变（换掉**当前页**的会话），`Ctrl+D` / `Ctrl+U` 仍是半页滚动。`Ctrl+N` 开的新页是**空白**的，上限 9 页。空白页**不落盘**：`--resume` 与 gateway `dock.1` 投影仍只跟第一页，退出即丢其它页；从历史会话开出的那一页（`adopt_archived`）例外，它接着写回原来的会话目录 |
| `/btw <问题>` | **插一嘴**：从当前页分叉出**一张只读分页**并把问题发进去，然后切过去看答案。工具只有 `read_file` / `grep` / `list_dir` / `glob` —— 它和主线并发跑、共用工作目录，给写工具就是制造竞态；也没有 `search_tool` / `use_tool`，插一嘴不该能去点桌面。「不打断」指的是**主线那一轮照跑**（各页各自的会话与循环），答案也**不进主线上下文**；看完 `Alt+1` 回主线。标签上用 `?` 标（分叉页是 `⑂`）。`/tab close` 关掉；`/tab promote` 把它转正 —— 拿它已经聊出来的历史开一张**全权**常驻页，旁问那一页关掉。答案走这一页自己的滚动区，markdown、工具卡、流式渲染都和主线一样 |
| 鼠标框选 | **在滚动区或输入框里按住左键拖动即选中，抬手即复制**（flash「已复制」），高亮留到下一次按下。全屏应用把终端的原生拖选顶掉了（alt-screen + 鼠标上报，拖动事件进的是 dock），所以这一段是 dock 自己做的；两处共用同一支高亮。**输入框这一块永远归它自己**——欢迎页、overlay 盖在上面时，框里照样能拖选（框外仍是各自的点击）。输入框里没拖动的单击只是移光标；**选中之后按删除删的是整段、打字是替换**，挪光标才算作废 |
| 输入框边框 | 有焦点时亮，没焦点时暗（空输入框在生成中会失焦，那时 Enter 不抢跑，显示 `Build anything` 占位）|
| `Ctrl+t` | 切换待办卡片折叠：未完成（默认）→ 全部 → 一行概要。列表非空时快捷键条出现 `Ctrl+t:todo`；没有列表时闪「当前没有待办列表」 |
| 待办卡片 | 钉在滚动区末尾，live-lookup `"todos"`。头行 `◆ 待办  ◆◆◇◇◇◇◇◇ 2/7`（进度条 + 已关闭/总数），点头行或 `Ctrl+t` 循环折叠。默认只列未完成项（最多 6 行，超出 `… 还有 N 项`），已完成折成 `✓ 已完成 N 项`；「全部」连已完成一起列；「概要」只留头行并把进行中的标题带上。`todo_write` 调用本身画成 `◆ 待办 写入 N 项 ✓2 ▶1 □1` 卡片，展开是本次写入的条目，不铺 JSON |
| 滚动区 | 用户气泡灰带铺满行宽（Grok `with_background`），**上下各一行同色内边距**——一句话的提问也是三行体量，不然一行高的色带在页面底上分不出来，滚起来和模型输出糊成一片；`/timestamps` 的时钟跟正文行走，不落在内边距上。带色本身也比页面底亮一档（`bg_light`，三个主题都拉开 ≥20/255，浮层与斜杠下拉共用这一层）。待办图标抄 Grok `todo_pane`（`□` `▶` `✓` `✗`）。各类卡片：**`subagent`** 独立卡（角色名 + type id，始终折叠，点开全屏对话）；后台 bash / `monitor` 画任务卡；前台 `bash` 画 **Bash 卡**；`update_goal` → **Goal 卡**（设定 / 进展 / 完成 / 受阻）；`scheduler_*` → **Loop 卡**（设定 / 列表 / 关闭）；`search_tool` → **Search Tools** 卡（关键词 + 条数，展开是「动作  服务器」列表，不铺 JSON schema）；MCP 调用（`use_tool` 或遗留 `mcp_{server}__{tool}`）→ **Server Action** 卡（参数 kv + 输出，标题用内层 `tool_name`）。<br><br>**Bash 卡**：标题 `◆ Bash $ <命令>`，工具名粗体 + dim `$` + 命令，与 `Read` / `Glob` / `List` / `Search` / `Edit` 同构——原先只有一个 `$`，扫一眼分不出是什么卡。**相邻的多次前台 bash 合成一张**，标题 `◆ Bash N 条命令`，展开逐条列 `$ 命令` + 各自输出（空输出留「（无输出）」，读得出跑过没有）。中间夹任何别的事件就断开，合并的前提是它们视觉上本来连成一片；后台 bash 归任务卡（自带任务 id / 实时输出 / 时钟）、正在跑的调用要单独挂时钟，都不参与合并。**组内每条各自可折**：组头切整组（键 `bash-group:<首条 id>`），每条命令的 `$ 命令` 与正文归它自己的 tool call id，点它只折它——三条长输出可以只收掉一条。这是 grok 的形态（每条 execute 各是一个带 `DisplayMode` 的块），只是外观收进一张卡；每条缺省继承组的折叠态，单独点过才有自己的值。<br><br>**共用外壳**（`scrollback/card.rs`）：正文缩进 2 列（与 `◆ ` 等宽，左边缘和标题文字对齐）、换行宽度同一常量、截断脚注统一成「`… 还有 N 行`／`N 项`／`N 个文件`」、标题行末钉折叠符（`›` 收起 / `⌄` 还有更多 / `⌃` 全展开），**整张卡可点**，不只折叠头那行。打开覆盖层的卡片（子代理 / 后台任务）不折叠，写「（点击查看）」。<br><br>运行中的卡片**头行扫光**：高光带拖尾从左扫到右，把文字染成该卡状态色（工具青、思考紫），扫完停一拍再来，行尾右对齐实时耗时。扫光与耗时都在 **paint 期**画，不进布局缓存——卡片正文不含任何会自己变的内容，否则长跑 bash / 子代理干活时父会话不追加事件，烤进去的动画会直接冻住 |
| 顶栏 | 右上角 `上下文 {used} / {window} │ {协议}`（Grok `context_bar` 同款形态）：数字是**至多 4 字符**的紧凑计数（`121K` / `1.0M`，进位到头改用整数形，`9960` → `10K` 而不是 5 字符的 `10.0K`），颜色**按占用率在断点间渐变**（正常文本色 → accent_user → warning → accent_error，50/65/75/85/95 五个断点线性插值）——原先是 70/85 两档硬跳，而 85% 正好是自动压缩阈值，跳过去时已经没有预告余地了。**鼠标停上去**数字原地换成进度条 + `42.0%`；两种形态等宽（数字串先右补到 6 列），所以扫过去不会横跳、命中区也不会错位。末尾还有一个 `[Agents]` chip（Grok 同位置是 `[Dashboard]`），点开**会话面板** `/dashboard`；它有自己的命中区，压在右段最右端，判定时优先于占用率那段（否则点 chip 会连带把 `/context` 也开出来）。flash 期间不画 chip、命中区也清掉。前一段是当前窗口占用（系统提示 + 消息 + 模型可见工具定义 + 图片 + 推理；有官方 prompt 则取较大值），MCP extras 与按需本地工具不进采样工具表，也不计入占用；后一段是**当前推理协议**——该模型 `api_backends` 里声明的那几条之一，`/protocol` 现场切，切换时的 flash 过后也一直挂着（没选模型时不显示）。点击打开占用 overlay，再点分类看明细。不是 `/usage` 的会话累计账本 |
| `enter_plan_mode` / `exit_plan_mode` 卡 | scrollback：`◆ Plan: Enter\|Exit`；Exit 展开 markdown。`exit` 会 park 审批，写闸保持到用户决定 |

---

## `/usage` 数据

抄 Grok `UsageLedger` + `session_usage_block_text`，不接 `x.ai/billing`。

- 只把 SSE **官方** `usage` 折进账本（`prompt_tokens_details.cached_tokens`，缺省再认 `prompt_cache_hit_tokens` / `cache_read_input_tokens`；思考认 `completion_tokens_details.reasoning_tokens`）。开转前的本地估算只更新顶栏占用，不入账（Grok fail-closed：缺费用 ≠ 免费）。
- **缓存写在哪**：`chat_completions` / `responses` 靠上游自动前缀缓存；`messages` 必须显式打断点，由 `http/messages.rs` 的 `apply_cache_breakpoints`（抄 Grok）在 system 尾 + 对话 tip + 上一轮收尾处各打一个，第四个槽留给网关。`[model.<id>].prompt_cache = false` 可关（自建代理不认 `cache_control` 时）。
- **缓存占比** = 缓存命中 / 完整输入（Grok：`cached_prompt_tokens` 是 `prompt_tokens` 的子集，不要相减）。会话累计用总量相除，不是各轮百分比再平均。超过 100% 钳到 100%；输入为 0 显示 `-`。格式抄 Grok `/context` 的 `percent_of_window`（不足 10% 一位小数，否则整数）。
- **输入拆三段**：命中 / 写入 / 未命中，互不相交且加起来等于完整输入（`messages` 的 `input_tokens` 本不含前两项，解析时已加回）。overlay 里画成一条按 token 比例分段的 bar，分段靠字形（`█` 命中 / `▓` 写入 / `░` 未命中）而不只靠颜色，单色终端与截图里照样读得出。非零段至少占一格——四舍五入到 0 会让「有一小段白付了全价」从图上消失。写入为 0 不占图例行（`chat_completions` / `responses` 根本不报它，画一行 0 会被误读成缓存没生效）。
- **每轮走势**：账本留最近 40 次主循环调用，overlay 画成 sparkline（`▁`–`█`，旧 → 新）。刻度固定 0–100%，**不**按样本自适应——自适应会把一串都在 90% 上下的调用画成大起大落。只有一次调用不画。用途：命中率低时区分「这一轮新内容本来就多」和「前缀被改写了整段重算」（压缩、系统提示或工具表变化），只看 **上一轮** 一个数分不出来。
- **走势的横轴是「有输入的主循环调用」**，不是「模型调用」。三处口径不同：子代理走 `record_subagent`、压缩走旁路 `record_side_call`，两者都折进 `model_calls` 但不进 `recent_calls`；`input_tokens == 0` 的调用也不占格子（没有命中率可言）。所以 sparkline 旁边报的是**实际画出来的格数**，`模型调用` 一行在有子代理 / 压缩时拆成 `总数（主循环 N · 子代理 M · 压缩 K）`——不写明就会被读成走势少画了一格。压缩是单次命中率掉格最大的事件，且它之后那一轮必然全量重算；以前它既不记账也不占格，`/usage` 里一个 token 都看不见。
- **窄窗口裁最旧的**：数据旧 → 新排列，交给渲染层在右边截断等于丢掉刚发生的那几次，正好是最该看的。宽度不够时 `hit_rate_trend` 自己丢队首。
- 主循环每次 `finish_llm` 记一笔；子代理 isolate 结束时 `record_subagent` 折进父会话，不增加 `numTurns`；压缩走旁路 `record_side_call`，花钱但既不增加 `numTurns` 也不占走势格子（它不是用户的一轮）。
- **费用**：`CallCost` 三态 `Reported | Estimated | Unknown`，上游优先。`[model.<id>.pricing]` 是 USD / 百万 token 的四价位（input 指**未命中**、cache_read、cache_write 省略回落 input、output），按 `/usage` 的同一套分段计价，存 tick（1 USD = 1e10）避免浮点累积。**不内置厂商价格表**：价格变动频繁，过期单价会静默给出错误金额，和内置模型目录当初被删是同一个理由。估算值一律带「约」并注明来源，混合会话显示「部分上报 · 部分估算」——估算不含分时折扣（DeepSeek off-peak 半价），不能当账单读。四个价位全 0 / 负数 = 没配价，不拿 $0 冒充免费。
- `/new` / `clear` / `/resume` 清零账本。

---

## 仍比 Grok 薄（CLI 面）

- `/goal`：模型可用 `update_goal(objective)` 自己开目标；continuation 已接上（内循环 + 整轮结束后隐藏 GoalSummary）。尚未自动 spawn Grok 的 `goal plan writer` / classifier / strategist
- 一轮采样安全上限 256 步（Grok 默认不限 `max_turns`）；撞上限时滚动区留下说明，而不是静默停
- **LLM 请求失败**：滚动区画一条错误色的 `◆ 请求失败` + 详情。详情存在 `LlmOutput::error` 而**不是** `text` 里——三条 wire builder（messages / responses / chat）都以 `text` 非空或有 tool_call 为门槛，所以失败详情**不会被回放给模型**，也不会每轮重复付它的 token；但它跟着会话落盘，`/resume` 之后仍看得见上次为什么断的。**三条 wire 的流内错误也走这里**：provider 在 SSE 中途发的错误事件（messages 的 `{"type":"error"}`、responses 的 `error` / `response.failed`、chat/completions 的 `{"error":{…}}`）都只进 `error`。chat/completions 那条原先把解不成 chunk 的帧**无条件丢弃**，于是这一轮完全空白地收场——没有文本也没有报错，界面上像是应用坏了；现在认得出错误信封就记下来，认不出才丢。详情会走一遍错误的 `source()` 链（reqwest 的 `Display` 只印 `error sending request for url (…)` 这层壳，真正的 `connection reset by peer` / `dns error` / `certificate verify failed` 在链里），并带上分类前缀（`[连接失败]` / `[超时]` / `[传输]`）。HTTP 状态码错误另带 `request-id`（依次试 `request-id` / `x-request-id` / `cf-ray` / `x-amzn-requestid`）——**传输层失败没有响应，也就没有 request-id**，别在那种错误里找
- **重试**：传输层失败（连接重置 / DNS / TLS / 连接超时）与 HTTP 503 都重试，最多 3 次、800ms 线性递增，期间可被 Stop 取消。请求拼错（builder 错误）不重试。LLM 的 `reqwest::Client` 是**进程级共用**的（连接池复用，避免每次请求重做 DNS+TCP+TLS 握手），只设 20s `connect_timeout`、**不设整体 timeout**（响应是 SSE 长流，全局超时会把正常的长回答拦腰砍断）
- 提问 overlay：有 Other，自由输入比 Grok pager 简单
- `/usage`：占用 + 用量两 tab；无 grok.com 账号额度条、无 Session info
- `/compact`：Grok full-replace 一轮（structured 九段摘要 + 85% 自动；只换发给模型的历史，滚动区不擦）。无 two-pass / segments / `updates.jsonl` 旁路；模型前缀落 `compact.json`
