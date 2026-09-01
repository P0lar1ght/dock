# AGENTS.md

Dock 是 Grok 外形的 TUI，跑在 Cordis 插件树上。和 DeepSeek Harness 一样：**一切皆插件**。没有可私自打补丁的内核；新行为是再挂一个插件，或接到已有 named service / waterfall 上。

先读 [README.md](README.md) 知道怎么跑。模型工具 / 缺口：[TOOLS.md](TOOLS.md)。斜杠、快捷键、overlay：[CLI.md](CLI.md)。浏览器注入：[embed-sdk/README.md](embed-sdk/README.md)。加工具前先改 TOOLS 清单，不要只在 loop 里加名字；改斜杠 / overlay 时同步 CLI。

## 仓库边界

工作只在本仓库（`AILab/dock`）。不要改 `grok-build/`、`deepseek-harness/`、上游 `cordis/`（JS）。不要 path-dep `grok-build/`：需要 Grok 源码时复制进 `vendor/xai/` 或对应 crate 再改。

## 仓库布局

```
cordis-rust/     插件内核 crate `cordis`：Context、inject、named services
cordis-spine/    Agent 循环、工具、MCP、会话、预设；install_app 挂整棵产品树
cordis-tui/      全屏终端 UI 插件（theme / scrollback / prompt / …）
cordis-gateway/  回环 HTTP/WS 插件：Origin 配对、dock.1 投影、slash/list|execute
cordis-app/      二进制入口：install_app + agent-loop + gateway + tui
cordis-render/   Markdown / Mermaid 渲染
embed-sdk/       宿主页 SDK（dock-embed.js）；协议 dock.1，不要为每个斜杠单独适配
vendor/          冻结副本：mermaid 布局栈、xai Grok 拷贝（见 vendor/README.md）
skills/          Agent skills（动态 Cordis 插件工作流）
assets/          品牌图
config.toml.example   用户/项目模型目录样例
```

Crate README 管该包的 API 与挂载方式。根目录这三份清单是产品面的权威：TOOLS（模型工具）、CLI（人操作的面）、本文件（架构约定）。

## 命令

```bash
cargo run -p cordis-app
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
cargo test -p cordis-spine --test round -- install_app_registers
```

浏览器 SDK：

```bash
cd embed-sdk && npm install && npm run build
```

`cordis-gateway` 需要 **rustc 1.88+**（1.85 编不过）。本机不够时在 CI / 1.88 环境跑 `cargo test -p cordis-gateway`。`Context::new()` 需要 tokio runtime。

不要默认跑全量套件来证明一次小改；改行为就跑对应 crate 的测试。核对 `install_app` 工具表时用 `install_app_registers`。

## 一切皆插件

| 层 | 例子 | ctx key |
|---|---|---|
| Spine 五件套 | `sessions` `llm` `tools` `systemPrompt` `agents` | 同名 |
| 循环 | `agent-loop` 提供 `LoopHandle` | `agentLoop` |
| 其它 spine | `settings` `turn` `permissions` `cron` `jobs` `todos` `planMode` `ask` `mcp` `goal` `lsp` `subagents` `memory` `workflows` `slash` `agentPresets` `dynamicCordisRunner` `compact` | 同名。`agentPresets` 是 YAML 目录（内置 `code` / `minimal` / `cordis` / `warden` < `~/.dock/presets/<id>/` 或显示名目录如 `创造/` < 项目 `.dock/presets/<id>/`；旧 `<id>.yml` 仍可读）。新建模式默认落到项目层，细节见 [CLI.md](CLI.md) |
| 工具插件 | `tool-web` `tool-todo` `plan-mode` `tool-ask-user` `tool-jobs` `tool-scheduler` `tool-task` `tool-subagent` `tool-memory` `tool-monitor` `tool-goal` `tool-lsp` `tool-workflow` `mcp-client` `tool-cordis` | 向 `"tools"` `register`。清单与缺口：[TOOLS.md](TOOLS.md) |
| TUI | `theme` `tui.scrollback` `tui.prompt` `tui.statusBar` `tui.welcome` `tui.shortcuts` `tui.pairing` | 同名 |
| 回环网关 | `gateway` | `"gateway"`（`GatewayRef`）。事件 `gateway/pairing`。只绑 loopback（默认 `127.0.0.1:18991`，同时尝试 `[::1]` 同端口；`DOCK_GATEWAY_BIND` 可覆盖）。配对、CORS、斜杠 list/execute 的终端限制见 [CLI.md](CLI.md) `/pair` |
| 事件循环 | `tui` inject `session` + `session.port` | — |

- **换插件，不改 loop。** 新 UI 面做成 `tui.*` 插件；新采样做成 `llm` 插件。工具能力插件 `inject: ["tools"]` 后 `ctx.tools.register()`（DSH 一个 `"tools"` 表，不是 `tools.mcp` ExtraTools）。不要把功能焊进 `event_loop` 或 `agent-loop`。
- **循环本身也是插件。** 不要调用 `xai_grok_pager::app::run`，不要 spawn Grok `MvpAgent`。ACP 只是 pager 的权限表面（`PermissionOptionKind`），不是完整 ACP agent。
- **扩展走 waterfall**（`agent/pre-step`、`llm/stream`、`tools/execute`、`system-prompt/assemble`）。拦截时接 `on_waterfall`；默认实现放在 `waterfall(..., || default)` 的闭包里。Waterfall 监听必须把控制权交给下一环，不要悄悄吞掉链。
- **MCP 是 fail-open 插件。** `mcp-client` 连不上或没配置时仍 `Active`，往 `"mcp"` 写空/失败状态；工具名 `mcp_{server}__{tool}` 注册进 `"tools"`（开启的 MCP 工具穿过 Agent 预设允许名单），不能盖掉 `bash`。HTTP MCP 的 OAuth token 在 `~/.dock/mcp_credentials.json`（`DOCK_HOME`），不要写进 `config.toml`。不是 grok.com 账号登录。
- **浏览器 companion 不是第二套 harness。** `embed-sdk/` 只解析、采集（截图）、把 Gateway 的 `{ kind }` 画出来。斜杠目录迭代 `cordis_tui::slash_catalog()`（不是手抄表）+ `"slash"` extras + `/screenshot*`。list 标 `terminal` 的命令（`/cd` `/settings` 含带参）execute 拒绝。斜杠 `notice` / `applied` 走命令输出卡片，不要当 composer 错误。

## Live-lookup，不捕获

Named service 用 `ctx.get` / `ctx.require` **在调用点**取。不要把 `Arc<T>` 关进长生命周期闭包（TUI frame、HTTP 重试、cron tick、sampler `on_delta` 除外：那些闭包里也要再 `get`，不要 clone 服务本身带走）。

```rust
// 错：启动时抓住 settings，之后一直用旧 Arc
let settings = ctx.require::<AppSettings>(SETTINGS)?;
move || settings.model()

// 对：用的时候再取
ctx.get::<AppSettings>(SETTINGS).map(|s| s.model())
```

模式、模型、权限开关住在 `"settings"` 插件上。TUI 只把按键映射成 Action，然后 live-lookup `settings`（例如 `Shift+Tab` → 切 `PermissionMode`）。不要在 prompt / event loop 里另存一份模式状态。

快捷键条是 `"tui.shortcuts"`：每帧 live-lookup，用当前 overlay / prompt / `session.port` 拼 hint。不要把 hint 列表写死在 `event_loop`。

## 复制 Grok，适配 Cordis

Chrome（快捷键条 `Key:label`、思考折叠、工具卡片输入/输出、prompt 框）对齐 Grok pager，但数据面走 spine 的 `LogEvent` / `LlmOutput`，渲染走 `tui.scrollback`。

- 推理是 `StreamDelta::Reasoning` / `LlmOutput.reasoning`，不要混进助手 markdown。
- 工具卡片折叠显示 name + 参数摘要；展开先「输入」再「输出」。参数存在 `LogEvent::ToolExecute.arguments`。
- 用户可见文案用中文；Grok 底栏那种短 hint（`send` / `mode` / `shortcuts`）保持 Grok 用词和 `Enter:send` 无空格格式。

## 测试与配置

- `install_fakes` 保持 **echo**。`cordis-spine/tests/round.rs` 期望 `TurnOutcome::Text("echoed: hello")`。
- 打开 MCP 的测试必须 fail-open；harness 默认 `mcp: false`。
- 不要提交 `.dock/config.toml`、API key、`.env`。`DOCK_HOME` 覆盖用户配置目录。
- `[::1]` 绑失败会打 stderr，并出现在 `/pair` overlay 与 `initialize.connection.companion`，不能静默。CORS 反射 Origin 是有意的：鉴权靠配对 + 回环，不是 Origin 白名单。Approved 的 poll **不**回 ticket 明文；`POST /v1/pairing/exchanges` 校验 TTL、一次性消费。

## 改代码时

1. 先问：这是新插件、换现有插件，还是该接到已有 waterfall / named service？
2. 不要为了方便在 `tui` / `agent-loop` 里加私有状态。
3. 改工具面时同步 [TOOLS.md](TOOLS.md)；改斜杠 / overlay 时同步 [CLI.md](CLI.md)。
4. 用户没要求就不要 commit。

## 改这份说明

规则尽量自洽，细节链到 TOOLS / CLI / crate README，不要把目录表再抄一遍。能缩短且不失真就缩短。
