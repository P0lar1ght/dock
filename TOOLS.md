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
8. **产品面要跟上。** 新工具要有：register 进 specs（或 `register_deferred` 走 `search_tool`）、execute 路径、权限/计划门（该挡的挡）、人要看见的 slash / TUI（记 [CLI.md](CLI.md)）。用户可见文案中文；Grok 底栏 `Enter:send` 那种短 hint 保持英文无空格。
9. **测的是树，不是 stub。** 清单以 `install_app` 挂上的插件和 `Tools::specs()` 为准。不要留一个同名但走另一套 end state 的假实现。

---

## 已有（`install_app`）

挂载顺序见 `cordis-spine/src/bundle.rs`。能力插件都在 `workspace_tools` 之后、`llm` 之前；`compact` 在 `llm` 之后（要注入 `"llm"`）。

| 插件 | ctx | 模型工具 | 备注 |
|---|---|---|---|
| `tools`（`workspace_tools`） | `"tools"` | `list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file` | 工作区内建，不能被 register 盖掉 |
| `jobs` | `"jobs"` | — | 后台进程表；bash `is_background` / `block_until_ms: 0` 用它 |
| `slash` | `"slash"` | — | 额外斜杠命令表；TUI live-lookup。内建 `CATALOG` 不能被盖掉。`kind`：`prompt` / `overlay` / `slot`（打开已登记的 `tui.slots` id）/ **`tool`**（`text`=工具名，直接 `Tools::execute`，结果 Notice；权限门仍生效） |
| `skills` | `"skills"` | — | 发现 `SKILL.md`（`{cwd}/skills/` < `~/.dock/skills/` < `{cwd}/.agents/skills/` < `{cwd}/.dock/skills/`，后者同名覆盖）。frontmatter：`name` `description` `when-to-use` `paths` `user-invocable`（缺省 true）`disable-model-invocation`（缺省 false）。向 `"context"` 登记 listing 段（窗口 token ×4 ×8%，不要把全文塞进系统提示）。`agent/pre-step` 把用户气泡 `/name args` 注入 `SystemReminder`。`tools/execute` 路径靠近 skills 目录时中途发现。`user-invocable` 技能登记成 slash extras（不可盖 `RESERVED_SLASH`）；另登记 `/skills` overlay。fail-open |
| `tui.slots` | `"tui.slots"` | — | 动态包登记的 TUI 插槽（数据+回调，不是 ratatui widget）。`install_app` 始终挂上。TUI 用一次通用 `Overlay::Slot` 臂 |
| `agent-presets` | `"agentPresets"` | — | 组装 Agent：YAML 定义人设 + 工具允许名单，运行时只过滤 live `"tools"`。**正在运行的动态包**用 `Tools::register_dynamic` 登记的 extra 工具、以及 **已开启的 MCP 工具**（`register_mcp`，公名 `mcp_{server}__{tool}`，经 `use_tool` 调度）会穿过允许名单。层：crate `presets/<id>/agent.yml` + `agents/*.yml` < `~/.dock/presets` < 项目 `.dock/presets`。仍可读旧 `<id>.yml`（目录优先）。加一个目录就是一个 Agent。内置 `code` / `minimal` / `cordis` / `warden`（守望）。`code`/`cordis` 的 `agents/` 名册是 `general-purpose` / `explore` / `plan`（项目层可加，如 `.dock/presets/创造/agents/review.yml` 叠到 `cordis`）；`warden` 是 `岑` `锁` `甲` `乙` `丙` `衡` `验` `观` `突击`（不要用拼音 id）。发给模型的 `subagent`/`task` 把 `subagent_type` 收成当前名册 enum。省略 `tools` = 全部已注册工具。新建模式默认写**当前工作区** `.dock/presets/<id>/agent.yml`（`/preset` n/d 有项目层时落到这里；系统提示注入 `.dock/presets` 与 `.dock/presets/<模式>/agents`，不用绝对 `{cwd}`；id 必须 `[a-z0-9][a-z0-9-]*`，汉字目录只叠内置）。新建子代理默认写 `.dock/presets/<当前模式 id>/agents/<type>.yml`。空名册仍注入这两处路径。只有用户明确要求保存到全局才写 `~/.dock/presets/`。写完人设后 `subagent` 校验立刻重读；本轮刚写完时用 `subagent`（`reload_roster: true`）刷新 enum。新建模式写完后用 `/preset` 应用该 id。改 crate `presets/` 要重新编译 |
| `tool-web` | → `"tools"` | `web_fetch` `web_search` | Grok SSRF / 同 host 重定向 / htmd。`web_search` 无 xAI 账号，走同一套 fetch 打公开 HTML 索引 |
| `tool-browser` | `"browser"` + `"tools"` | `browser_open` `browser_navigate` `browser_navigate_back` `browser_snapshot` `browser_click` `browser_hover` `browser_type` `browser_press_key` `browser_select_option` `browser_fill_form` `browser_wait_for` `browser_drag` `browser_handle_dialog` `browser_file_upload` `browser_resize` `browser_evaluate` `browser_console_messages` `browser_network_requests` `browser_screenshot` `browser_tabs` `browser_close`（按需） | **BUA P2**：in-process **chromiumoxide** CDP。P1 之外增加 `browser_evaluate`（**权限门同 bash**：`needs_permission` + `blocked_in_plan`，经 `tools/execute` / `use_tool` 命中）、只读截断的 `browser_console_messages` / `browser_network_requests`（会话连接时挂 Network/Runtime 监听，保留最近 N 条）、以及 **同域 iframe**：`browser_snapshot` / `browser_evaluate` / `browser_click` 可选 `frame`/`frame_selector`（CSS 选 iframe）；跨域或找不到则明确报错。无 Node/Playwright；CUA 像素点击仍不做。`register_deferred`：不进 sampler / `specs_for_model`。Fiber dispose 关掉 Chromium。`browser_screenshot` 写路径给 `/browser`，并经 `ToolResult.images` 进多模态（见「工具结果图」）。`/browser` 驾驶舱列 P0–P2 + 最近 evaluate/network，并可 **`h` 切换有头/无头**（`[browser].headed`，默认无头；`DOCK_BROWSER_HEADED` 任意非空覆盖；切换后需 close/open 才作用于已开会话。有头 launch：`with_head().viewport(None)` + 初始 `window_size`，避免默认 800×600 Emulation 只画一角；Chromium CLI `.arg` 勿带前导 `--`（库会再加））。`code` / `cordis`（+ general-purpose）允许名单含这些 `browser_*`；`minimal` / `warden` 主代理不含 |
| `tool-computer` | `"computer"` | — | **CUA C0**：薄驾驶舱 named `"computer"`。桌面键鼠经 trycua `cua-driver` MCP（公名 `mcp_cua-driver__*`，现有 `mcp-client`），**不**自研键鼠 / Docker。live-lookup `"mcp"` 看 cua-driver 是否就绪；`/computer` 为 TUI CATALOG builtin（`Overlay::Computer`）。全部 `mcp_cua-driver__*` 与 bash 同级 permissions / 计划门。挂在 `mcp-client` 之后；fiber dispose 注销。见下文「Computer / CUA」 |
| `tool-todo` | `"todos"` + `"tools"` | `todo_write` | Grok merge/replace |
| `plan-mode` | `"planMode"` + `"tools"` | `enter_plan_mode` `exit_plan_mode` | 计划文件 `.dock/plan.md`。计划态挡住 bash / 写文件等 |
| `tool-ask-user` | `"ask"` + `"tools"` | `ask_user_question` | 事件 `ask/pending` |
| `tool-jobs` | → `"tools"` | `get_task_output` `wait_tasks` `kill_task` | 查/等/杀后台 bash |
| `tool-scheduler` | → `"tools"`（live `"cron"`） | `scheduler_create` `scheduler_list` `scheduler_delete`（按需） | 包着已有 `"cron"`。`register_deferred`。`fire_immediately` 立刻跑第一次；循环 7 天后过期（过期不跑最后一次，滚动区留中文说明）；最多 50 条；`task_id` 原地更新并保持相位。滚动区 **Loop 卡**（设定 / 列表 / 关闭）。`/tasks` 里 `x` / `[✗]` 关闭 |
| `tool-task` | `"subagents"` + `"tools"` | `task` | Grok `ChannelBackend` + coordinator actor；dock `ChildRunner` isolate `"sessions"`+`"turn"`+`"agentPresets"`。提供 named `"subagents"`。`task` 一次性收集：后台回 `get_task_output`，完成后 `resume_from`。不要在这个插件里挂 `subagent` |
| `tool-subagent` | inject `"tools"` + `"subagents"` | `subagent` `send_message` `list_agents` `interrupt_agent` `report` | **独立 grain**，不是 `task` 的包装。`subagent_type` = 当前模式 `agents/<id>.yml` 角色 id（模型侧参数 enum 即这份名册）。写完新 YAML 后用同一工具 `reload_roster: true` 重读名册（不 spawn）。后台回 `send_message` / `list_agents` / `interrupt_agent`。`send_message`：idle 时 queued 与 urgent 都立刻开下一轮并在返回前把状态打成 running；urgent 只在 running 时才是 send-now。`report` 是子代理和主代理的**多轮通道**（同轮可多次），不是一次性交卷；助手正文到不了父级。mailbox 子代理若本轮未 `report` 就 idle，运行时代转发回合输出，避免主代理空等。必须挂在 `tool-task` 之后（live-look `"subagents"`）。`warden` 主代理只用这套，工具名单不含 `task` / `get_task_output` |
| `tool-memory` | `"memory"` + `"tools"` | `memory_search` `memory_get`（按需） | 本地 `~/.dock/memory` / `.dock/memory`。`register_deferred`：不进 sampler，经 `search_tool` / `use_tool` |
| `tool-monitor` | → `"tools"`（live `"jobs"`） | `monitor`（按需） | 长命令 stdout 盯梢。`register_deferred` |
| `tool-goal` | `"goal"` + `"tools"` | `update_goal`（按需） | Grok oneshot ack + drain。`objective` 可在无 `/goal` 时由模型自己开目标；无目标且只有 message/completed 时仍 `HarnessDisabled`。进度卡在滚动区。`register_deferred` |
| `tool-lsp` | `"lsp"` + `"tools"` | `lsp`（按需） | Grok `LspManager`/`dispatch`。第一次调用时读 `~/.dock/lsp.json` 与 `<cwd>/.dock/lsp.json`（项目盖用户）；没有配置则按工作区标记探测 PATH 上的 `rust-analyzer` / `typescript-language-server` / `gopls` / `pyright-langserver`（标记可在子目录，跳过 `node_modules` / `target`）。`/lsp` 把缺的服务器写入项目 `.dock/lsp.json`（不覆盖已有条目）；`/lsp user` 写 `~/.dock/lsp.json`。`search_replace` / `write_file` 之后后台 `didChange`。相对路径按 cwd 展开。没服务器时 fail-open（工具仍注册，调用返回配置说明）。`register_deferred` |
| `tool-skills` | → `"tools"`（live `"skills"`） | `skill`（按需） | 按需读 `SKILL.md` 正文（去 frontmatter）+ `$ARGUMENTS` / `$SKILL_DIR`。参数 `name` 必填、`args` 可选。返回 skill 信封和同目录最多约 10 个附属文件名。`disable-model-invocation` 的技能不进 listing / 本工具，斜杠仍可用。没 skill 目录时工具仍注册。`code` / `cordis` 允许名单含 `skill`。`register_deferred` |
| `tool-workflow` | `"workflows"` + `"tools"` | `workflow`（按需） | Grok Rhai 引擎（`vendor/xai/workflow`）+ 同款 oneshot ack。Host `SpawnAgent` live-lookup `"subagents"`，并把 `capability_mode` / `output_schema` 传给子代理（JSON 输出会解析给脚本）。内置 `deep-research` 脚本在 `cordis-spine/src/workflow/workflows/deep_research.rhai`；磁盘扫描 bundled → 内置 → `{cwd}/.dock/workflows/<name>.rhai` → `~/.dock/workflows/`（同名不覆盖已有）。向 `"context"` 登记 listing 段（窗口 token ×4 ×8%）。每个目录项登记 slash extra（`kind: tool`，`text=workflow`；不可盖 `RESERVED_SLASH` / `/skills`）。`/name` 与 `/workflow <name>` 直接 `Tools::execute`，不经模型。`tools/execute` 路径靠近 workflows 目录时中途发现。`code` / `cordis` 允许名单含 `workflow`。`register_deferred` |
| `mcp-client` | `"mcp"` + `"tools"` | `search_tool` `use_tool` | stdio + Streamable HTTP。先走 MCP `2026-07-28`（无 initialize / 无 session，`_meta` + `MCP-Protocol-Version`）；服务器仍是 initialize 时代则回退 `2025-11-25`。fail-open。MCP 工具 `inputSchema` 原样 `register_mcp`，公名 `mcp_{server}__{tool}`，与 `register_deferred` 的本地工具一起 **不进** sampler `specs_for_model`。模型只看见静态描述的 `search_tool` / `use_tool`（`code` / `cordis` / `warden` 允许名单含这两项，`minimal` 不含）。`search_tool` 按 query 匹配名/组/描述，只返回命中项（每项完整 `input_schema`），limit 默认 5、最大 255；`total_hidden_tools` 是目录总数。`use_tool` 调度 MCP 或按需本地工具，输出 20KB 帽（search_tool 无帽）。第一类工具误走 `use_tool` 会纠正。`/mcps` Space 立刻 `dispose` 注销或追加注册，不改系统提示；目录变化在 `agent/pre-step` 与开关时写服务器级 `<system-reminder>`（已连接/已更新/已断开 + 数量，不含 schema）。`tools/list` 跟 `nextCursor`（最多 64 页）；`notifications/tools/list_changed` 50ms 合并后重列。advertise `elicitation.form` + `elicitation.url`；stdio 读循环 / HTTP POST SSE 按序 / GET SSE 收 `elicitation/create`。HTTP GET 长连接；initialize 时代 `Mcp-Session-Id` 的 POST 404 会重新握手再试一次。HTTP 服务器 `i` 浏览器 PKCE OAuth（DCR 或 `oauth.clientId`），token 在 `~/.dock/mcp_credentials.json`，启动不自动开浏览器 |
| `dynamic-runner` | `"dynamicCordisRunner"` | — | 会话注册表 + 磁盘永久插件。热挂体走 `ctx.plugin` / `fiber.dispose`。会话定义盖章 `Sessions::identity()`；磁盘插件 `session_id` 为 `*`，所有会话可见。`cordis_promote` 写 `{cwd}/.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/`（目录名即 pluginId）；`install_app` 自动加载（fail-open，不走权限 overlay） |
| `compact` | `"compact"` | — | Grok 会话压缩。`install_app` 在 `llm` 之后挂。手动 `/compact [说明]`；上下文达到窗口 85% 时 `maybe_auto`（loop live-lookup，工具轮次结束后、下次采样前）。摘要 prompt / 清洗 / 阈值从 grok-build `xai-grok-compaction` 拷来。成功后 **滚动区保留原对话**（Grok pager 也不擦 scrollback），只把 sampler 历史换成摘要前缀；占用数字按模型历史计。占用 overlay 是 TUI live-look `"context"`，不是模型工具 |
| `tool-cordis` | → `"tools"`（inject `"dynamicCordisRunner"` + `"context"`） | `cordis_*`（按需） | 预置工厂 `echo` / `note` / `hold` / `slash`，加上 `factory: "rhai"`（`source` 在 define 时 compile，run 时 eval `apply`）。`register_deferred`。系统提示只留短指针（`search_tool` 查 cordis）；教程在 `skills/cordis-plugin-development/SKILL.md`。`host.on("session/event")` 观察会话日志。`cordis_call` 经 `Tools::execute` 试调任意 live 工具。`cordis_inspect` `what`: `services` / `builtins` / `events` / `slots` / `temporary` / `permanent`。`inspect_self` 对 Rhai 包回传 source。用户文本 `@pluginId` 在 `agent/pre-step` 注入身份 reminder（不含源码）。审批走权限 overlay（`cordis_run` `cordis_promote`）。`/cordis` 列出内存与磁盘层。Skill：`/cordis-plugin-development` 或 `skill` 工具加载 `skills/cordis-plugin-development/SKILL.md` |

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

- **接入**：用户本机安装 `cua-driver`；Dock 用现有 `mcp-client` stdio。`config.toml` 样例见 [config.toml.example](config.toml.example) 与下文。公名 `mcp_cua-driver__{tool}`（服务器键名必须是 `cua-driver`），与其它 MCP 一样 **不进** sampler / `specs_for_model`，经 `search_tool` / `use_tool`。
- **stdio 帧**：Dock MCP stdio **默认 NDJSON**（每行一条 JSON-RPC，对齐 cua-driver 0.24+）。LSP `Content-Length` 仅显式 `framing = "content-length"`。可选 `framing = "auto"`：先 CL 探测，`-32700`/parse 则 **kill+respawn** NDJSON（不在同一 stdin 硬切）。无需 Python 桥。
- **权限 / 计划门**：所有 `mcp_cua-driver__*` 与 `bash` 同级（`needs_permission` + `blocked_in_plan`）。`use_tool` 内层 `execute` 会命中该门。
- **Allowlist**：MCP extras 仍按现规则 **穿过** Agent preset allowlist；但 `code` / `cordis`（含 general-purpose）须保留 `search_tool` / `use_tool`。`minimal` / `warden` 主代理不含这两项则调不到 cua-driver。
- **勿混 BUA**：`cua-driver` 自带的 `browser_*` MCP 工具 ≠ Dock chromiumoxide `browser_*`。网页自动化优先 Dock BUA；桌面键鼠 / 开应用走 cua-driver。
- **Linux 坑**（写进安装说明）：需要 **X11 或 XWayland**（原生 Wayland 仍预览）；`DISPLAY` / `XAUTHORITY`；`at-spi2-core`（+ 必要时 toolkit-accessibility）否则 AT-SPI / `get_window_state` 弱；把 `~/.local/bin` 放进 `PATH`，或用 `cua-driver mcp-config` 给出的绝对 command；telemetry 默开，可 `cua-driver telemetry disable`。
- **TUI**：`/computer` 薄驾驶舱（连上 / 未装 driver、审批提示）；named `"computer"` 已挂；完整 live overlay 由 TUI 同 PR 跟。不嵌真桌面。
- **冒烟**：装好后 `/mcps` 见 `cua-driver` → `search_tool` 查桌面工具 → `use_tool`（先过权限门）完成截图或点按一类动作。

**本机边界（BUA 在 Linux + X11 冒烟，`cua-driver` 0.24.x）**

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
cua-driver doctor          # 查 DISPLAY / X11 / AT-SPI
cua-driver mcp-config      # 打印推荐 command/args（可抄进 config.toml）
# 可选：cua-driver telemetry disable
```

`~/.dock/config.toml`（或项目 `.dock/config.toml`）样例——优先 PATH 上的 `cua-driver`：

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
