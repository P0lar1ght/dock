# 架构

Dock 的 harness 是一棵 Cordis 插件树。内核是 crate `cordis`（`Context`、`inject`、named service、waterfall、fiber 生命周期）。**没有可私自打补丁的内核**：新行为只能是再挂一颗插件，或接到已有 named service / waterfall 上。

硬规则在根 [AGENTS.md](../AGENTS.md)；本文件是可重复查的目录地图与不变式。工具名单见 [TOOLS.md](../TOOLS.md)（细节在 [tools/](tools/)），人操作的面见 [CLI.md](../CLI.md)。

## 目录地图

```
cordis-rust/             crate `cordis`：Context、inject、named service、fiber、waterfall
cordis-base/             spine 的底座：wire 类型、config.toml、纯引擎。**不含插件**
  src/types.rs           LogEvent / ToolCall / ToolResult 与会话身份原语
  src/config.rs          config.toml 解析与模型目录     src/usage.rs   token / 费用账本
  src/chat_chunk.rs      流式分片                       src/stream_acc.rs  流式累积
  src/grep.rs            进程内 ripgrep 引擎            src/tool_output.rs 工具输出预算
  src/cua.rs             cua-driver 发现与授权          src/acp.rs     ACP 权限选项种类
cordis-spine/            Agent 循环、工具粒、MCP、会话、预设、权限；install_app
  src/*.rs               lib / names（ctx 键）/ error / bundle（组合根）
  src/agent/             runtime、loop_plugin、turn、agents、presets、capability（子会话委派档位）
  src/session/           log（内存事件流）、persist（落盘）、roster（跨 cwd 名册）
  src/llm/               sampler、http/（三条 wire）、compact/
  src/prompt/            assemble、context_book、listing、project_instructions、context_usage
  src/tools/             registry（那张唯一的 "tools" 表）+ 全部工具插件
  src/host/              宿主 live-look 的表：settings、permissions、slash、tui_slots
  presets/               内置 Agent 预设 YAML（code / minimal / cordis / warden）
  tests/                 round.rs（install_app_registers）、dynamic.rs、subagents.rs
cordis-tui/              全屏终端 UI 插件：theme、scrollback、prompt、statusBar、shortcuts…；fuzzy-file-search crate
  src/app/               状态与派发：actions、dispatch、event_loop、input、clipboard
  src/views/             画出来的东西：overlay、dashboard、各 *_view / *_modal / pane
  src/seam/              TUI live-look 的 named service 座：session、gateway、shortcuts、tabs
  src/theme/             调色板（grokday / groknight / tokyonight）
  src/scrollback/        transcript 渲染与卡片
  src/grok/              从 grok pager 冻结复制的 chrome（glyphs、picker、wrapping…）
cordis-gateway/          回环 HTTP/WS 插件：配对、dock.1 JSON-RPC 投影、slash list|execute
cordis-app/              二进制入口：一个 Context，install_app + agent-loop + gateway + tui
dock-memory/             crate `dock-memory`：跨会话 topics/observations + FTS（默认关）
dock-render/markdown/    crate `cordis-markdown`
dock-render/mermaid/     crate `xai-grok-mermaid`
embed-sdk/               宿主页 JS SDK（`dist/dock-embed.js`，协议 dock.1）
dock-render/third_party/ 冻结的 mermaid 布局栈（dagre / graphlib / to-svg / ordered_hashmap）
vendor/xai/              冻结的 xai 拷贝（workflow、grok-tools；fuzzy-file-search 在 cordis-tui/）
skills/                  Bundled skills
.agents/skills/          Agents scope skills（仓库流程）
assets/logo/             品牌图
config.toml.example      用户 / 项目模型目录样例
```

`vendor/xai/grok-tools` 不是 workspace 成员（`Cargo.toml` 的 `members` 里没有它）；成员列表以根 `Cargo.toml` 为准。

## 树怎么长出来

`cordis-app` 起**一个** `Context`，分两步：

1. `install_app` 先挂 Spine 五件套 + Harness 服务 + 工具粒；尾部是 `llm`，之后 `compact`。
2. `main` 再挂 `system-prompt.base`、`agent-loop`、`session_actor`、`gateway`（默认不监听）、`cron-driver`、`tui`。

`main` 只做组装，不焊行为逻辑——基座系统提示、1s 调度都是各自一颗插件。TUI 与 Gateway 都是树上的插件，不是旁路进程。`embed-sdk` 只连回环 Gateway（`dock.1`），不另起 harness，也不直连 TUI。

挂载有序：工具粒都在 `workspace_tools` 之后、`llm` 之前 `register`；`compact` 在 `llm` 之后（`inject "llm"`）；`tool-task` 同时登记 spawn 工具与 mailbox 工具。完整顺序与每颗粒的工具名见 TOOLS.md，单颗细节见 `docs/tools/`。

## named service 与插件粒

| 层 | 例子 | ctx key |
|---|---|---|
| Spine 五件套 | `sessions` `llm` `tools` `systemPrompt` `agents` | 同名 |
| 循环 | `agent-loop` 提供 `LoopHandle` | `agentLoop` |
| 其它 spine | `context` `settings` `turn` `permissions` `cron` `roster` `jobs` `todos` `planMode` `ask` `mcp` `goal` `lsp` `skills` `subagents` `memory` `browser` `computer` `workflows` `slash` `agentPresets` `dynamicCordisRunner` `compact` | 同名 |
| 工具插件 | `tool-web` `tool-browser` `tool-todo` `plan-mode` `tool-ask-user` `tool-jobs` `tool-scheduler` `tool-task` `tool-memory` `tool-monitor` `tool-goal` `tool-lsp` `tool-skills` `tool-workflow` `mcp-client` `tool-cordis` | 向 `"tools"` `register` |
| TUI | `theme` `tui.scrollback` `tui.prompt` `tui.statusBar` `tui.welcome` `tui.shortcuts` `tui.pairing` `tui.tabs` | 同名 |
| 回环网关 | `gateway` | `"gateway"`（`GatewayRef`），事件 `gateway/pairing` |

`settings` 持有模式、模型、权限开关；TUI 只把按键映射成 Action，再 live-lookup `settings`。计划是独立模式，不是第三种权限。会话落盘在 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` + `chat_history.jsonl`），不是项目 `.dock/`。

新东西往 `cordis-base` 还是 `cordis-spine` 放，判据是**有没有插件**：base 不 `provide` 任何 named service、不认识 ctx 键（所以 `names` 不在那儿）、也不依赖内核 crate `cordis`；它只有 wire 类型、`config.toml` 解析和纯引擎（ripgrep、cua 发现）。反过来，`settings` / `permissions` / `slash` / `cron` 虽然也不成环，但它们 provide 服务，留在 spine。这条线由编译器守着——base 反向依赖 spine 会直接编译失败，以前只能靠约定。

`cordis-spine/src/tools/` 把注册表与全部工具实现收在一起，对齐 Grok 的 `xai-grok-tools`（那边同样是 `registry/` + `implementations/` 一个 crate）。注册表要问预设的允许名单、工具又要往注册表 register，这圈依赖是工具表这件事的固有形态，不是 dock 特有的耦合，所以不拆成两个 crate。**布局标准**：一能力一顶层目录（`ask_user/`、`browser/`、`read_file/`、`bash/`、`jobs/`…）；共享助手可以是旁边的 `*_common.rs`；工作区七颗由薄 `workspace.rs` 套件调度，**没有** `tools/workspace/` 伞目录。子会话委派档位（`CapabilityMode`）在 `agent/capability.rs`，不是模型工具，不进 `tools/`。

`roster` 与 `sessions` 不是一回事，别混：`sessions` 是**本页**的会话日志（每页一份，`archived()` 走 `load_cwd`，只看当前 cwd 且会把整份 transcript 解出来）；`roster` 是**跨 cwd** 的会话抬头名册（全局一份，扫 `$DOCK_HOME/sessions/*/*/`，每条只读 `meta.json` 加 jsonl 尾部 64KB 取一行摘要，压 2s TTL 备忘挡住每帧重扫）。名册项的 `cwd` **只能从 `meta.json` 读**——`encode_cwd_dirname` 把 `/` 和非字母数字都压成 `-` 再折叠连续 `-`，目录名是有损的、反解不回来。对应 Grok pager 的 `app/roster.rs`，是 agent dashboard 的行来源之一。

## 分页：一个终端里的多个会话

`Ctrl+N` 开的每一页 = **一棵 isolate 子树**。第 1 页就是根上下文本身；第 2 页起由
`root.isolate("sessions").isolate("turn")…` 派生，只有 `PER_TAB_SERVICES` 里的名字
各有一份：

| 每页一份 | 全局一份（落回根） |
|---|---|
| `sessions` `turn` `agentLoop` `session` `session.port` `goal` `todos` `planMode` `settings` `permissions` `ask` `agentPresets` | `tools` `llm` `systemPrompt` `agents` `mcp` `skills` |
| `tui.scrollback` `tui.prompt` `tui.statusBar` `tui.welcome` | `browser` `computer` `jobs` `cron` `lsp`（按项目根分，见不变式 13） `workflows` `slash` `theme` `gateway` `memory` |

服务按 `(isolate realm, name)` 解析，没被 isolate 的名字自然落回根 —— 所以**一张
`"tools"` 表**的不变式没有被破坏，两页共用同一张表、同一个 `llm`。模型、协议、
权限模式在这一页的 `settings` 上；权限队列和 `ask` 也按页各一份，底栏读的是
当前页。预设也按页（`tab.presets` 从开它的那一页 `AgentPresets::fork` 一份），
所以在工具体里读预设要用 `exec_ctx()`（调用方那页），不能用注册时捕获的根 ctx
——`task` 的名册 / 默认角色就是这么取的。工作目录同理，新页继承开它那页钉住的
cwd。系统提示照样按页组装：`SystemPrompt::assemble_on(exec)` 收的是各页自己的 ctx。

事件**不分 realm**（`ctx.emit` 不看 isolate），后台页的 `session/event` 照样能把
前台叫醒重绘，标签栏上的 `●` 就是靠这个活的。

装一页要挂哪些插件由**组合根**决定（`cordis-app` 的 `tab_mount()`），TUI 只管开 /
关 / 切：`"tui.tabs"` 拿到的是一个建页插件工厂。关页 `dispose` 那一颗页 fiber，
它下面的会话、循环、actor、视图一起走。

**分叉**（`Ctrl+F`）= 建页时把来源页的 `model_history()` 快照 `seed` 进新页的会话，
并记下 `origin`。快照是值传递，分叉后两页互不影响。**带回**（`Ctrl+B`）把分叉页最近
一条有正文的回复填进来源页的 `PromptWidget` 并切过去 —— **不**往来源页的历史里写，
替用户说话不是分页该做的事。

**旁问**（`/btw`）是第三种语义：同样是分叉，但那一页 `TabKind::Aside` —— 它的
`agentPresets` 不从来源页复制，而是一份**只读**预设（`cordis-app` 的 `aside_preset()`）。
它就是一张**普通分页**，进标签栏（标 `?`）、有自己的滚动区，所以答案走的是和主线
一样的渲染路径（markdown、工具卡、流式）。「不打断」指的是主线那一轮照跑，不是把
答案塞进一个额外的面板里 —— 早先那版把它画成输入框上方的 pane，既不渲染 markdown
也放不下长回答，已经删掉。`/tab promote` 转正：用它的历史开一张全权常驻页，再把
旁问那一页 dispose 掉。

分页会话的身份是 `main#<N>`，`is_main_identity` 认它是**并列的主线**而不是子代理
（判错会让第 2 页拿不到工具目录）。

**落盘**：常驻页和主会话一样 `attach_disk`，落在**这一页自己的 cwd** 下（`tab.sessions`
建页时做；`Sessions::attach_disk` 用会话自己的 cwd），关页后仍在历史里；`/btw` 旁问页
不落盘。**已知边界**：`--resume` 与 gateway `dock.1` 投影仍只跟第 1 页。权限队列和 `ask` 按页隔离，只在那一页上弹出；MCP 连接仍是全局的，
elicitation 盖了来源页，也只在那一页上显示。同一台 MCP 服务器两页可以同时调用：HTTP 上提问跟着那次 POST 的响应走，各页弹各页的框。标签上 `◆` 表示那一页有东西在等回答。
浏览器、cua、后台任务表仍是全局单例，两页会抢。

**计划文件按页划分**：`planMode` 隔离之后，计划文件也跟着按会话分 ——
`$DOCK_HOME/sessions/<cwd-key>/<id>/plan.md`（主会话）或
`sessions/<cwd-key>/tabs/<pid>/<identity>/plan.md`（不落盘的页——现在只有 `/btw` 旁问页，identity = `main#N`；`pid` 隔开不同进程的同一页号；常驻页落盘，计划在它自己的会话目录里），
对齐 grok-build 的 `$GROK_HOME/sessions/<cwd>/<id>/plan.md`。这样第 2 页的计划
不会覆盖第 1 页的。写门按本页期望路径判定，只有写**本页**计划文件的 `write_file` /
`search_replace` 才算计划编辑而豁免只读门。`enter_plan_mode` 交给模型的是这条
绝对路径，相对路径 `.dock/plan.md` 不再放行。

**子代理按页记账**：`"subagents"` 仍是全局一份，但协调器本来就按
`parent_session_id` 分账（`spawn_blocked_sessions` / `session_running_count` /
`belongs_to_session`），所以只要 spawn 时带上正确的身份就够了。`task` 的工具体是
全局注册一次、捕获根 ctx 的，身份从**执行期 ctx** 拿 ——
`Tools::execute_on` 用 task-local `EXEC_CTX` 把它递进工具体，`caller_session_id()`
读出来。只认主线身份（`main` / `main#N`）：子代理再 spawn 的孙代理照旧挂在 `main`
上，不动既有的取消语义。Stop 与新会话都走 `Subagents::cancel_session(identity)` /
`open_admission_for(identity)`，所以第 2 页按 Stop 不会收掉第 1 页的孩子。

## 一轮怎么跑

TUI 从不持有循环：按键映射成 `SessionCommand` 交给 `session_actor`，actor 管队列（提交 / 立即发送 / 提前 / GoalSummary 续跑 / 子代理 mailbox 续跑），每次取一条调 `agent-loop` 的 `LoopHandle`。默认 driver 是 `GrokStep`：

```
agent/pre-step               每轮开始，一次
system-prompt/assemble       每轮一次
agent/step-start             每个采样步之前，一次
llm/stream                   枢纽：出工具调用 → tools/pre-execute（改写 / 改道 / 拒绝）
                             → 计划门 / 权限门 → 工具体 → tools/execute → 回采样
                             出文本（或采样步数耗尽）→ agent/turn-end
agent/turn-end               有人要续跑 → 落 <system-reminder> 回到采样
                             没人要续跑 → turn 结束
```

安全上限 256 步。换 driver 只换 `agent-loop` 插件，不动 actor。

**开轮前的 handler 自己 append。** `agent/pre-step` 和后两条不一样：handler 直接往 `Sessions` 写（目标指令、技能正文、MCP 目录变更通告、计划提醒），而它能拿到的 `Sessions` 只有注册时捕获的那一份 —— 主会话。子代理开轮同样会跑这条链，所以凡是要写会话、消费一次性状态、或推进用户自己状态的 handler，都得先看载荷里的 `identity`（`PreStep::is_main_session()`）。六个内建 handler 都这么做。

**中途盯梢也是插件说了算。** `agent/step-start` 每个采样步之前跑一次 —— `agent/pre-step` 是每轮一次、`agent/turn-end` 是收尾一次，都盯不住跑起来的一轮。载荷 `StepStart` 带 `step`（本轮已采样步数，续跑不清零）和 `identity`，handler 用 `remind(order, 正文)` 排队，循环按 order 顺序落成 `SystemReminder`。带 per-turn 状态的 handler 在 `step == 0` 自己重置，循环不替谁存状态。现有一个：

- `tool-todo`（`ORDER_STEP_START_TODO = 10`）：待办列表连续 6 步没动且仍有未完成项时提醒勾选 / 调整，每轮最多 3 次（`Todos::revision()` 计数）。只在主会话生效 —— 子代理不 isolate `"todos"`，靠载荷里的 `identity` 判断；分页则**各有**一份 `"todos"`（`PER_TAB_SERVICES`），各提醒各的。

**收不收尾是插件说了算。** 循环只跑 `agent/turn-end` 链、数轮数（硬止损 64 轮）、把胜出的正文落成 `SystemReminder`；`append` 不交给 handler，免得 reminder 插进 `tool_calls` 和它的 `ToolExecute` 之间。载荷 `TurnEnd` 带 `text` / `rounds` / `ended_with_text` / `queued_followups` / `identity`，handler 用 `keep_working(order, 正文)` 表态，order 小的赢。没有 handler 就正常收尾（fail-open）。handler **看不到链外的表态**，也就不知道自己赢没赢，所以自限配额这类账要用 `keep_working_with(order, 正文, on_win)` —— 循环选出胜者后只跑胜者的 `on_win`，输掉的那轮不扣（`settle` 取走所有权，扣两次这种事写不出来）。现有两个：

- `tool-todo`（`ORDER_TURN_END_TODO = 10`）：出文本收尾但还有 pending / 无后台任务托底的 in_progress 时续跑，每条用户消息最多 2 次（`keep_working_with` 扣在胜出，计数在 `agent/pre-step` 清零）。只在主会话生效 —— 子代理不 isolate `"todos"`，靠载荷里的 `identity` 判断；分页各有自己的清单。
- `tool-goal`（`ORDER_TURN_END_GOAL = 20`）：`/goal` 没 `update_goal(completed)` 就一直续，最多 64 轮；两种收尾都续。

两个都尊重 `queued_followups`：用户已经排了下一条时不抢方向盘。

## 不变式

1. **换插件，不改 loop。** 新 UI 面是 `tui.*` 插件；新采样是 `llm` 插件。不要把功能焊进 `event_loop` 或 `agent-loop`。不要调用 `xai_grok_pager::app::run`，不要 spawn Grok `MvpAgent`。
2. **一张 `"tools"` 表。** 工具能力插件 `inject: ["tools"]` 后 `ctx.tools.register()`；MCP 也进同一张表，不是 `tools.mcp` 之类的副表。
3. **live-lookup，不捕获。** 调用点 `ctx.get` / `ctx.require`。不要把 `Arc<T>` 关进长生命周期闭包（TUI frame、HTTP 重试、cron tick、sampler `on_delta` 这类闭包里也要重新 `get`）。
4. **扩展走 waterfall。** 七条：`agent/pre-step`、`agent/step-start`、`agent/turn-end`、`llm/stream`、`tools/pre-execute`、`tools/execute`、`system-prompt/assemble`。工具那两条分工是**时机**：`tools/pre-execute` 夹在允许名单的两次检查之间（入站一次挡模型越界、改写后一次挡插件替它越界）、计划门与权限门之前，载荷 `PreExecute` 带着 `arguments`，可以 `rewrite_args`（改参数）/ `rewrite`（改道到另一颗工具）/ `deny`（拒绝，理由直接成为模型看到的结果）；两道门读的是改写**之后**的名字，所以改道会跟着换一套门。`deny` 是**单调**的——第一次拒绝说了算，后面的 handler 掀不翻，否则「这条策略成不成立」就由挂载顺序决定了。它是**同步**的（内核 waterfall 收同步 handler），做不了异步询问，自定义权限询问仍走 `acp::needs_permission` + `Permissions::request`。`tools/execute` 则是 post-hoc 的，载荷 `ToolResult`。循环里不留第八条私有扩展点。拦截接 `on_waterfall`，默认实现放在 `waterfall(..., || default)` 的闭包里。监听必须把控制权交给下一环，不许吞链。handler **拿不到执行 ctx**（`EventArgs` 只有 payload 和 `next`），只有注册时捕获的那个 ctx；跟着会话走的东西（如 `identity`）要放进载荷，per-turn 状态由 handler 自己存、按载荷里的信号（如 `StepStart::step == 0`）重置。`agent/step-start` 是例外：循环把**当前这个 agent 的** ctx 挂成 task-local（`cordis_spine` 的 `tools::exec_ctx()` / `with_exec_ctx`），要子代理自己那份会话 / 窗口的 handler 走它——子代理是隔离 ctx，注册时捕获的那个只看得见主会话。多个 handler 会抢同一个结果、或要定彼此先后时用显式 order 槽（`system-prompt/assemble` 的段序、`agent/turn-end` 的 `ORDER_TURN_END_*`、`agent/step-start` 的 `ORDER_STEP_START_*` = 规约 5 / 记忆 7 / todo 10 / 动态 50），不靠挂载顺序定胜负。其中 `agent/step-start` 与 `agent/turn-end` 对磁盘 Rhai 包开放（`host.on`，槽位 `*_DYNAMIC = 50`，排在内建之后）—— 这两条的契约是「返回正文、循环 append」，脚本既碰不到 `Sessions` 也吞不掉链；其余五条不可脚本化。`agent/step-start` 的提醒有两种落位：`remind` 追加在尾部；`remind_preamble` 给「整场对话的背景」（规约、记忆），会话还没向模型发过请求时由 `Sessions::insert_preamble` 排在第一条用户消息之前，之后同样追加——只在什么都还没发时往前插，才改写不到任何已经发出去、被上游缓存过的前缀。
5. **提示词分段归贡献插件。** `inject: ["context"]` 后向 `ContextBook` 登记 `set_base` / `section` / `replace_base`（fiber dispose 注销）。`systemPrompt` 只是 facade；基座只写身份与按需发现，**不列工具名**。计划 / 目标、**工作区规约（`AGENTS.md`）与长期记忆（`MEMORY.md`）**走历史里的 `<system-reminder>`，不进系统提示——`AGENTS.md` 是仓库内容、`MEMORY.md` 由会话内容沉淀，都不能借用 harness 自己的声音（中和 `<system-reminder>` 变体后再包进带标记的块；内容与历史里最近一份一致就不重注）。规约与记忆在新会话里排在第一条用户消息之前（第 4 条的 `remind_preamble`），所以每个新会话（以及同一角色先后派生的子代理之间）的请求在用户消息之前逐字节相同，跨会话命中前缀缓存。段序：`ORDER_CORDIS=10` < `ORDER_PERSONA=20` < `ORDER_WORKFLOWS=40` < `ORDER_SKILLS=41`——**这张表里没有任何一段跟着 cwd 变**，`/cd` 不再让前缀作废，主会话、各分页、子代理共享的头更长。**工具用法不进系统提示**——子代理名册与信箱语义归 `task` / `send_message` / `interrupt_agent` 的 description，基座只写身份、按需发现与不随预设改变的工作底线。技能 / 工作流两段只发主会话（子代理靠 `agents/<type>.yml` 的 `listings: true` opt-in），且各自 header 点名的 loader（`skill` / `workflow`）必须在本会话的 `specs_for_model_on` 里才发——listing 不能宣传一个这个预设调不动的工具。预算与渲染共用 `src/listing.rs`。
6. **MCP / 按需工具 fail-open。** 连不上仍是 `Active`，往 `"mcp"` 写空 / 失败状态。不常用本地工具（`register_deferred`：scheduler / monitor / goal / lsp / cordis_* / browser_*）与运行中动态包注册的工具（`register_dynamic`，照样绕过预设允许名单）注册进 `"tools"` 但**不进** sampler 的 `specs_for_model`；模型侧固定 `search_tool` + `use_tool`。`memory_search` / `memory_get` 在 memory **启用**时走 `register`（sampler 常驻），关闭时不进表。**两类工具不能改成按需**：常驻描述或提醒里点了名的（`skill` / `workflow` 被 listing 点名，`job` / `kill_task` 被 `bash`，`send_message` / `list_agents` / `interrupt_agent` 被 `task`（`send_message` 还被子代理的初始任务点名），`exit_plan_mode` 被计划模式），以及只读子代理要直接用的（`web_*`、`memory_*`）——只读档不给 `use_tool`，按需化等于把它们从只读子代理手里拿走（`deep-research` 的 researcher 就是只读档）。工具描述保持静态，以保住 tools JSON 前缀缓存。
7. **工具名不撞车。** MCP 公名 `mcp_{server}__{tool}`，不能盖掉 `bash` 之类的内置名。
8. **Gateway 默认不监听。** 只绑 loopback（首选 `127.0.0.1:18991`，占用往上找，同端口再试 `[::1]`；`DOCK_GATEWAY_BIND` 只改首选）。`/pair` 开启；鉴权靠配对 + 一次性 ticket + 回环，CORS 反射 Origin 是有意的。
9. **reasoning 不混进助手 markdown。** 推理走 `StreamDelta::Reasoning` / `LlmOutput.reasoning`；工具卡折叠显示 name + 参数摘要，展开先「输入」再「输出」，参数在 `LogEvent::ToolExecute.arguments`。
10. **一次 workflow run 一个预算 + 一个并发池。** host 在 `cordis-spine/src/tools/workflow/host.rs`，每 run 一个 `WorkflowHost`。引擎（`vendor/xai/workflow`）**自己不记账**——`agent()` 预留 1、`parallel()` 一次预留整批，全靠 host 的 `ReserveAgentCalls` 回执决定放不放行，所以 `agent_budget` 只能在这里兑现；超了回 `AgentCallQuotaExceeded`，引擎翻成 `WorkflowOutcome::BudgetExceeded`。并发同理：`admission.rs` 对 workflow owner 的子代理**直接放行**（注释里的 "follow the run's own pool"），会话限流管不到它们，那个 pool 就是 host 的 semaphore。子代理的 owner 必须带**真实 run id** 并共用 run 的 `CancellationToken`，否则按 run 取消（`cancel_workflow_children` + `workflow_cancel_waiters`）一个也匹配不到。`workflow` 工具在 `depth > 0` 拒绝：宽口径角色的工具集里有它，不挡则每层递归都拿一份全新预算。
11. **workflow 只在收尾时叫醒主线程一次。** 子代理通往父信箱的两条路都按 owner 拦在 `ChildStore`：回合结束通知看 `SubagentRequest::surface_completion`（`runner.rs` 真的读它），子代理发给父级的 `send_message` 看 `owner.workflow_run_id()` 分流进 run 自己的队列。run 还在跑时推「某个孩子跑完了」，主线程就会在一份残缺的中间结果上烧一整轮，而一次 deep-research 有十来个孩子。过程上报折进 run 快照供 overlay 显示，并在 run 里按发生顺序攒着（上限 64 条），收尾时随 `ParentNotice::WorkflowDone` **整批**交付——行上的 `latest_report` 是覆盖写的，从它反推等于对主线程少说一半。
12. **能力档位压在允许名单之上。** `agent::capability::CapabilityMode` 挂在**受限子会话**的 `"capability"`（主会话没有这一项 = 不设限），`Tools::outside_allowlist` 与 `specs_for_model_on` 两处都查，且查在 `bypasses_allowlist` **之前**——MCP 与动态包工具绕过预设允许名单是有意的，但绕不过「这次委派只准读」。分类按工具名、**默认关闭**：新工具忘了归类是在受限子代理里不可用，而不是带着写盘能力溜进只读会话。同一套做法给采样：`"model-override"` 只挂在受限子会话上，`llm` 采样器先查它再回落 `"settings"`。**两个名字都必须先 `isolate` 再 `provide`**——没隔离的名字 provide 进的是共用注册表，第二个同样收窄的孩子会撞「service 已注册」直接起不来，而那张 `Disposable` 被 `ChildStore` 攥到孩子被处置为止，期间**主会话**自己也查得到那份 read-only，工具表跟着被收窄。**不给子会话隔离一份 `AppSettings`**——那里面还有权限档位这类会话级状态，隔离一份等于让子代理带着一张过期的权限快照跑。
13. **cwd 跟会话走，不跟进程走。** 同一进程里的分页（将来 GUI 的多项目会话）可以在不同目录，所以工具、系统提示的辅助函数、子进程、TUI 视图不直接读 `std::env::current_dir()`：有 ctx 用 `cordis_spine::session_cwd(ctx)`，没有就用 `current_cwd()`（读 `exec_ctx()`——`LoopHandle` 把整轮、`Tools::execute_on` 把每次工具调用都挂在这页的 ctx 下）。来源是 `Sessions::workspace_cwd`：没钉 = 跟随进程 cwd，子代理起步时继承父会话钉住的值，新分页继承开它那页的。`/cd` 走 `cordis_spine::change_dir`，只钉这一页（连同这页预设的项目层），**不调 `set_current_dir`**，所以进程 cwd 从启动起就不再变。起子进程一律显式 `current_dir`（`bash` 不传 `workdir` 也给会话 cwd）；在后台任务里执行的（如 workflow 启动）要在工具调用时取好 cwd 随请求带过去，那里没有 exec ctx。`cordis-base` 不依赖 spine，需要 cwd 的函数由调用方传入（如 `grep::run(.., cwd)`）。LSP 按项目根各一份（`LspHub::for_root`）。**只认启动目录（进程 cwd）的项目级资源**：`.dock/config.toml`（模型目录、MCP、浏览器、web_fetch、memory 配置）、skills、动态插件——它们在启动时装进全局服务 / 斜杠表，按页拆要动一张 `"tools"` 表与前缀缓存；`/cd` 到带这些的目录时 flash 会说明。会话落盘（`attach_disk`）按会话自己的 cwd，分页各落各的项目。
14. **skills 覆盖顺序。** `scan_all` 按 Builtin（`$DOCK_HOME/bundled/skills`，编译期嵌入、启动物化）→ Bundled（`{cwd}/skills`）→ User（`~/.dock/skills`）→ Agents（`{cwd}/.agents/skills`）→ Project（`{cwd}/.dock/skills`）合并，同名后者覆盖前者。

## 磁盘

- `~/.dock`（可用 `DOCK_HOME` 覆盖）：config、presets、plugins、skills、memory、`sessions/`、`mcp_credentials.json`。
- Memory（Stage 3，默认关）：`$DOCK_HOME/memory/{global,workspace-<slug>}/{topics,observations/_inbox,archive}/`，索引 `$DOCK_HOME/memory/search.sqlite`（FTS5 + 可选 sqlite-vec hybrid + MMR）。启用时 file watcher dirty sync、可选 `[memory.embedding]` 查询 embed + 写后 `embed_missing`。`/flush` 只写 memory；compact 仍写 `sessions/.../compaction/`（A7）并 bump cycle。斜杠 `/flush` `/dream` `/memory` `/remember`；工具 `memory_search` / `memory_get` 启用时 sampler 常驻。仍 deferred：auto-capture / auto-dream timer / drag_select。
- 项目 `.dock/`：覆盖 config、presets、plugins、skills、`plan.md`、workflows。
- HTTP MCP 的 OAuth token 在 `~/.dock/mcp_credentials.json`，**不写进** `config.toml`。它不是 grok.com 账号登录。

## 已知边界

- `cordis-gateway` 的 rustc **1.94+** 下限由根 `Cargo.toml` 的 `[workspace.package].rust-version` 固化，见 [DEVELOPMENT.md](DEVELOPMENT.md)。
- `dock-render` 的 mermaid 面依赖 `dock-render/third_party/` 冻结副本。
- `embed-sdk` 只解析、采集（截图）、把 Gateway 的 `{ kind }` 画出来；斜杠目录迭代 `cordis_tui::slash_catalog()` + `"slash"` extras + `/screenshot*`，不手抄表。标 `terminal` 的命令（`/cd`、`/settings` 含带参）execute 拒绝。
- 本机桌面 CUA 走外部 cua-driver MCP，不自研键鼠；全部与 `bash` 同级权限 / 计划门。cua-driver 自带的 `browser_*` ≠ Dock BUA 的 `browser_*`。
