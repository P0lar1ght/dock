# Cordis 插件

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

插件就是一个目录：`{项目}/.dock/plugins/<id>/`（覆盖同名的用户级）或
`~/.dock/plugins/<id>/`，里面是 `source.rhai` 加可选的 `plugin.toml`。目录名即插件 id。
模型侧只有一条生命周期（对齐 DSH 的 Plugin Manager）：用文件工具写 → `cordis_plugin
reload` → 用 `use_tool` 调新工具验证 → 再改 → 再 reload。

以前的 `cordis_define` / `cordis_run` / `cordis_call` / `cordis_stop` / `cordis_promote`
已经退役：会话插件和磁盘插件两套生命周期让一次修改要三次调用，插件 id 跟着分裂，
define 和 run 校验的路径也不一样。历史会话里的这些调用照样能渲染。

## `dynamic-runner`

- **ctx**：`"dynamicCordisRunner"`
- **模型工具**：—

插件运行时：热挂体走 `ctx.plugin` / `fiber.dispose`，挂在 `cordis-dynamic` 组 fiber 下。

- **启动与开项目**：`install_app` 装上用户级和进程 cwd 的项目级插件；桌面 GUI 打开某个
  项目的会话时，网关装上那个项目的 `.dock/plugins`（同名已装的跳过）。都不走权限确认，
  fail-open。
- **reload**（`DynamicRunner::reload_disk`）：把那一刻的 `source.rhai` 读成一个不可变的
  新 Package 跑起来；在跑的那版先整个卸掉再起新版。新版起不来就自动回到上一版继续跑，
  原因留在 `reload_error` 里给 `cordis_inspect` 和设置页看，下一次成功才清掉。
  `plugin.toml` 写着 `enabled = false` 的顺手改成 true。
- **disable**：停掉，写 `enabled = false`，文件留着。
- **登记账本**：每次运行记下 Rhai `host.*` 实际登记的工具 / 斜杠 / 面板 / 钩子 / bag /
  设置卡（`Surface`），reload 回执和 `cordis_inspect` 都从这里读。
- **程序侧 API**：`define` / `run` / `stop` / `undefine` 还在（测试、设置页用），按
  `Sessions::identity()` 分会话；没有模型工具调它们。

`plugin.toml` 每个键都可选：`name`（默认目录名）、`purpose`、`enabled`（默认 true）、
`factory`（默认 `rhai`；`slash` 时另写 `command` / `kind` / `text`）、
`[permissions] http = ["api.example.com", "*.example.com"]`。认不得的键不挡加载，
reload 回执会提醒。

项目级的 `{项目}` 是**调用这颗工具的会话**的工作目录（桌面 GUI 每个会话一个项目）；
不在任何一轮里（启动时）才是进程 cwd。

## `tool-cordis`

- **ctx**：→ `"tools"`（inject `"dynamicCordisRunner"`）
- **模型工具**：`cordis_inspect`、`cordis_plugin`。创造预设（`cordis`）里常驻；
  `code` / `minimal` / `warden` 不带；从零新建的预设默认按需（种子 `on_demand_tools` 带
  `cordis_*`）。不进系统提示；写法在内置技能 `cordis-plugin-development`（见文末）。

### `cordis_inspect`（只读，免确认，可并行）

- 不带参数：这个会话的两个插件根，以及每个插件的状态（`running` / `running (last reload
  failed)` / `failed` / `disabled` / `shadowed` / `not loaded`）和第一行错误。
- `plugin: "<id>"`：目录、开关、正在跑的那一版（Package / Run id）和它登记了什么、
  在等哪些依赖、声明的 HTTP host、最近的错误、被盖住的同名目录、`plugin.toml` 警告。
  不回源码（就在目录里，用 `read_file`）。
- `what`：`plugins` / `services`（脚本够得着的 `tools` / `slash` / `tui.slots` + 活着的
  bag，与只是挂着的名字分两段）/ `tools` / `builtins`（Rhai host API 签名）/ `events`
  （八个钩子的载荷与返回值）/ `slots`。

### `cordis_plugin`（过权限门，计划模式里挡）

- `action`：`reload` 或 `disable`（`enable` / `load` / `stop` 这类同义词照认）。
- `id`：目录名。`scope`：`project` / `user`，默认项目级优先。
- 回执是中文：结论一行、`登记：` 一行、HTTP 声明、被盖住的同名目录、`plugin.toml` 警告，
  最后一行 `下一步：`——成功时教怎么验证（有工具就给 `use_tool` 的写法），失败时按报错
  类型给一条修法。失败的 reload `is_error = true`。
- **确认按插件记**：键是 `cordis_plugin <id>`，「始终允许」只放行这一个插件，迭代它不用
  每次点；换一个插件还会问。摘要写着插件目录、`host.on("…")` 的钩子名、声明的 HTTP host。

## 宿主能力（MCP 做不到的那一面）

MCP 要等模型来调；插件包在模型外面，每一次调用、每一轮都会经过它。插件的主业是**改 agent
的行为**，工具只是其中一样。Rhai `host` 上（签名以 `cordis_inspect what:"builtins"` 为准）：

| 能力 | API | 说明 |
| --- | --- | --- |
| 拦 / 要确认一次调用 | `host.on("tools/pre-execute")` 返回 `#{ deny }` / `#{ ask }` / `#{ rewrite }` | `ask`：自动批准、「始终允许」都不算数（`PreExecute::ask`，单调，deny 优先）；载荷带解析好的 `args` |
| 事后改结果 | `host.on("tools/execute")` | 载荷带这次调用的 `arguments` / `args`（注册表把参数经 task-local 交给同步跑的监听器）和 `identity` / `main` |
| 结构化会话事件 | `host.on("session/entry")` | `user` / `assistant` / `tool` / `reminder` / `notice` / `turn-end`，带页身份；`assistant` 来自采样收尾事件 `llm/finished`（`LlmFinished`：最终输出 + 用量 + 缓存命中 + 费用） |
| 主动找 agent | `host.followup(text)` / `host.followup(identity, text)` | 进那一页的信箱（`ParentNotice::Plugin`，同后台任务跑完通知）：在跑就下一步带上，闲着的主会话页叫醒开一轮；一页 5 秒内最多叫醒一次 |
| 定时器 | `host.every(秒, f)` / `host.after(秒, f)` | `every` ≥5 秒、`after` ≥1 秒，都支持小数秒且 ≤2592000 秒（30 天）；超限返回脚本错误，不占名额；一次运行最多 16 个，随运行取消 |
| 问模型一句 | `host.llm(prompt)` / `host.llm(#{ system, prompt })` | 这一页的模型，不带工具，不进会话日志、不计入 `/usage`，阻塞调用方 |
| 小存储 | `host.load` / `host.save` / `host.forget` | `$DOCK_HOME/plugin-state/<id>-<目录指纹>.json`，≤1 MiB，跨 reload 和重启 |
| 回答下面的卡片 | `host.card` / `update_card` / `remove_card` | named service `"turnCards"`，GUI `turn/cards/*`，见 [PLUGIN-VIEWS](../PLUGIN-VIEWS.md#回答下面的卡片) |

脚本里的「当前页」：工具体、钩子里是发起这一步的那一页；定时器、卡片按钮这类不在任何一轮里
的回调，用这颗插件最近打过交道的那一页（卡片按钮用卡片所在的那一页）。

技能 `cordis-plugin-development` 带三个行为插件例子，都有测试：`guard.rhai`（项目规约守门）、
`done-check.rhai`（改了 `.rs` 没跑测试不许收尾 + 卡片）、`trim-output.rhai`（去 ANSI、裁头尾）。
已有 Claude Code / Codex hooks 配置的不用写插件，见 [HOOKS](../HOOKS.md)。

## 权限

- 五个流程钩子（`agent/pre-step`、`system-prompt/assemble`、`tools/pre-execute`、
  `tools/execute`、`llm/stream`）只给**从插件目录装载**的包；门就是「加载这个插件」：
  reload 要用户确认，自动装载的是用户放进插件目录的东西。程序侧 define 的会话包打不开。
  另三个（`session/event`、`agent/step-start`、`agent/turn-end`）任何包都能挂。钩子的
  排序、返回值和不吞链的规矩见技能 `references/hooks.md`。
- `http_request`：按 host 确认（`http_request <host>`）。`plugin.toml` 声明过的 host
  不再逐次问；声明只认主机名（可带 `*.` 前缀、只配子域），不认光一个 `*`。
- 脚本里的 `host.call_tool` 与 HTTP 确认都落在**发起这次调用的那一页**：动态工具的
  脚本跑在阻塞线程上，工具体在进线程前抓住 `exec_ctx()` 再挂上。以前拿插件 fiber 所在的
  根算，确认框弹到第 1 页——GUI 里那一页没人看，调用一直挂着。
- 动态包经 `host.register_tool` 注册的工具不进采样表（`register_dynamic`）：挂上卸下都
  不改工具表，前缀缓存不因此作废；模型用 `search_tool` 找（目录里归 `dynamic` 组）、
  `use_tool` 调，绕过预设允许名单（能力档位照查，未归类的只有 `all` 档放行）；
  `/context` 里算进「本地按需」。
- 用户文本 `@<id>` 在 `agent/pre-step` 注入那个插件的目录、名字、状态（不含源码）。
- `/cordis` 列出插件目录与状态。

### 技能 `cordis-plugin-development`（内置）

随二进制发，启动物化到 `$DOCK_HOME/bundled/skills/`，任何项目都能加载。
源码在 `cordis-spine/src/tools/skills/builtin/cordis-plugin-development/`，分三层：
- `SKILL.md`：选载体、插件目录、标准流程、两颗工具、路由表、交付规矩。只有这层随 `skill` 加载。
- `references/*.md`：package / host-api / ui / hooks / http / lifecycle / rhai-pitfalls / troubleshooting，按需 `read_file`。
- `examples/`：`plugin.toml`、`ping.rhai`、`memo.rhai`、`api-tool.rhai`。
加文件要登记进 `builtin.rs` 的 `BUILTIN_FILES`（`shipped_skills_conform_to_spec` 会查）。
模型可见文字（工具描述、技能全部文件）只许点名活着的 `cordis_*` 工具、不许提退役的那几个：
`every_cordis_name_in_model_text_is_a_live_tool` 守着。
例子能真跑：`cargo test -p cordis-spine --test dynamic -- bundled_skill_examples`。
