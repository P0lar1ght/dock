# 工具清单

对照当前 `install_app` 与 `cordis-spine` 源码，不是愿望列表。硬规则见 [AGENTS.md](AGENTS.md)，插件树与不变式见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)；本文件只记 **模型工具、插件粒、缺口、不要做的事**。斜杠命令、快捷键、overlay、底栏见 [CLI.md](CLI.md)。

核对：

```bash
cargo test -p cordis-spine --test round -- install_app_registers
```

`install_fakes` 是 echo，**没有**下表能力工具。不要拿 echo 测试当产品清单。

---

## 原则

1. **一切皆插件。** 新工具是一个（或一套）Cordis 插件，`inject: ["tools"]` 后 `ctx.tools.register()`。不要焊进 `event_loop` / `agent-loop`。
2. **一张 `"tools"` 表。** DSH 不是「一个工具名一个 ExtraTools key」。Grain 是套件：`tool-web` 同时 register `web_search` + `web_fetch`；`tool-todo` 只有 `todo_write` 因为那就是整套。MCP 是 **一个插件连一个 server**，工具仍 `register` 进同一张表，公名 `mcp_{server}__{tool}`，不能盖掉 `bash`。
3. **先复制 Grok，再改成 Cordis。** 工作只在 `dock/`。不要改、不要 path-dep `grok-build/`。Grok 已有逻辑就 `cp` 再剥依赖（`register_resource!`、`tracing`、schemars、xAI 账号 client）。
4. **Live-lookup。** 调用点 `ctx.get` / `ctx.require`。不要把 `Arc<T>` 关进长生命周期闭包。
5. **扩展走 waterfall**（`tools/execute`、`system-prompt/assemble`、`agent/turn-end`、`agent/step-start` 等），不要在 loop 里分支。「模型出了文本但还不该收尾」就是 `agent/turn-end`，「跑到一半要提醒一句」就是 `agent/step-start`，不是往 `grok_sample_loop` 里再加一个 `if` 或一份计数状态。三条 agent 面的 waterfall 子代理开轮 / 收尾时同样会跑，而 handler 只能拿到主会话的 `Sessions` —— 会写会话或消费一次性状态的，先看载荷里的 `identity`。
6. **MCP / 可选能力 fail-open。** 没配置或连不上时插件仍 Active，不要让 `install_app` 失败。
7. **不接 Grok 账号产品。** 登录、账单、分享、marketplace、Imagine / 视频生成、voice、dashboard 账号面：不做。核心 agent 能力（读改跑、搜网、todo、提问、计划、后台任务、调度、MCP）要补。本会话 token 账本（输入/输出/缓存/思考）不是账号产品，见 [CLI.md](CLI.md) `/usage`。
8. **产品面要跟上。** 新工具要有：register 进 specs（或 `register_deferred` 走 `search_tool`）、execute 路径、权限/计划门（该挡的挡）、人要看见的 slash / TUI（记 [CLI.md](CLI.md)）。用户可见文案中文；Grok 底栏 `Enter:send` 那种短 hint 保持英文无空格。
9. **工具表的顺序是缓存面，不是审美。** `specs_for_model_on` 把「整份名册里每个预设与角色都允许的工具」排在前面，其余（会被某个角色过滤掉的、`task` 这种 schema 随名册改写的、MCP / 按需的）排在后面；组内顺序不变（内置在前、其余按名字）。排序键只看名册、与当前预设无关，所以**子代理那张表是主会话那张的真前缀**。tools 排在整份 prompt 最前面，中间少一项就会让后面整段前缀作废，子代理每次冷启动都得把公共头重付一遍。子代理 spawn 时经 `AgentPresets::overlay_with_order` 继承父会话的排序依据——`overlay` 只带一个角色预设，自己算出来的分组和父会话对不上。
10. **测的是树，不是 stub。** 清单以 `install_app` 挂上的插件和 `Tools::specs()` 为准。不要留一个同名但走另一套 end state 的假实现。

---

## 已有（`install_app`）

挂载顺序见 `cordis-spine/src/bundle.rs`。能力插件都在 `workspace_tools` 之后、`llm` 之前；`compact` 在 `llm` 之后（要注入 `"llm"`）。

| 插件 | ctx | 模型工具 | 备注 |
|---|---|---|---|
| `tools`（`workspace_tools`） | `"tools"` | `list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file` | 工作区内建，不能被 register 盖掉。`bash` 前台预算 **5 分钟**（`DOCK_BASH_FOREGROUND_MS` 覆盖，对齐 Grok `GROK_MAX_FOREGROUND_BLOCK_MS`——Grok 默认 30s，这里放宽是因为一条 `cargo clippy` 就 1 分钟起，30s 到点只会逼模型反复起后台再轮询）；**到点与取消都把已产出的输出一并返回**，不再只回一句错误 |
| `jobs` | `"jobs"` | — | 进程表。bash `is_background` / `block_until_ms: 0` 用它，**前台 bash 也在这张表上**（`foreground: true`，只为让 TUI 边跑边读输出；不进 tasks pane、不进 `get_task_output` 的无参列表，结束即摘掉）。stdout / stderr **并发抽干**——顺序读会在任一侧写满 64KB 管道缓冲时把子进程永久堵死。输出流式累积，上限 20KB（头 4KB + 尾 16KB，中间截断并在正文标明省略字节数），对齐 Grok `output_byte_limit` |
| `slash` | `"slash"` | — | 额外斜杠命令表；TUI live-lookup。内建 `CATALOG` 不能被盖掉。`kind`：`prompt` / `overlay` / `slot`（打开已登记的 `tui.slots` id）/ **`tool`**（`text`=工具名，直接 `Tools::execute`，结果 Notice；权限门仍生效） |
| `skills` | `"skills"` | — | 发现 `SKILL.md`（`$DOCK_HOME/bundled/skills/` 内置 < `{cwd}/skills/` < `~/.dock/skills/` < `{cwd}/.agents/skills/` < `{cwd}/.dock/skills/`，后者同名覆盖；内置是编译期嵌入、启动物化到 bundled 缓存的 dock 自述技能，通用技能不进二进制）。frontmatter：`name`（出厂技能须合法且与目录同名；运行时非法或缺省即回退目录名，仍非法则丢弃该技能）`description`（出厂技能必填 ≤1024；运行时缺省取正文首行、再用 name）`license` `when-to-use` `paths` `user-invocable`（缺省 true）`disable-model-invocation`（缺省 false）。`license` 只在 `/skills` overlay 的路径行展示。向 `"context"` 登记 listing 段（窗口 token ×4 ×**3%**，不要把全文塞进系统提示；预算与渲染规则与工作流共用 `src/listing.rs`）。两道门（`listing::wants_listing`）：**只有主会话拿这一段**，子代理要在 `agents/<type>.yml` 写 `listings: true` 才带（内置只有 `general-purpose` 打开）；且 **header 点名的 `skill` 必须对本会话可见**（在当前预设 allowlist 里且已注册），否则整段不发——`warden` 主代理没有 `skill`，给了也只是诱导一次必然被 allowlist 挡回的调用。`agent/pre-step` 把用户气泡 `/name args` 注入 `SystemReminder`。带 `paths:` 的技能渐进披露：匹配文件被触碰前不进 listing，`tools/execute` 触碰后激活并 `SystemReminder` 通告（listing 首帧冻结不回写，与 Grok 同款）。`tools/execute` 路径靠近 skills 目录时中途发现。`user-invocable` 技能登记成 slash extras（不可盖 `RESERVED_SLASH`）；另登记 `/skills` overlay。fail-open |
| `project-instructions` | → 挂 `agent/step-start` | — | 把工作区工程规约读进**历史尾部 `<system-reminder>`**，**不进系统提示**：`~/.dock/AGENTS.md`（用户层）+ `{cwd}/AGENTS.md`（项目层，追加不覆盖）。`AGENTS.md` 是仓库内容（clone 陌生的仓库，那份文件就是陌生人写的），系统提示是 harness 自己的声音，所以它降级成「工作区提供的数据」：注入前按 Grok `neutralize_reminder_tags` 中和内容里的 `<system-reminder>` 变体（大小写 / 带斜杠 / 带空格都算），来源标注（`## AGENTS.md`）由 harness 写、不被文件内容顶掉。槽位 `ORDER_STEP_START_INSTRUCTIONS = 5`，排在所有提醒最前——它是这一步的规则，其余提醒是在这些规则之下的催办。**一条判据覆盖四种情况**：渲染出来的规约与本会话历史里最近一份不一致才注入，认 `<system-reminder>` + `# 工作区工程规约` 开头（会话开始 / 中途改文件 / 压缩吃掉旧副本 / `/resume` 回放旧版）。只追加、不原地改写，前缀一个字节不动。预算窗口 token ×4 ×5%，**只管文件内容**（框架行必须完整，否则副本认不出来、标签闭合不上），超出从尾部按字符边界截断并附说明。**只读工作区根目录**，子目录嵌套 `AGENTS.md` 不读（优先级与预算都不好界定，要看用 `read_file`）。文件缺失 / 读失败 / 全空白都 fail-open（插件仍 Active，不贡献提醒）。子代理跑隔离 ctx，按**它自己的**历史与窗口判断该不该注（handler 拿不到调用方 ctx，走循环挂的 task-local）。`/context` 单列一行 **「工程规约」**，数是**实际注入的那份**，不是磁盘当前内容 |
| `tui.slots` | `"tui.slots"` | — | 动态包登记的 TUI 插槽（数据+回调，不是 ratatui widget）。`install_app` 始终挂上。TUI 用一次通用 `Overlay::Slot` 臂 |
| `agent-presets` | `"agentPresets"` | — | 组装 Agent：YAML 定义人设 + 工具允许名单，运行时只过滤 live `"tools"`。**正在运行的动态包**用 `Tools::register_dynamic` 登记的 extra 工具、以及 **已开启的 MCP 工具**（`register_mcp`，公名 `mcp_{server}__{tool}`，经 `use_tool` 调度）会穿过允许名单。层：crate `presets/<id>/agent.yml` + `agents/*.yml` < `~/.dock/presets` < 项目 `.dock/presets`。仍可读旧 `<id>.yml`（目录优先）。加一个目录就是一个 Agent。内置 `code` / `minimal` / `cordis` / `warden`（守望）。`code`/`cordis` 的 `agents/` 名册是 `general-purpose` / `explore` / `plan`（项目层可加，如 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`）；`warden` 是 `岑` `锁` `甲` `乙` `丙` `衡` `验` `观` `突击`（不要用拼音 id）。发给模型的 `task` 把 `subagent_type` 收成当前名册 enum，并在 description 尾部追加 `subagent_role_hint()`（角色 id + 显示名 + **角色说明** + 写路径 + `reload_roster` 用法）。**系统提示不再有名册段**：它原先是 `task` / `send_message` / `report` / `interrupt_agent` description 的中文重写，逐条重复，已整体删除（`ORDER_ROSTER` 一并撤掉）。省略 `tools` = 全部已注册工具。新建模式默认写**当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` n/d 有项目层时落到这里；`task` description 注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`；id 必须 `[a-z0-9][a-z0-9-]*`，汉字目录只叠内置）。新建子代理默认写 `.dock/presets/<当前模式 id>/agents/<type>.yml`。空名册仍注入这两处路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后 `task` 校验立刻重读；本轮刚写完时用 `task`（`reload_roster: true`）刷新 enum。新建模式写完后用 `/preset` 应用该 id。改 crate `presets/` 要重新编译 |
| `tool-web` | → `"tools"` | `web_fetch` `web_search` | Grok SSRF / 同 host 重定向 / htmd。`web_search` 无 xAI 账号，走同一套 fetch 打公开 HTML 索引 |
| `tool-browser` | `"browser"` + `"tools"` | `browser_open` `browser_navigate` `browser_navigate_back` `browser_snapshot` `browser_click` `browser_hover` `browser_type` `browser_press_key` `browser_select_option` `browser_fill_form` `browser_wait_for` `browser_drag` `browser_handle_dialog` `browser_file_upload` `browser_resize` `browser_evaluate` `browser_console_messages` `browser_network_requests` `browser_screenshot` `browser_tabs` `browser_close`（按需） | **BUA P2**：in-process **chromiumoxide** CDP。P1 之外增加 `browser_evaluate`（**权限门同 bash**：`needs_permission` + `blocked_in_plan`，经 `tools/execute` / `use_tool` 命中）、只读截断的 `browser_console_messages` / `browser_network_requests`（会话连接时挂 Network/Runtime 监听，保留最近 N 条）、以及 **同域 iframe**：`browser_snapshot` / `browser_evaluate` / `browser_click` 可选 `frame`/`frame_selector`（CSS 选 iframe）；跨域或找不到则明确报错。无 Node/Playwright；CUA 像素点击仍不做。`register_deferred`：不进 sampler / `specs_for_model`。Fiber dispose 关掉 Chromium。`browser_screenshot` 写路径给 `/browser`，并经 `ToolResult.images` 进多模态（见「工具结果图」）。`/browser` 驾驶舱列 P0–P2 + 最近 evaluate/network，并可 **`h` 切换有头/无头**（`[browser].headed`，默认无头；`DOCK_BROWSER_HEADED` 任意非空覆盖；切换后需 close/open 才作用于已开会话。有头 launch：`with_head().viewport(None)` + 初始 `window_size`，避免默认 800×600 Emulation 只画一角；Chromium CLI `.arg` 勿带前导 `--`（库会再加））。`code` / `cordis`（+ general-purpose）允许名单含这些 `browser_*`；`minimal` / `warden` 主代理不含 |
| `tool-computer` | `"computer"` | — | **CUA C0**：薄驾驶舱 named `"computer"`。桌面键鼠经 trycua `cua-driver` MCP（公名 `mcp_cua-driver__*`，现有 `mcp-client`），**不**自研键鼠 / Docker。live-lookup `"mcp"` 看 cua-driver 是否就绪；`/computer` 为 TUI CATALOG builtin（`Overlay::Computer`）。全部 `mcp_cua-driver__*` 与 bash 同级 permissions / 计划门。挂在 `mcp-client` 之后；fiber dispose 注销。见下文「Computer / CUA」 |
| `tool-todo` | `"todos"` + `"tools"` | `todo_write` | Grok merge/replace，含 Grok `effective_merge` 自动升级：`merge:false` 但每一项都只带已存在 id + status 时按合并处理，不会把 content 抹成 id。工具结果在列表后追加 `n/total done` 与状态提示（没有 in_progress / 多于一个 in_progress / 全部关闭）。`Todos` 另外暴露 `stats()`（TUI 折叠条）/ `revision()` / `gate_reminder()`。注册两条 waterfall：`agent/turn-end`（`ORDER_TURN_END_TODO = 10`）出文本收尾但还有未完成项时续跑一轮，每条用户消息最多 2 次（`keep_working_with` 只在真的胜出时扣），配额在主会话的 `agent/pre-step` 清零，被 live job / running subagent 托底的 `in_progress` 不算；`agent/step-start`（`ORDER_STEP_START_TODO = 10`）列表连续 6 步没动且仍有未完成项时中途提醒勾选 / 调整，每轮最多 3 次，watchdog 在 `step == 0` 重置。**注意 `"todos"` 不随子代理 isolate**，两条都靠载荷里的 `identity` 判断，只在主会话生效 |
| `plan-mode` | `"planMode"` + `"tools"` | `enter_plan_mode` `exit_plan_mode` | 计划文件 `.dock/plan.md`。计划态挡住 bash / 写文件等 |
| `tool-ask-user` | `"ask"` + `"tools"` | `ask_user_question` | 事件 `ask/pending` |
| `tool-jobs` | → `"tools"` | `get_task_output` `wait_tasks` `kill_task` | 查/等/杀后台 bash **或子代理**（同一 id 空间）。`timeout_ms` 是**本次调用愿意等多久**，不是任务寿命：等到任务完成、或等到点返回当前快照（`[running]` + 已产出输出）——**不设上限、不中止任务**，长任务下次调用接着查。等到点仍未完成时正文补一句「这是快照不是结论」，免得 `[running]` 被当结果读掉；只有仍在跑的是子代理才加「等它推回合结束」那半句（bash / monitor 不推通知）。`get_task_output` 的 `timeout_ms: 0` 或省略 = 不等待的即时快照，`wait_tasks` 省略时默认等 30s |
| `tool-scheduler` | → `"tools"`（live `"cron"`） | `scheduler_create` `scheduler_list` `scheduler_delete`（按需） | 包着已有 `"cron"`。`register_deferred`。`fire_immediately` 立刻跑第一次；循环 7 天后过期（过期不跑最后一次，滚动区留中文说明）；最多 50 条；`task_id` 原地更新并保持相位。滚动区 **Loop 卡**（设定 / 列表 / 关闭）。`/tasks` 里 `x` / `[✗]` 关闭 |
| `tool-task` | `"subagents"` + `"tools"` | `task` `send_message` `list_agents` `interrupt_agent` `report` | Grok `ChannelBackend` + coordinator actor；dock `ChildRunner` isolate `"sessions"`+`"turn"`+`"agentPresets"`。提供 named `"subagents"`。**只有一个 spawn 工具**：`task`。`subagent_type` = 当前模式 `agents/<id>.yml` 角色 id（模型侧参数 enum 即这份名册）。参数只有 prompt / description / subagent_type / run_in_background / resume_from / reload_roster；`cwd` / `isolation` / `model` 已删（dock 不实现，别再加回来当摆设）。子代理**每轮结束**都会 park idle 并把回合结束推到父级（`<system-reminder>`，未 `report` 时带上回合正文）——后台 spawn 不必轮询 `get_task_output`；前台 spawn 内联拿到结果时会消费掉自己那条通知。`send_message`：idle 时 queued 与 urgent 都立刻开下一轮并在返回前把状态打成 running；urgent 只在 running 时才是 send-now。`report` 是子代理和主代理的多轮通道（同轮可多次）。`kill_task` 对子代理有效（dispose），只想停本轮用 `interrupt_agent`。parent session id = `session::ROOT_IDENTITY`，所以用户 Stop / 新会话会取消本会话子代理，下一次 prompt 重新开放准入 |
| `tool-memory` | `"memory"` + `"tools"` | `memory_search` `memory_get`（按需） | 本地 `~/.dock/memory` / `.dock/memory`。`register_deferred`：不进 sampler，经 `search_tool` / `use_tool` |
| `tool-monitor` | → `"tools"`（live `"jobs"`） | `monitor`（按需） | 长命令 stdout 盯梢。`register_deferred` |
| `tool-goal` | `"goal"` + `"tools"` | `update_goal`（按需） | Grok oneshot ack + drain。`objective` 可在无 `/goal` 时由模型自己开目标；无目标且只有 message/completed 时仍 `HarnessDisabled`。进度卡在滚动区。注册 `agent/turn-end`（`ORDER_TURN_END_GOAL = 20`）：目标没 completed 就续跑，最多 64 轮，用户已排队下一条时让路；`LoopHandle::continue_goal`（GoalSummary 入口）复用同一份 `continuation_reminder`，不走 waterfall。`register_deferred` |
| `tool-lsp` | `"lsp"` + `"tools"` | `lsp`（按需） | Grok `LspManager`/`dispatch`。第一次调用时读 `~/.dock/lsp.json` 与 `<cwd>/.dock/lsp.json`（项目盖用户）；没有配置则按工作区标记探测 PATH 上的 `rust-analyzer` / `typescript-language-server` / `gopls` / `pyright-langserver`（标记可在子目录，跳过 `node_modules` / `target`）。`/lsp` 把缺的服务器写入项目 `.dock/lsp.json`（不覆盖已有条目）；`/lsp user` 写 `~/.dock/lsp.json`。`search_replace` / `write_file` 之后后台 `didChange`。相对路径按 cwd 展开。没服务器时 fail-open（工具仍注册，调用返回配置说明）。`register_deferred` |
| `tool-skills` | → `"tools"`（live `"skills"`） | `skill` | 按需读 `SKILL.md` 正文（去 frontmatter）+ `$ARGUMENTS` / `$SKILL_DIR`。参数 `name` 必填、`args` 可选。返回 skill 信封和同目录最多约 10 个附属文件名。`disable-model-invocation` 的技能不进 listing / 本工具，斜杠仍可用。带 `paths:` 的技能激活前本工具也拒载（提示 gated on paths），斜杠不受限。没 skill 目录时工具仍注册。`code` / `cordis` 允许名单含 `skill`。**`register`（进 sampler）**——系统提示的技能 listing 直接点名这个工具，不能再让模型先 `search_tool` 绕一圈 |
| `tool-workflow` | `"workflows"` + `"tools"` | `workflow` | Grok Rhai 引擎（`vendor/xai/workflow`）+ 同款 oneshot ack。Host `SpawnAgent` live-lookup `"subagents"`，并把 `output_schema` 拼进子代理 prompt（JSON 输出会解析给脚本）；`model` / `effort` / `capability_mode` / `max_output_tokens` 被忽略（只记 debug 日志）。内置 `deep-research` 脚本在 `cordis-spine/src/workflow/workflows/deep_research.rhai`；磁盘扫描 bundled → 内置 → `{cwd}/.dock/workflows/<name>.rhai` → `~/.dock/workflows/`（同名不覆盖已有）。向 `"context"` 登记 listing 段（窗口 token ×4 ×**3%**，与技能共用 `src/listing.rs`）。同样两道门：**只有主会话拿这一段**（同 `listings: true` 开关），且 `workflow` 工具必须对本会话可见才发。每个目录项登记 slash extra（`kind: tool`，`text=workflow`；不可盖 `RESERVED_SLASH` / `/skills`）。`/name` 与 `/workflow <name>` 直接 `Tools::execute`，不经模型。`tools/execute` 路径靠近 workflows 目录时中途发现。`code` / `cordis` 允许名单含 `workflow`。**`register`（进 sampler）**——理由同 `skill` |
| `mcp-client` | `"mcp"` + `"tools"` | `search_tool` `use_tool` | stdio + Streamable HTTP。先走 MCP `2026-07-28`（无 initialize / 无 session，`_meta` + `MCP-Protocol-Version`）；服务器仍是 initialize 时代则回退 `2025-11-25`。fail-open。MCP 工具 `inputSchema` 原样 `register_mcp`，公名 `mcp_{server}__{tool}`，与 `register_deferred` 的本地工具一起 **不进** sampler `specs_for_model`。模型只看见静态描述的 `search_tool` / `use_tool`（`code` / `cordis` / `warden` 允许名单含这两项，`minimal` 不含）。`search_tool` 用 **BM25**（`bm25` crate，`Language::English`，标识符按 `__` / `_` / `-` / camelCase 拆词进文档与查询）匹配组/本名/描述/入参名，只返回命中项（每项完整 `input_schema`），limit 默认 5、最大 255；`total_hidden_tools` 是目录总数。零命中回退子串扫描，仍为零时 note 指出下一步（目录是英文，换 server / 能力名再搜）。**不重复发 schema**：命中的工具如果 schema 已在本会话模型历史里（`Sessions::model_history()` 扫既往 `search_tool` 结果，压缩后自动失效），该行回 `schema_in_context: true` 而不带 `input_schema`；**精确工具名搜索永远回完整 schema**，是重复搜索的逃生舱。输出另有 20KB 预算（按命中行计，JSON 外壳不计）：schema 先花掉其中 3/5，之后的命中降级成 `schema_omitted: "budget"`（只剩名字和短描述，仍可直接 `use_tool`），预算耗尽才整条不发、并在 note 里说清丢了几条。保住 `limit` 255 的 parity，又不至于一次把整个目录灌进历史。`use_tool` 调度 MCP 或按需本地工具，输出 20KB 帽，超帽部分落 `$DOCK_HOME/tool-output/<call_id>.txt` 并在截断提示里给路径（写盘失败退回纯截断）。第一类工具误走 `use_tool` 会纠正。`/mcps` Space 立刻 `dispose` 注销或追加注册，不改系统提示；目录变化在 `agent/pre-step` 与开关时写服务器级 `<system-reminder>`（已连接/已更新/已断开 + 数量，不含 schema）。`tools/list` 跟 `nextCursor`（最多 64 页）；`notifications/tools/list_changed` 50ms 合并后重列。advertise `elicitation.form` + `elicitation.url`；stdio 读循环 / HTTP POST SSE / GET SSE 收 `elicitation/create`，**服务器请求交给单独任务处理**（回复 stdio 直接写、HTTP 走 `reply_tx`）—— 内联 await 会让读侧一直等到用户填完整张表单，期间连本次 `tools/call` 的结果都读不到。单次 JSON-RPC 有兜底超时：默认 600s，`DOCK_MCP_CALL_TIMEOUT_SECS=<秒数>` 覆盖（`0` = 不限），超时/放弃时按规范发 `notifications/cancelled`。工具执行本身由 agent 循环与 `TurnControl` 赛跑，所以 `[stop]` 对 MCP 调用同样生效，不必等服务器回话。HTTP GET 长连接；initialize 时代 `Mcp-Session-Id` 的 POST 404 会重新握手再试一次。HTTP 服务器 `i` 浏览器 PKCE OAuth（DCR 或 `oauth.clientId`），token 在 `~/.dock/mcp_credentials.json`，启动不自动开浏览器 |
| `dynamic-runner` | `"dynamicCordisRunner"` | — | 会话注册表 + 磁盘永久插件。热挂体走 `ctx.plugin` / `fiber.dispose`。会话定义盖章 `Sessions::identity()`；磁盘插件 `session_id` 为 `*`，所有会话可见。`cordis_promote` 写 `{cwd}/.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/`（目录名即 pluginId）；`install_app` 自动加载（fail-open，不走权限 overlay） |
| `compact` | `"compact"` | — | Grok 会话压缩。`install_app` 在 `llm` 之后挂。手动 `/compact [说明]`；上下文达到窗口 85% 时 `maybe_auto`（loop live-lookup，工具轮次结束后、下次采样前）。摘要 prompt / 清洗 / 阈值从 grok-build `xai-grok-compaction` 拷来。成功后 **滚动区保留原对话**（Grok pager 也不擦 scrollback），只把 sampler 历史换成摘要前缀；占用数字按模型历史计。占用 overlay 是 TUI live-look `"context"`，不是模型工具 |
| `tool-cordis` | → `"tools"`（inject `"dynamicCordisRunner"` + `"context"`） | `cordis_*`（按需） | 预置工厂 `echo` / `note` / `hold` / `slash`，加上 `factory: "rhai"`（`source` 在 define 时 compile，run 时 eval `apply`）。`register_deferred`。系统提示只留短指针（`search_tool` 查 cordis）；教程在 `skills/cordis-plugin-development/SKILL.md`。`host.on` 可挂三个事件：`session/event`（事后观察）、`agent/step-start` 与 `agent/turn-end`（拦截，返回 `<system-reminder>` 正文或 `()`，宿主代跑 `next`、脚本吞不掉链，order 槽 50 排在内建 todo(10) / goal(20) 之后；用户已排队下一条时宿主直接不问脚本；抛错或返回非字符串都当没意见）。`cordis_call` 经 `Tools::execute` 试调任意 live 工具。`cordis_inspect` `what`: `services` / `builtins` / `events` / `slots` / `temporary` / `permanent`。`inspect_self` 对 Rhai 包回传 source。用户文本 `@pluginId` 在 `agent/pre-step` 注入身份 reminder（不含源码）。审批走权限 overlay（`cordis_run` `cordis_promote`）。`/cordis` 列出内存与磁盘层。Skill：`/cordis-plugin-development` 或 `skill` 工具加载 `skills/cordis-plugin-development/SKILL.md` |

`lsp.json` 例（`~/.dock/lsp.json` 或项目 `.dock/lsp.json`；也可 `{ "lspServers": { … } }`）。没写时按工作区标记探测 PATH：

```json
{
  "rust-analyzer": {
    "command": "rust-analyzer",
    "extensionToLanguage": { ".rs": "rust" }
  }
}
```

权限门（询问 overlay）：`bash` `search_replace` `write_file` `scheduler_create` `kill_task` `monitor` `cordis_run` `cordis_promote` `browser_evaluate`，以及全部 **`mcp_cua-driver__*`**（trycua `cua-driver` MCP，与 bash 同级）。

计划门：同上（`enter_plan_mode` 之后这些返回 blocked，直到 `exit_plan_mode`）。例外：对 `.dock/plan.md` 的 `search_replace` / `write_file` 自动放行（对齐 grok）。

---


### Browser iframe 策略（BUA P2）

- **默认**：`browser_snapshot` / `browser_evaluate` / click·type 等操作在**主文档**（current main frame）。
- **进入同域 iframe**：在 `browser_snapshot` / `browser_evaluate`（及 click 的文档对称参数）上传可选 `frame` 或 `frame_selector`（CSS，指向 `<iframe>`/`<frame>`）。实现用主文档 `querySelector` 探测 + `Page.getFrameTree` 匹配 CDP `FrameId`，再对 evaluate 设 `Runtime.evaluate` 的 `contextId`，对 snapshot 传 `Accessibility.getFullAXTree.frameId`。
- **跨域 / 找不到**：探测读 `contentDocument` 失败或树里匹配不到时，**直接失败并返回明确错误**（例如 `cross-origin iframe (CDP cannot enter)` / `no element matching frame_selector`）。不做 OOPIF 像素点击，也不静默落到主文档。
- **refs**：framed snapshot 产生的 `@eN` 只对该 frame 有效；click 前应使用同一 `frame_selector` 拍到的 snapshot。


### 工具结果图（多模态，#14 / #15）

- `ToolResult` 可带 `images`（落盘 `$DOCK_HOME/tool-images/`，cap 张数/体积）。`browser_screenshot` 与 CUA 截图类 MCP（经 `format_call_result` / `promote_cua_fields`）在路径之外把像素塞进下一轮采样；**同轮多个 tool 先齐结果，再统一挂图**（避免 Chat 线 HTTP 400）。
- 路径字符串仍给 `/browser` / `/computer` 驾驶舱；TUI **不**嵌真图。
- 读盘限 `$DOCK_HOME`；模型不接图时 degrade 为占位文本。`register_deferred` / 权限门不因产图回退。
- MCP `structuredContent` / `structured_content`：**非图片字段**（如 CUA `list_windows` 的 `window_id`/`bounds`）序列化追加进工具结果文本（默认截断约 8KiB）；`png_base64` 等大图字段仍只走 `ToolResult.images`，不进文本。不影响 deferred / 权限门（#26）。

### Computer / CUA（`cua-driver` MCP，C0）

控本机桌面走 **trycua [`cua-driver`](https://github.com/trycua/cua)** MCP，**不**自研键鼠、**不** Docker / cua 云沙箱、**不** path-dep 进 spine。

- **接入（零配置）**：Dock 启动时自己找本机 driver —— `DOCK_CUA_DRIVER`（绝对路径）→ `PATH` → `~/.local/bin/cua-driver` → macOS `/Applications/CuaDriver.app/Contents/MacOS/cua-driver`。**找得到就注入一条内置 `[mcp_servers.cua-driver]`**（`args = ["mcp"]`、`enabled = true`、绝对路径当 command），不用写 `config.toml`；找不到就当没这条，`/mcps` 不会多出一条死行。配置文件里的同名行**整条覆盖**内置行（连 command 一起），`DOCK_CUA_DRIVER=off` 彻底关掉内置行（测试与 CI 用这个）。公名 `mcp_cua-driver__{tool}`（服务器键名必须是 `cua-driver`），与其它 MCP 一样 **不进** sampler / `specs_for_model`，经 `search_tool` / `use_tool`。
- **不打包 driver**：Dock 的 release **不带** driver 二进制。macOS 上它是 trycua 签名的 app（`com.trycua.driver`，69MB），Accessibility / 屏幕录制授权绑那份签名身份 —— 拷进 Dock 的产物里重签会让授权失效，也等于重分发别人的公证产物。`/computer` 按 `i` 走的是**官方安装脚本**（`https://cua.ai/driver/install.sh`，下载后 `bash <tmp> --no-modify-path`），不改用户的 shell rc；装完自动重探 + `Mcp::reload`，不用重启 dock。
- **搜不到桌面工具时**：`search_tool` 的空结果 note 会点名 —— 桌面类关键词命中且 cua-driver 没连上时，明确告诉模型「去 `/computer` 按 i 装」，免得它只回一句没有这个能力。
- **stdio 帧**：Dock MCP stdio **默认 NDJSON**（每行一条 JSON-RPC，对齐 cua-driver 0.24+；0.28.1 实测仍是 NDJSON，0.28.0 的「modern stdio MCP」没有改默认帧）。LSP `Content-Length` 仅显式 `framing = "content-length"`。可选 `framing = "auto"`：先 CL 探测，`-32700`/parse 则 **kill+respawn** NDJSON（不在同一 stdin 硬切）。无需 Python 桥。
- **元素寻址（`element_token`，优先于坐标）**：`get_window_state` 走一遍 AX 树，同时给出 `structuredContent.elements`（每项 `element_index` / `element_token` / `role` / `label` / `value` / `actions` / `frame` / `parent_index` / `depth`）与向后兼容的 `tree_markdown`。8 个工具接受 `element_token`：`click` `double_click` `right_click` `scroll` `press_key` `type_text` `set_value` `hotkey`。token 格式 `s{snapshot_id:08x}:{element_index}`，**按 (pid, window_id) 作用域，下一次同窗快照即替换**——driver 的不变式是「每轮、每个 (pid, window_id) 先 `get_window_state` 再做元素动作」，过期 token 明确报 `element_token is stale`。走 token 的好处 driver 自己写在 `click` 描述里：对后台 / 最小化 / 隐藏 / 不在当前 Space 的窗口有效，不移光标、不抢焦点，且能回报点的是什么（role + label）。只有 canvas / video / WebGL / 自绘表面（不进 AX 树）才退回 `x, y`。AX 树不可靠时返回 `degraded_reason: ax_window_unresolved`，那种情况按像素点。
- **截图会吃图片配额**：`get_window_state` **默认同时返回截图**，而 `tool_images.rs` 的 `MAX_TOOL_IMAGES = 5` 是每轮硬上限。纯重新索引时传 `include_screenshot: false`（便宜路径，只要树）；只要预览不要树则 `include_accessibility_tree: false`（AX walk 是贵的那半，最长 20s）。两个都 false 是错误。大树（Electron / Obsidian 10k+ 元素）用 `max_elements` / `max_depth` / `query` 收口；缺省是 ≤2000 元素、深度 ≤25。`capture_mode` 已废弃且被忽略。
- **权限 / 计划门**：所有 `mcp_cua-driver__*` 与 `bash` 同级（`needs_permission` + `blocked_in_plan`）。`use_tool` 内层 `execute` 会命中该门。**已知缺口**：权限摘要是 `tools.rs` 里的 `format!("{} {}", call.name, 参数前 120 字符)`——CUA 调用的 `target` 样板会把真正要审的动作挤掉，且 `element_token` 对人不可读，弹窗实际上无法判断点的是什么。
- **Allowlist**：MCP extras 仍按现规则 **穿过** Agent preset allowlist；但 `code` / `cordis`（含 general-purpose）须保留 `search_tool` / `use_tool`。`minimal` / `warden` 主代理不含这两项则调不到 cua-driver。
- **勿混 BUA**：`cua-driver` 自带的 `browser_*` MCP 工具 ≠ Dock chromiumoxide `browser_*`。网页自动化优先 Dock BUA；桌面键鼠 / 开应用走 cua-driver。
- **Linux 坑**（写进安装说明）：需要 **X11 或 XWayland**（原生 Wayland 仍预览）；`DISPLAY` / `XAUTHORITY`；`at-spi2-core`（+ 必要时 toolkit-accessibility）否则 AT-SPI / `get_window_state` 弱；把 `~/.local/bin` 放进 `PATH`，或用 `cua-driver mcp-config` 给出的绝对 command；telemetry 默开，可 `cua-driver telemetry disable`。
- **TUI**：`/computer` 驾驶舱带状态机（未挂载 / 未安装 / 缺授权 / 已禁用 / 未连上 / 已连接）与两个动作键：`i` 安装或重装 driver、`p`（macOS）跑 `permissions grant`，`Ctrl+R` 重新探测并重载 MCP 配置。两个动作都是**两步**：先在确认块里列出要执行什么，Enter 才跑；进度一行行进驾驶舱。状态、文案、动作全在 named `"computer"` 里，TUI 只渲染 + 路由按键。不嵌真桌面。
- **冒烟**：装好后 `/mcps` 见 `cua-driver` → `search_tool` 查桌面工具 → `use_tool`（先过权限门）完成截图或点按一类动作。

**本机边界（BUA 在 Linux + X11 冒烟；工具面按 macOS `cua-driver` 0.28.1 实测，56 个工具）**

- `doctor` 应见 `display server: X11` + `X11 connection: connected`。若 `[warn] AT-SPI: accessibility bus not reachable`：装 `at-spi2-core`，确保用户会话有 D-Bus；GNOME 可再开 `gsettings set org.gnome.desktop.interface toolkit-accessibility true`。AT-SPI 弱时 `get_window_state` / a11y 树不可靠，点按仍可能走几何。
- 验收常用 MCP 名（公名前缀 `mcp_cua-driver__`）：`list_apps` / `list_windows` / `launch_app`、`click` / `double_click` / `right_click` / `drag` / `scroll`、`type_text` / `press_key` / `hotkey`、`get_accessibility_tree` / `get_desktop_state` / `get_screen_size`、`bring_to_front` / `invoke_menu`。driver 另暴露 `browser_*`——**不要**当 Dock BUA 用。
- 无图形会话 / 纯 SSH 无 `DISPLAY`：`doctor` 会挂；CI 不要默认跑 cua-driver 实机。本机可用既有 X11/Xvfb，但 AT-SPI 仍要会话总线。
- **办公链 S3（完整 CUA 重测）**：开 `mousepad` → `bring_to_front` → `type_text` → **`hotkey` `ctrl+s`（`delivery_mode=foreground`）** 落盘 `/tmp/...`；`invoke_menu` 仅 AT-SPI 绿时可选；再用 `verify_state` + 读文件确认。固定步骤见验收台 `dock-cua-accept/S3_REPRO.md`。包已装仍 warn 时勿只靠菜单。
- **开应用 / S8**：`list_apps` 可能漏 `mousepad` 等 —— `launch_app` **优先** `launch_path=/usr/bin/mousepad`（或绝对路径）。S8 Thunar 选中/树验证依赖 AT-SPI；弱则只证目录打开（几何/截图），勿强求 a11y 选中态。
- 安装脚本：`https://cua.ai/driver/install.sh` → 常落到 `~/.local/bin/cua-driver`；`mcp-config` 的 JSON `command` 可直接抄进 Dock。

安装（Linux 示例）：

```bash
# 官方安装（二进制进 ~/.cua-driver，并 symlink 到 ~/.local/bin）
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
# Debian/Ubuntu 建议再装 AT-SPI：
#   sudo apt-get install -y at-spi2-core
export PATH="$HOME/.local/bin:$PATH"
cua-driver --version
cua-driver doctor          # 查 DISPLAY / X11 / AT-SPI（macOS 报 TCC / 安装路径）
cua-driver list-tools      # 当前版本真实工具面（macOS 0.28.1 是 56 个）
cua-driver describe <tool> # 单个工具的完整描述 + input_schema
cua-driver mcp-config      # 打印推荐 command/args（可抄进 config.toml）
# 可选：cua-driver telemetry disable

# 升级（原地替换，macOS 的 TCC 授权不丢；升完要重启守护进程）
cua-driver check-update
cua-driver update --apply
cua-driver stop && open -n -g -a CuaDriver --args serve   # macOS
cua-driver permissions status                             # 确认 Accessibility / Screen Recording 还在

# 可选：官方 skill pack（**不要** vendor 进本仓——它带 `version:` 标注，会随 driver 漂移）
# 从 GitHub Release 拉版本化副本，并 symlink 进各 agent 的 skills/ 目录，升级自动跟随
cua-driver skills install
cua-driver skills          # 查看本地包与各 agent 的链接状态
```

`~/.dock/config.toml`（或项目 `.dock/config.toml`）样例——**只有要覆盖内置行时才需要写**（换命令、换 args、固定绝对路径，或 `enabled = false` 关掉）：

```toml
[mcp_servers.cua-driver]
command = "cua-driver"
args = ["mcp"]
enabled = true
# 默认已是 ndjson；旧 CL 服务器才写：
# framing = "content-length"
# 若 PATH 没有，可改成 mcp-config 给出的绝对路径，例如：
# command = "/home/YOU/.cua-driver/packages/releases/…/cua-driver"
```

不写这段也能用：装好 driver 后 `/computer` 会自己发现它。`/mcps` 里给内置行按 Space 禁用时，Dock 会把完整的一行落进用户 `config.toml`（带 `enabled = false`），之后就归配置文件管。


## 待做（已挂名、仍比 Grok 薄）

对照 Grok 默认 toolset。做的时候：**cp Grok → 独立插件 → `register` → 测 specs/execute**。不要改 loop。

核心名字都已挂上。`workflow` 是 Rhai。`task` 走 Grok coordinator（`ChannelBackend` + actor）；child 仍是 dock isolate + `GrokStep`（无 worktree / MvpAgent）。

### 已有工具上仍薄的地方（不是新名字）

相对 Grok 完整实现，这些 **已经在表里** 但行为更窄。补的时候还是改对应插件，不要新焊一层。

- `read_file`：纯文本，无 PDF / 图 / PPTX
- `grep` / `list_dir`：参数比 Grok 少
- `web_search`：不是 xAI Responses API
- `task`：`tool-task` 保持 Grok coordinator；无 worktree / ACP / MCP pool。子代理共用父模型、父工具集与父工作目录（没有 per-child cwd/model/persona）
- `update_goal`：尚未自动 spawn Grok 的 `goal plan writer` / classifier / strategist
- MCP：`x-mcp-header` 自定义头未镜像
- `ask_user_question`：自由输入比 Grok 简单

---

## 明确不做

| 项 | 原因 |
|---|---|
| Grok 登录 / 账单 / 账号额度 / 分享 / marketplace | 账号产品。本会话 token 账本除外，见 [CLI.md](CLI.md) `/usage` |
| `image_gen` / `image_edit` / video gen / Imagine | 账号 + 生成；不要用 stub 占名 |
| voice、vim 模式、agent dashboard | 产品面，不是核心工具 |
| ExtraTools / `tools.mcp` 分发表 | 已废弃。MCP 和能力工具都 `register` 进 `"tools"` |
| path-dep 或修改 `grok-build/` | 只复制进 dock |
| 把新工具写进 `event_loop` / `agent-loop` | 违反一切皆插件 |

---

## 加一个工具时的检查

1. 是新插件、换现有插件，还是接到已有 named service / waterfall？
2. Grok 有没有现成实现可 `cp`？
3. `install_app` 是否 `plugin(...).wait()`？`install_fakes` 是否仍是 echo？
4. `Tools::specs()` 是否出现该名字？execute 是否走到 register 的 body？
5. 该挡权限 / 计划的是否进了 `acp.rs`？
6. 人需要看见的：slash、overlay、系统 prompt 一句（slash / overlay 记 [CLI.md](CLI.md)）。
7. 测试对着 **当前树** 断言 specs + 至少一条 execute，不要只 assert stub。
