# 架构

Dock 的 harness 是一棵 Cordis 插件树。内核是 crate `cordis`（`Context`、`inject`、named service、waterfall、fiber 生命周期）。**没有可私自打补丁的内核**：新行为只能是再挂一颗插件，或接到已有 named service / waterfall 上。

硬规则在根 [AGENTS.md](../AGENTS.md)；本文件是可重复查的目录地图与不变式。工具名单见 [TOOLS.md](../TOOLS.md)，人操作的面见 [CLI.md](../CLI.md)。

## 目录地图

```
cordis-rust/             crate `cordis`：Context、inject、named service、fiber、waterfall
cordis-spine/            Agent 循环、工具粒、MCP、会话、预设、权限；install_app
  src/*.rs               各 named service 与插件粒（llm、tools、permissions、slash、mcp…）
  src/skills/            skills 发现与 listing（扫描顺序见下）
  presets/               内置 Agent 预设 YAML（code / minimal / cordis / warden）
  tests/                 round.rs（install_app_registers）、dynamic.rs、subagents.rs
cordis-tui/              全屏终端 UI 插件：theme、scrollback、prompt、statusBar、shortcuts…
cordis-gateway/          回环 HTTP/WS 插件：配对、dock.1 JSON-RPC 投影、slash list|execute
cordis-app/              二进制入口：一个 Context，install_app + agent-loop + gateway + tui
cordis-render/markdown/  crate `cordis-markdown`
cordis-render/mermaid/   crate `xai-grok-mermaid`
embed-sdk/               宿主页 JS SDK（`dist/dock-embed.js`，协议 dock.1）
vendor/mermaid/          冻结的 mermaid 布局栈（dagre / graphlib / to-svg / ordered_hashmap）
vendor/xai/              冻结的 xai 拷贝（fuzzy-file-search、workflow、grok-tools）
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

挂载有序：工具粒都在 `workspace_tools` 之后、`llm` 之前 `register`；`compact` 在 `llm` 之后（`inject "llm"`）；`tool-subagent` 在 `tool-task` 之后（live-lookup `"subagents"`）。完整顺序与每颗粒的工具名见 TOOLS.md。

## named service 与插件粒

| 层 | 例子 | ctx key |
|---|---|---|
| Spine 五件套 | `sessions` `llm` `tools` `systemPrompt` `agents` | 同名 |
| 循环 | `agent-loop` 提供 `LoopHandle` | `agentLoop` |
| 其它 spine | `context` `settings` `turn` `permissions` `cron` `jobs` `todos` `planMode` `ask` `mcp` `goal` `lsp` `skills` `subagents` `memory` `browser` `computer` `workflows` `slash` `agentPresets` `dynamicCordisRunner` `compact` | 同名 |
| 工具插件 | `tool-web` `tool-browser` `tool-todo` `plan-mode` `tool-ask-user` `tool-jobs` `tool-scheduler` `tool-task` `tool-subagent` `tool-memory` `tool-monitor` `tool-goal` `tool-lsp` `tool-skills` `tool-workflow` `mcp-client` `tool-cordis` | 向 `"tools"` `register` |
| TUI | `theme` `tui.scrollback` `tui.prompt` `tui.statusBar` `tui.welcome` `tui.shortcuts` `tui.pairing` | 同名 |
| 回环网关 | `gateway` | `"gateway"`（`GatewayRef`），事件 `gateway/pairing` |

`settings` 持有模式、模型、权限开关；TUI 只把按键映射成 Action，再 live-lookup `settings`。计划是独立模式，不是第三种权限。会话落盘在 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` + `chat_history.jsonl`），不是项目 `.dock/`。

## 一轮怎么跑

TUI 从不持有循环：按键映射成 `SessionCommand` 交给 `session_actor`，actor 管队列（提交 / 立即发送 / 提前 / GoalSummary 续跑 / 子代理 mailbox 续跑），每次取一条调 `agent-loop` 的 `LoopHandle`。默认 driver 是 `GrokStep`：

```
agent/pre-step               每轮开始，一次
system-prompt/assemble       每轮一次
llm/stream                   枢纽：出工具调用 → tools/execute（权限 / 计划门）→ 回采样
                             出文本 → turn 结束
```

安全上限 256 步；`/goal` 在外层多轮直到 `update_goal(completed)`。换 driver 只换 `agent-loop` 插件，不动 actor。

## 不变式

1. **换插件，不改 loop。** 新 UI 面是 `tui.*` 插件；新采样是 `llm` 插件。不要把功能焊进 `event_loop` 或 `agent-loop`。不要调用 `xai_grok_pager::app::run`，不要 spawn Grok `MvpAgent`。
2. **一张 `"tools"` 表。** 工具能力插件 `inject: ["tools"]` 后 `ctx.tools.register()`；MCP 也进同一张表，不是 `tools.mcp` 之类的副表。
3. **live-lookup，不捕获。** 调用点 `ctx.get` / `ctx.require`。不要把 `Arc<T>` 关进长生命周期闭包（TUI frame、HTTP 重试、cron tick、sampler `on_delta` 这类闭包里也要重新 `get`）。
4. **扩展走 waterfall。** 拦截接 `on_waterfall`，默认实现放在 `waterfall(..., || default)` 的闭包里。监听必须把控制权交给下一环，不许吞链。
5. **提示词分段归贡献插件。** `inject: ["context"]` 后向 `ContextBook` 登记 `set_base` / `section` / `replace_base`（fiber dispose 注销）。`systemPrompt` 只是 facade；基座只写身份与按需发现，**不列工具名**。计划 / 目标走历史尾部 `<system-reminder>`，不进系统提示。
6. **MCP / 按需工具 fail-open。** 连不上仍是 `Active`，往 `"mcp"` 写空 / 失败状态。不常用本地工具（`register_deferred`：scheduler / memory / monitor / goal / lsp / skill / workflow / cordis_* / browser_*）注册进 `"tools"` 但**不进** sampler 的 `specs_for_model`；模型侧固定 `search_tool` + `use_tool`。工具描述保持静态，以保住 tools JSON 前缀缓存。
7. **工具名不撞车。** MCP 公名 `mcp_{server}__{tool}`，不能盖掉 `bash` 之类的内置名。
8. **Gateway 默认不监听。** 只绑 loopback（首选 `127.0.0.1:18991`，占用往上找，同端口再试 `[::1]`；`DOCK_GATEWAY_BIND` 只改首选）。`/pair` 开启；鉴权靠配对 + 一次性 ticket + 回环，CORS 反射 Origin 是有意的。
9. **reasoning 不混进助手 markdown。** 推理走 `StreamDelta::Reasoning` / `LlmOutput.reasoning`；工具卡折叠显示 name + 参数摘要，展开先「输入」再「输出」，参数在 `LogEvent::ToolExecute.arguments`。
10. **skills 覆盖顺序。** `scan_all` 按 Bundled（`{cwd}/skills`）→ User（`~/.dock/skills`）→ Agents（`{cwd}/.agents/skills`）→ Project（`{cwd}/.dock/skills`）合并，同名后者覆盖前者。

## 磁盘

- `~/.dock`（可用 `DOCK_HOME` 覆盖）：config、presets、plugins、skills、memory、`sessions/`、`mcp_credentials.json`。
- 项目 `.dock/`：覆盖 config、presets、plugins、skills、`plan.md`、workflows。
- HTTP MCP 的 OAuth token 在 `~/.dock/mcp_credentials.json`，**不写进** `config.toml`。它不是 grok.com 账号登录。

## 已知边界

- `cordis-gateway` 的 rustc **1.88+** 要求：README 声明，本仓无 toolchain 文件兜底，说明见 [DEVELOPMENT.md](DEVELOPMENT.md)。
- `cordis-render` 的 mermaid 面依赖 `vendor/mermaid/` 冻结副本。
- `embed-sdk` 只解析、采集（截图）、把 Gateway 的 `{ kind }` 画出来；斜杠目录迭代 `cordis_tui::slash_catalog()` + `"slash"` extras + `/screenshot*`，不手抄表。标 `terminal` 的命令（`/cd`、`/settings` 含带参）execute 拒绝。
- 本机桌面 CUA 走外部 cua-driver MCP，不自研键鼠；全部与 `bash` 同级权限 / 计划门。cua-driver 自带的 `browser_*` ≠ Dock BUA 的 `browser_*`。
