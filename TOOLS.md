# 工具清单

对照当前 `install_app` 与 `cordis-spine` 源码，不是愿望列表。架构规则仍以 [AGENTS.md](AGENTS.md) 为准；本文件只记 **模型工具、插件粒、缺口、不要做的事**。斜杠命令、快捷键、overlay、底栏见 [CLI.md](CLI.md)。

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
5. **扩展走 waterfall**（`tools/execute`、`system-prompt/assemble` 等），不要在 loop 里分支。
6. **MCP / 可选能力 fail-open。** 没配置或连不上时插件仍 Active，不要让 `install_app` 失败。
7. **不接 Grok 账号产品。** 登录、账单、分享、marketplace、Imagine / 视频生成、voice、dashboard 账号面：不做。核心 agent 能力（读改跑、搜网、todo、提问、计划、后台任务、调度、MCP）要补。本会话 token 账本（输入/输出/缓存/思考）不是账号产品，见 [CLI.md](CLI.md) `/usage`。
8. **产品面要跟上。** 新工具要有：register 进 specs（LLM 看得到）、execute 路径、权限/计划门（该挡的挡）、人要看见的 slash / TUI（记 [CLI.md](CLI.md)）。用户可见文案中文；Grok 底栏 `Enter:send` 那种短 hint 保持英文无空格。
9. **测的是树，不是 stub。** 清单以 `install_app` 挂上的插件和 `Tools::specs()` 为准。不要留一个同名但走另一套 end state 的假实现。

---

## 已有（`install_app`）

挂载顺序见 `cordis-spine/src/bundle.rs`。能力插件都在 `workspace_tools` 之后、`llm` 之前；`compact` 在 `llm` 之后（要注入 `"llm"`）。

| 插件 | ctx | 模型工具 | 备注 |
|---|---|---|---|
| `tools`（`workspace_tools`） | `"tools"` | `list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file` | 工作区内建，不能被 register 盖掉 |
| `jobs` | `"jobs"` | — | 后台进程表；bash `is_background` / `block_until_ms: 0` 用它 |
| `slash` | `"slash"` | — | 额外斜杠命令表；TUI live-lookup。内建 `CATALOG` 不能被盖掉。`kind`：`prompt` / `overlay` / `slot`（打开已登记的 `tui.slots` id）/ **`tool`**（`text`=工具名，直接 `Tools::execute`，结果 Notice；权限门仍生效） |
| `tui.slots` | `"tui.slots"` | — | 动态包登记的 TUI 插槽（数据+回调，不是 ratatui widget）。`install_app` 始终挂上。TUI 用一次通用 `Overlay::Slot` 臂 |
| `agent-presets` | `"agentPresets"` | — | 组装 Agent：YAML 定义人设 + 工具允许名单，运行时只过滤 live `"tools"`。**正在运行的动态包**用 `Tools::register_dynamic` 登记的 extra 工具、以及 **已开启的 MCP 工具**（`register_mcp`，公名 `mcp_{server}__{tool}`）会穿过允许名单。层：crate `presets/<id>/agent.yml` + `agents/*.yml` < `~/.dock/presets` < 项目 `.dock/presets`。仍可读旧 `<id>.yml`（目录优先）。加一个目录就是一个 Agent。内置 `code` / `minimal` / `cordis` / `warden`（守望）。`code`/`cordis` 的 `agents/` 名册是 `general-purpose` / `explore` / `plan`（项目层可加，如 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`）；`warden` 是 `岑` `锁` `甲` `乙` `丙` `衡` `验` `观` `突击`（不要用拼音 id）。发给模型的 `subagent`/`task` 把 `subagent_type` 收成当前名册 enum。省略 `tools` = 全部已注册工具。新建模式默认写**当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` n/d 有项目层时落到这里；系统提示注入 `{cwd}/.dock/presets` 绝对路径；id 必须 `[a-z0-9][a-z0-9-]*`，汉字目录只叠内置）。新建子代理默认写 `.dock/presets/<当前模式 id>/agents/<type>.yml`。空名册仍注入这两处路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后 `subagent` 校验立刻重读；本轮刚写完时用 `subagent`（`reload_roster: true`）刷新 enum。新建模式写完后用 `/preset` 应用该 id。改 crate `presets/` 要重新编译 |
| `tool-web` | → `"tools"` | `web_fetch` `web_search` | Grok SSRF / 同 host 重定向 / htmd。`web_search` 无 xAI 账号，走同一套 fetch 打公开 HTML 索引 |
| `tool-todo` | `"todos"` + `"tools"` | `todo_write` | Grok merge/replace |
| `plan-mode` | `"planMode"` + `"tools"` | `enter_plan_mode` `exit_plan_mode` | 计划文件 `.dock/plan.md`。计划态挡住 bash / 写文件等 |
| `tool-ask-user` | `"ask"` + `"tools"` | `ask_user_question` | 事件 `ask/pending` |
| `tool-jobs` | → `"tools"` | `get_task_output` `wait_tasks` `kill_task` | 查/等/杀后台 bash |
| `tool-scheduler` | → `"tools"`（live `"cron"`） | `scheduler_create` `scheduler_list` `scheduler_delete` | 包着已有 `"cron"`。`fire_immediately` 立刻跑第一次；循环 7 天后过期（过期不跑最后一次，滚动区留中文说明）；最多 50 条；`task_id` 原地更新并保持相位。滚动区 **Loop 卡**（设定 / 列表 / 关闭）。`/tasks` 里 `x` / `[✗]` 关闭 |
| `tool-task` | `"subagents"` + `"tools"` | `task` | Grok `ChannelBackend` + coordinator actor；dock `ChildRunner` isolate `"sessions"`+`"turn"`+`"agentPresets"`。提供 named `"subagents"`。`task` 一次性收集：后台回 `get_task_output`，完成后 `resume_from`。不要在这个插件里挂 `subagent` |
| `tool-subagent` | inject `"tools"` + `"subagents"` | `subagent` `send_message` `list_agents` `interrupt_agent` `report` | **独立 grain**，不是 `task` 的包装。`subagent_type` = 当前模式 `agents/<id>.yml` 角色 id（模型侧参数 enum 即这份名册）。写完新 YAML 后用同一工具 `reload_roster: true` 重读名册（不 spawn）。后台回 `send_message` / `list_agents` / `interrupt_agent`。`send_message`：idle 时 queued 与 urgent 都立刻开下一轮并在返回前把状态打成 running；urgent 只在 running 时才是 send-now。`report` 是子代理和主代理的**多轮通道**（同轮可多次），不是一次性交卷；助手正文到不了父级。mailbox 子代理若本轮未 `report` 就 idle，运行时代转发回合输出，避免主代理空等。必须挂在 `tool-task` 之后（live-look `"subagents"`）。`warden` 主代理只用这套，工具名单不含 `task` / `get_task_output` |
| `tool-memory` | `"memory"` + `"tools"` | `memory_search` `memory_get` | 本地 `~/.dock/memory` / `.dock/memory` |
| `tool-monitor` | → `"tools"`（live `"jobs"`） | `monitor` | 长命令 stdout 盯梢 |
| `tool-goal` | `"goal"` + `"tools"` | `update_goal` | Grok oneshot ack + drain。`objective` 可在无 `/goal` 时由模型自己开目标；无目标且只有 message/completed 时仍 `HarnessDisabled`。进度卡在滚动区 |
| `tool-lsp` | `"lsp"` + `"tools"` | `lsp` | Grok `LspManager`/`dispatch`。没 `lsp.json` 时 fail-open |
| `tool-workflow` | `"workflows"` + `"tools"` | `workflow` | Grok Rhai 引擎（`vendor/xai-workflow`）+ 同款 oneshot ack。Host `SpawnAgent` live-lookup `"subagents"` |
| `mcp-client` | `"mcp"` + `"tools"` | `mcp_{server}__{tool}` | stdio + Streamable HTTP。先走 MCP `2026-07-28`（无 initialize / 无 session，`_meta` + `MCP-Protocol-Version`）；服务器仍是 initialize 时代则回退 `2025-11-25`。fail-open。`inputSchema` 原样注册。开启的 MCP 工具用 `register_mcp` 穿过 Agent 预设允许名单。`tools/list` 跟 `nextCursor`（最多 64 页）；`notifications/tools/list_changed` 50ms 合并后重列。advertise `elicitation.form` + `elicitation.url`；stdio 读循环 / HTTP POST SSE 按序 / GET SSE 收 `elicitation/create`。HTTP GET 长连接；initialize 时代 `Mcp-Session-Id` 的 POST 404 会重新握手再试一次。`/mcps` Space 开关服务器（`[mcp_servers.<name>].enabled`）或单工具（`[disabled_mcp_tools.<server>]`）；HTTP 服务器 `i` 浏览器 PKCE OAuth（DCR 或 `oauth.clientId`），token 在 `~/.dock/mcp_credentials.json`，启动不自动开浏览器 |
| `dynamic-runner` | `"dynamicCordisRunner"` | — | 会话内注册表；热挂体走 `ctx.plugin` / `fiber.dispose`，不改内核。进程内存，不落盘。定义盖章 `Sessions::identity()`（主会话 `main`，子代理用其 id）；别的会话读起来像不存在 |
| `compact` | `"compact"` | — | Grok 会话压缩。`install_app` 在 `llm` 之后挂。手动 `/compact [说明]`；上下文达到窗口 85% 时 `maybe_auto`（loop live-lookup，工具轮次结束后、下次采样前）。摘要 prompt / 清洗 / 阈值从 grok-build `xai-grok-compaction` 拷来 |
| `tool-cordis` | → `"tools"`（inject `"dynamicCordisRunner"`） | `cordis_inspect` `cordis_inspect_self` `cordis_define` `cordis_run` `cordis_call` `cordis_stop` `cordis_undefine` | 预置工厂 `echo` / `note` / `hold` / `slash`，加上 `factory: "rhai"`（`source` 在 define 时 compile，run 时 eval `apply`）。`cordis_call` 经 `Tools::execute` 试调任意 live 工具（含 `register_dynamic`，不必等模型下一回合或 TUI slash）。`cordis_inspect` `what`: `services`（`tools` / `slash` / `tui.slots` 带方法签名，其余只列名）/ `builtins`（Rhai `host` 方法）/ `slots`。`inspect_self` 对 Rhai 包回传 source。用户文本 `@pluginId` 在 `agent/pre-step` 注入身份 reminder（不含源码）。审批走权限 overlay（`cordis_run`）。Skill：`skills/cordis-plugin-development/SKILL.md` |

权限门（询问 overlay）：`bash` `search_replace` `write_file` `scheduler_create` `kill_task` `monitor` `cordis_run`。

计划门：同上（`enter_plan_mode` 之后这些返回 blocked，直到 `exit_plan_mode`）。例外：对 `.dock/plan.md` 的 `search_replace` / `write_file` 自动放行（对齐 grok）。

---

## 待做（已挂名、仍比 Grok 薄）

对照 Grok 默认 toolset。做的时候：**cp Grok → 独立插件 → `register` → 测 specs/execute**。不要改 loop。

核心名字都已挂上。`workflow` 是 Rhai。`task` 走 Grok coordinator（`ChannelBackend` + actor）；child 仍是 dock isolate + `GrokStep`（无 worktree / MvpAgent）。

### 已有工具上仍薄的地方（不是新名字）

相对 Grok 完整实现，这些 **已经在表里** 但行为更窄。补的时候还是改对应插件，不要新焊一层。

- `read_file`：纯文本，无 PDF / 图 / PPTX
- `grep` / `list_dir`：参数比 Grok 少
- `web_search`：不是 xAI Responses API
- `task`：`tool-task` 保持 Grok coordinator；无 worktree / ACP / MCP pool。不要把持续交流焊进 `task`
- `subagent`：`tool-subagent` 独立 grain（inject `"subagents"`）。不要经 `task` / `execute.rs` 转发
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
