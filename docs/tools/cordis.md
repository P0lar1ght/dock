# 动态 Cordis 插件

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `dynamic-runner`

- **ctx**：`"dynamicCordisRunner"`
- **模型工具**：—

会话注册表 + 磁盘永久插件。热挂体走 `ctx.plugin` / `fiber.dispose`。会话定义盖章 `Sessions::identity()`；磁盘插件 `session_id` 为 `*`，所有会话可见。`cordis_promote` 写 `{cwd}/.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/`（目录名即 pluginId）；`install_app` 自动加载（fail-open，不走权限 overlay）

项目级的 `{cwd}` 是**调用这颗工具的会话**的工作目录（桌面 GUI 每个会话一个项目）。
- 不在任何一轮里（启动时）才是进程 cwd：TUI 从哪启动就加载哪的项目插件。
- 桌面 GUI 开某个项目的会话时，网关加载那个项目的 `.dock/plugins`（同名已装的跳过）。
- `cordis_define` 的 `source_path` 按同一个项目根解析。

## `tool-cordis`

- **ctx**：→ `"tools"`（inject `"dynamicCordisRunner"`）
- **模型工具**：`cordis_*`（按需）

预置工厂 `echo` / `note` / `hold` / `slash`，加上 `factory: "rhai"`（`source` 在 define 时 compile，run 时 eval `apply`）。按需由预设 `on_demand_tools` 决定。不进系统提示；写法在内置技能 `cordis-plugin-development`（见文末）。`host.on` 可挂八个事件：`session/event`（事后观察）、`agent/step-start` 与 `agent/turn-end`（拦截，返回 `<system-reminder>` 正文或 `()`，宿主代跑 `next`、脚本吞不掉链，order 槽 50 排在内建 goal(20) 之后；内建 todo(10) 的回合末只投「记账」票（一步、只给 `todo_write`、然后收尾），任何续跑票都排在记账前面；用户已排队下一条时宿主直接不问脚本；抛错或返回非字符串都当没意见）——这三个任何插件都能挂。另有五个**只给磁盘来源插件**（插件目录 / 设置启用 / 审批过的 `cordis_promote` 落盘）的流程钩子：`agent/pre-step`（拒掉一整轮）、`system-prompt/assemble`（往系统提示加一段，固定槽位、字节稳定）、`tools/pre-execute`（拒绝一次调用或改道到别的工具，拒绝在权限门之前、改道后按新名字重过门）、`tools/execute`（事后替换模型看到的结果）、`llm/stream`（改写采样输出，改不了请求）；会话里 `cordis_define` 的包打不开这五个 —— 否则模型能给自己开闸门、能改自己的系统提示。六颗工具：`cordis_inspect`（`what` 看目录 / `pluginId`+`packageId` 看单个 Plugin，原 `cordis_inspect_self` 已并入）、`cordis_define`、`cordis_run`、`cordis_call`、`cordis_stop`（`drop: true` 即原 `cordis_undefine`）、`cordis_promote`。合并只在**同一门类**内做：`cordis_run` / `cordis_promote` 走权限门（`acp::gated_builtin` 按**工具名**判），不能与无门的工具并成一颗。`cordis_call` 经 `Tools::execute` 试调任意 live 工具。动态包经 `host.register_tool` 注册的工具不进采样表（`register_dynamic`）：包挂上、卸下都不改工具表，前缀缓存不因此作废；模型用 `search_tool` 找（目录里归 `dynamic` 组）、`use_tool` 调，仍绕过预设允许名单（能力档位照查，未归类的只有 `all` 档放行）；`/context` 里算进「本地按需」。同一轮要验证用 `cordis_call`。`cordis_inspect` `what`: `services` / `builtins` / `events` / `slots` / `temporary` / `permanent`。`services` 分两段：**脚本够得着的**（`tools` / `slash` / `tui.slots` + 活着的 bag，带 `host.*` 签名）与**只是挂着的**（名字可满足 inject，但 `host.get` 回 `()`，要用得走 `host.call_tool` 调对应模型工具）。`inject` 原样进内核 `Inject`，按名字解析、类型擦除，所以挂着的名字都能满足它——`waiting for` 因此只在 fiber 非 active 时才列名字（内核说了算），active 就是跑起来了。`cordis_inspect` 带 `packageId` 时对 Rhai 包回传 source。用户文本 `@pluginId` 在 `agent/pre-step` 注入身份 reminder（不含源码）。审批走权限 overlay（`cordis_run` `cordis_promote`）。`/cordis` 列出内存与磁盘层。Skill：`/cordis-plugin-development` 或 `skill` 工具加载（见下）

### 技能 `cordis-plugin-development`（内置）

随二进制发，启动物化到 `$DOCK_HOME/bundled/skills/`，任何项目都能加载。
源码在 `cordis-spine/src/tools/skills/builtin/cordis-plugin-development/`，分三层：
- `SKILL.md`：选载体、三个概念、文件优先流程、六颗工具、路由表、常见坑。只有这层随 `skill` 加载。
- `references/*.md`：package / host-api / ui / hooks / http / lifecycle / rhai-pitfalls / troubleshooting，按需 `read_file`。
- `examples/`：`plugin.toml`、`ping.rhai`、`memo.rhai`、`api-tool.rhai`。
加文件要登记进 `builtin.rs` 的 `BUILTIN_FILES`（`shipped_skills_conform_to_spec` 会查）。
例子能真跑：`cargo test -p cordis-spine --test dynamic -- bundled_skill_examples`。
