# cordis-gateway

回环 HTTP/WS Cordis 插件：把本地会话以 JSON-RPC **`dock.1`** 协议投影给宿主页面（`embed-sdk`）。

不是独立服务，是一颗插件：named service **`"gateway"`**（`GatewayHandle`），由 `cordis-app` 的 `install_app` 挂载。

## 核心不变式

| 不变式 | 实现位置 | 为什么 |
|---|---|---|
| **只绑 loopback** | `bind::parse_bind` 拒绝非回环地址；`bind_loopback` 绑定后二次校验 `local_addr()` 仍是 loopback | 网关不暴露到本机以外 |
| **默认挂载但不监听** | `gateway()` / `gateway_idle()` 只 mount，`start_listen` 由 TUI `/pair` 触发；唯一例外是 `gateway_serve`（`dock serve`，见下） | 没有配对就没有监听 |
| **CORS 反射 Origin** | `http.rs` | 有意为之：鉴权靠配对 + 一次性 ticket，不靠 Origin 白名单。**不要**改成白名单校验 |
| **不捕获长生命周期 `Arc`** | `mount` 的 `Inject` 只声明 service key | 调用点 live-lookup `Sessions` 等，不把 `Arc` 关进 HTTP 生命周期 |

> 上述任何一条都不只是约定——`parse_bind` 对非 loopback 直接返回 `Err`，`bind_loopback` 会因 `local_addr()` 非回环返回 `AddrNotAvailable`。改这些前先想清楚后果。

## 挂载方式

```rust
// 生产：挂载但不监听（/pair 时才 start_listen）
root.plugin(gateway(), ())?;
// 再挂 dock.1 的功能插件（vcs、定时任务……），各自往 "gateway.methods" 登记方法
for feature in features() { root.plugin(feature, ())?; }

// gateway_idle(bind_addr)：指定首选地址，仍不监听
// gateway_bind(bind_addr)：立即绑定（仅集成测试用）
```

- `DEFAULT_BIND` = `127.0.0.1:18991`
- `DOCK_GATEWAY_BIND` 覆盖首选地址；端口被占用时按 `PORT_SEARCH`（=32）向上找，`port 0` 交给 OS 分配
- `companion_listener` 在同一端口的另一族回环（`127.0.0.1` ↔ `::1`）上绑一个探针，绑定失败走 `CompanionStatus::Failed` 报给 UI，**从不静默**
- dispose 时 `stop_listen`（`ctx.effect("gateway-http", …)` 注册的 `Disposable`）

## 配对与鉴权（`pairing.rs`）

宿主页面先经 `/pair` 发起配对请求，用户在 TUI 确认后换得一次性 ticket，之后 WS 连接用 ticket 鉴权。

| 项 | 值 |
|---|---|
| `PAIRING_TTL` | 5 分钟（待确认的配对请求过期） |
| `TICKET_TTL` | 1 小时 |
| ticket | **一次性**，交换后即失效 |

`PairingStore` API：`request` / `poll` / `exchange` / `confirm` / `deny` / `revoke` / `issue_for_binding` / `authenticate`，状态变更经 `ctx.emit(GATEWAY_PAIRING, ())` 通知 UI 刷新。

## 无头模式（`serve.rs`，`dock serve`）

给桌面 GUI 当子进程用。`gateway_serve(bind)` 挂载即监听，config 是 `ServeConfig { application, origin }`（启动时就校验），另外 provide `"gateway.serve"`（`ServeControl`）。组合根拿它 `run(stdin, stdout)`：

| 方向 | 行 |
|---|---|
| 启动 → stdout | `{"event":"ready","protocol":"dock.1","version":…,"http":…,"ws":…,"ticket":…,"expiresAtMs":…}` |
| stdin `{"cmd":"ticket"}` → stdout | `{"event":"ticket","ticket":…,"expiresAtMs":…}` |
| 看不懂的行 → stdout | `{"event":"error","message":…}` |
| stdin EOF | `run` 返回，`dock serve` 退出 |

ticket 由 `PairingStore::issue_trusted` 签出：不要求绑定、不经 TUI 确认，但照样钉在 `origin` 上、照样一小时过期。**只经 stdout 交给父进程**，不留绑定——所以 `/v1/connection/tickets` 仍然 403，别的本机进程伪造同一个 Origin 也领不到。serve 模式下 stdout 只能写这些行，诊断走 stderr。

## 远程模式与设备令牌（`devices.rs`，`dock serve --remote`）

部署步骤见 [docs/REMOTE.md](../docs/REMOTE.md)。这里只记网关的行为。

- `gateway_remote(bind)`：挂载即监听，**仍只绑回环**，端口占用直接报错（不顺延、不开 `[::1]`）。
- 路由只剩 `/api/ws`：配对与 ticket 的 HTTP 路由不挂。
- `connection/authenticate { token }`：设备令牌，任何模式都认；远程模式**只**认它。
  - `initialize.connection.application` 报 `device:<名字>`，`origin` 是这条连接的 Origin。
- `connection/authenticate { ticket }`：本机配对 / `dock serve`；远程模式回 `unauthenticated`。
- 同一条连接鉴权失败 5 次：服务端关连接，关闭码 `4429`。
- 连着的设备每 2 秒复查一次；被撤销就关连接，关闭码 `4401`。
- 设备令牌是受信连接：`trusted_only` 的方法（设置页、插件面板、`fs/dirs`）都能调。
  - 理由：令牌本来就能跑命令、读写工作区；远程 GUI 的设置页改的就是远端这台。
- 令牌名单：`$DOCK_HOME/devices.json`（只存 sha256，unix 0600，只有 `dock device add|revoke` 写）。
- 最后使用时间：`$DOCK_HOME/devices.seen.json`（只有网关写，和名单分开，不会覆盖新加的设备）。

## 协议（`protocol.rs` / `rpc.rs`）

- `PROTOCOL_VERSION` = `"dock.1"`，WS 路径 `WS_PATH` = `/api/ws`
- `CAPABILITIES`：`(name, supported)` 数组，`initialize` 时回给宿主
- 协议骨架（连接、线程、轮次、交互、斜杠……）写在 `rpc::dispatch` 里。
- **功能**是插件：挂载时往 `"gateway.methods"`（`GatewayMethods`）登记方法，
  随插件 fiber 注销。用 `register_methods` + `method`；给 GUI 加一页 = 再写一颗。
- 核心方法名（`rpc::CORE_METHODS`）登记不进来。
- 设置页命名空间（`config/` `secret/` `mcp/` `model/` `plugin/` `pairing/` `device/`
  `cua/`、`browser/status`、`skill/list`）只能登记成 `trusted_only`。
- 每个方法带 `MethodPolicy`：
  - `detached`：放到连接锁外跑；
  - `trusted_only`：只认受信连接——`dock serve` 交给父进程的 ticket，或设备令牌；
    配对来的网页 `forbidden`；
  - `opens_thread`：关着的线程先开页。
- `features()` 是本 crate 自带的那几颗：
  - `gateway.settings`（设置页，全部 `trusted_only`）
  - `gateway.presets`（`preset/*`、`tool/catalog`）
  - `gateway.fs`（`fs/*`）、`gateway.canvas`（`canvas/*`）
  - `gateway.vcs`（`vcs/pr/*`）、`gateway.schedule`（`schedule/*`）
  - `gateway.surfaces`（`surface/*`）、`gateway.status`（`status/list`）
  - `gateway.toolViews`（`tool/views`、`tool/view`）、`gateway.pluginSettings`（`plugin/settings/*`）
- 功能插件推连接级通知用 `GatewayMethods::notify(method, params)`：
  `schedule/changed`、`surface/changed` 都是这么推的。
- 方法按域分在 `handlers/`：

| handler | 域 |
|---|---|
| `connection` | `connection/authenticate`；设置页用的 `mcp/list`、`mcp/reload`、`mcp/reconnect`、`model/list`（见下） |
| `settings` | 设置页写配置（只给受信连接，见下） |
| `thread` | 会话列表 / 启动 / 改名 / 归档 / 恢复 / 删除 / 历史 / 订阅；多线程：`thread/open` / `thread/close` / `thread/start {cwd}` / `thread/list {scope:"all"}` |
| `turn` | 发消息、流式回报 |
| `environment` | 环境信息、模型 / reasoning / approval / plan / memory / goal 设置 |
| `interaction` | `ask_user` 问答、权限请求 |
| `permission` | 权限授予状态 |
| `slash` | 斜杠命令远程执行 |
| `image_inputs` | 图片输入 |

### 设置页（MCP 与模型）

| 方法 | 作用 |
|---|---|
| `mcp/list` | 每台 MCP 服务器：`name` / `status`（`connected` / `failed` / `needs_auth` / `disabled`）/ `detail`（连着时是工具数说明，没连上时是原因）/ `toolCount` / `enabledToolCount`。不给启动命令（参数里可能带密钥） |
| `mcp/reload` | 重读 `[mcp_servers.*]`：新增的连上、删掉的断开、改过的和上次没连上的重连。回 `ok` / `serverCount`（老字段）、`summary`、`added` / `removed` / `reconnected` / `disabled` / `failed[]` 和 `servers` |
| `mcp/reconnect { name }` | 只重连这一台，不改配置；停用或认不得的回 `reconnect_failed`，连不上也是，状态里记着原因 |
| `model/list` | config.toml 模型目录：`id` / `label` / `description` / `apiBase` / `contextWindow` / `backends` / `auth`（`key` / `env` / `none`，不给密钥本身）/ `default`，外加当前全局默认 `default`。见下 |

`mcp/list` 与 `model/list` 加了只读字段（老客户端不受影响）：

- `mcp/list` 每台多出 `tools[]`（`name` / `description` / `enabled`）、`editable`、`builtin`。
- `model/list` 每条多出 `keyEnv` / `keyEnvSet`（读哪个环境变量、本进程里有没有）、`editable`。
- `model/list` 顶层多出 `configDefault`（配置文件写的默认；`default` 是根页正在用的）。

### 设置页写配置（只给受信连接，`handlers/settings.rs`）

- 只认 `dock serve` 交给父进程的 ticket（`IssuedTicket.trusted`）。
- 配对来的网页、远程设备调这些一律 `forbidden`：能写 MCP 启动命令 = 能在本机跑程序。
- 读也挡：MCP 的环境变量、请求头里可能有密钥。
- 只写用户级 `~/.dock/config.toml`，经 `toml_edit` 就地改，注释和不认识的键都留着。
- 文件本身解析失败时拒绝写（`config/status` 报行号），免得覆盖掉手写的半截内容。
- 密钥只进不出：`model/get` 只回 `hasApiKey`，`secret/list` 只回名字。

| 方法 | 作用 |
|---|---|
| `config/status` | `dockHome` / `path` / `exists` / `error`（`message` + `line`） |
| `config/get` | 白名单键的值（`edit::SETTING_KEYS`）+ 盖掉它们的环境变量 + `memoryForcedOff` |
| `config/set { key, value }` | 写一个白名单键；`null` = 删掉回默认。改 `memory.*` 当场重载记忆 |
| `config/env { names }` | 这些环境变量在 Dock 进程里设了没有（只回布尔） |
| `model/get { id }` | 用户配置里这条模型的全部字段 |
| `model/save { model, originalId?, makeDefault? }` | 新建 / 保存；回 `model/list` |
| `model/delete { id }` | 删的是默认模型时默认改成目录里下一条 |
| `model/default { id }` | 写 `[models].default`，根页跟着换（新页从根页抄） |
| `model/test { model? \| id }` | 发一次最小请求；表单可以没保存。回 `ok` + `ms` 或 `error` |
| `mcp/get` / `mcp/save` / `mcp/delete` | 编辑 `[mcp_servers.<name>]`，写完当场对账 |
| `mcp/enable` / `mcp/tool/enable` | 服务器 / 单个工具开关（同 `/mcps` 的 Space） |
| `mcp/login { name }` | HTTP 服务器的浏览器 OAuth，等授权完才回 |
| `cua/status` / `cua/action` | cua-driver 状态机；`install` / `grant` 在后台跑，进度靠轮询 `cua/status` |
| `browser/status` | 内置浏览器状态 + 有头偏好 |
| `plugin/list` | 永久插件（用户级 + 每个已知项目，按根分组、按目录认）+ 动态插件（带定义它的会话） |
| `plugin/enable` / `plugin/delete { path }` | 按插件目录启停 / 删除；目录必须在某个插件根下 |
| `plugin/promote { id, scope }` | 动态插件写成永久：`user` 或定义它的会话的项目 |
| `plugin/discard { id }` | 丢掉一个动态插件 |
| `skill/list` | 每层发现的技能，被同名遮住的标 `shadowed` |
| `secret/list` / `secret/set` / `secret/delete` | `$DOCK_HOME/secrets.json`（0600） |
| `pairing/list` / `pairing/resolve` / `pairing/revoke` / `pairing/accept` | `dock serve` 没有 TUI：网页配对在 GUI 里批；`accept` 不落盘 |
| `device/list` / `device/add` / `device/revoke` | 同 `dock device`；令牌只在 `add` 的回包里出现一次 |

会等外部的（`model/test`、`mcp/save|delete|enable|login`、`plugin/enable|promote|discard`）不占连接锁。

开页（`thread/open` / `thread/start {cwd}`）时在后台加载那个项目的 `.dock/plugins`。

- `transcript.rs`：把会话事件流转成 `dock.1` 的增量报文
- `LIVE_THREAD_ID` = `"live"` 指代第 1 页（根）的会话；老客户端只用它

### 多线程（能力 `openThreads`）

一个线程 = 一页（`threads.rs`）。`threadId` 是**落盘会话 id**，`live` 仍是第 1 页的别名；分页身份 `main#N` 只在网关内部用来路由事件。

| 方法 | 作用 |
|---|---|
| `thread/list { scope: "all" }` | 开着的页 + 所有目录的落盘会话（跨目录名册，不走 2 秒备忘），每项带 `cwd` / `open` / `presetId`（开着的页是它当前的预设，落盘的是 `meta.json` 记的，老会话为 `null`），第 1 页另带 `alias: "live"` |
| `thread/search { query, cwd?, limit? }` | 按标题和用户消息搜所有目录的落盘会话，回 `hits[]`：`threadId` / `title` / `cwd` / `snippet`（命中处前后一小段纯文本，只命中标题时为 `null`）/ `updatedAt`。空格分开的词都要命中；含中日韩文的查询按子串匹配，其余走 FTS 前缀匹配。还没落盘的内容搜不到 |
| `workspace/list { scope: "all" }` | 有会话的所有目录（`id` = 路径，`hasOpenThreads`） |
| `thread/start { cwd, title?, presetId? }` | 在 `cwd` 另开一页（不动第 1 页），回它的会话 id；预设在创建时定下：`presetId` 只切这一页并记进会话，不改全局默认；预设不存在就报 `invalid_params`、不开页；不带 `cwd` 是老语义 |
| `thread/open { threadId }` | 把落盘会话开成一页（在它自己的 cwd 下），预设、模型、推理强度切回会话记的那一份（`meta.json`；不改全局默认，模型已不在目录里就不切）；已开着就回那一页。`thread/model/set` / `thread/reasoning/set` 切完立刻写回 `meta.json` |
| `thread/close { threadId }` | 关页，会话留在磁盘上；第 1 页关不掉 |
| `preset/list`（能力 `presets`） | 可选的 Agent 预设（顺序同 TUI `/preset`）：`id` / `label` / `description` / `icon`（客户端图标名，没写为 `null`）/ `builtin`（内置预设的 id；`origin` 为 user/project 时是改过的覆盖层，删掉即恢复内置）/ `origin`（`shipped`/`user`/`project`）/ `available`（坏掉的为 `false` 并带 `error`），外加 `defaultId`。只读：预设在 `thread/start` 时定下，没有中途切换的方法 |
| `preset/create { name, icon?, description?, basedOn? }` | 新建用户层预设（`~/.dock/presets/<id>/`），**不改**默认预设；`basedOn` 照它复制人设、工具名单和子代理。`icon` 只收小写字母、数字、`-`。回 `preset`（同列表的一项） |
| `preset/get { id }` | 整份定义，编辑器用（见下） |
| `preset/update { id, preset }` | 整份写回（见下） |
| `tool/catalog { includeMcp? }` | 预设能选的工具（见下） |
| `preset/delete { id }` | 删用户 / 项目层预设；未改过的内置预设回错，改过的内置预设删掉覆盖层、恢复内置版本。已用它开过的会话不受影响 |

#### 预设编辑器（`preset/get` / `preset/update` / `tool/catalog`）

- `preset/get`：列表那几项，外加 `name` / `order` / `persona` / `replacePrompt`。
- `tools`：`null` = 全部已注册工具，`[]` = 不用工具，数组 = 允许名单。
- `agents[]`：`id` / `name` / `description` / `persona` / `tools` / `replacePrompt` / `listings`。
- `residentTools`（预设与 `agents[]` 各一份）：常驻工具，全名或以 `*` 结尾的前缀。
  - 让本来藏在 `search_tool` 后面的工具（MCP、按需、动态包）直接进模型工具表。
- `onDemandTools`（预设与 `agents[]` 各一份）：按需工具，写法同上，见 `docs/tools/agent-presets.md`。
  - `null` = 不设名单：覆盖内置预设时沿用内置名单，其余预设全部常驻；子代理继承预设的。`[]` = 全部常驻。
- `agents[].builtin`：内置预设自带的角色。
- `path`：落盘的 `agent.yml`；内置没改过为 `null`。
- 损坏的预设照样回：`available: false` + `error`。
- `preset/update`：字段同 `preset/get`，整份写回。
- `residentTools` 不传（或 `null`）= 保持原样：老客户端整份写回不会把它清掉。
- `onDemandTools` 不传 = 保持原样；`null` = 清掉名单（改回不设）；数组 = 新名单。
- 按需工具写法不对同样回 `invalid_params`；另外不收会盖住 `search_tool` / `use_tool` 的条目。
- 内置预设写成 `~/.dock/presets/<id>/` 覆盖层。
- 名册里去掉的角色删掉它的文件；内置自带的角色删不掉。
- 损坏的预设整份重写。
- 校验不过回 `invalid_params`、不写盘：名为空、整份替换却没有提示词、角色 id 不合法或重复、图标名不合法。
- 常驻工具写法不对也回 `invalid_params`：单独的 `*`、中间带 `*`、含空白。
- `tool/catalog`：同 TUI `/preset` 画布左栏，默认不含 MCP。
- `includeMcp: true` 另带 MCP 行：`kind: "mcp"` + `server`（给常驻工具选）。
- 每项 `name` / `summary`（描述第一句）/ `kind`。
- `kind`：`resident` 常驻、`deferred` 按需（按这一页当前预设的 `on_demand_tools` 与登记方式）、`dynamic` 运行中的动态包（不受允许名单限制）。

#### AI 辅助起草（`preset/draft` / `preset/rewrite` / `preset/suggestTools`）

- 用默认模型采样一次；不开会话、不进会话历史、**不写盘**，只回草稿。
- `preset/draft { description, icons? }` → `draft`：字段同 `preset/get`，另带 `toolReasons[]`。
- `draft.tools` 为 `null` = 模型没挑出目录里有的工具（按全部工具）。
- 工具只留 `tool/catalog` 里有的；图标只留 `icons`（客户端图标库）里有的；子代理 id 照规矩校验，最多 3 个。
- `preset/rewrite { persona, mode: "polish" | "expand", description? }` → `persona`。
- `preset/suggestTools { description, persona? }` → `tools[]`（`name` / `reason`）。
- 模型没配好、调用失败、回得不能用：`draft_failed`，`message` 是中文原因。
- 这三个方法不占连接锁：鉴权后另起任务，跑完再回帧，期间推送和其它请求照常。

其它线程级方法（`turn/*`、`thread/environment/*` 与各种 `set`、`thread/subscribe`、`permission/resolve`、`interaction/respond`、`plan/resolve`、`elicit/resolve`、`slash/execute`）接受任意线程的 id。关着的会话**按需开页**（同 `thread/open`，客户端不用先 open；同时来的请求只开一页）：`turn/start` / `enqueue` / `steer`、`thread/subscribe`、`thread/environment/*` 与各种 `set`、`slash/execute`。`turn/cancel` 与 `turn/queue/*` 对关着的会话回空结果（`cancelled: false`、空队列、`removed: false`），不开页；`permission/resolve`、`interaction/respond`、`plan/resolve`、`elicit/resolve` 只对开着的页有意义，仍回 `thread_not_open`；认不得的 id 回 `not_found`。`thread/history` 例外：关着的会话也给，`events` 由落盘事件按真实时间回放（`Transcript::replay`，和重开会话重建投影同一条路），每轮都以 `turn/completed` 收尾（结果照落盘的 `turn-end` 行报，停止、出错也看得出；更早的会话没有这一行，一律补成完成，最后一次采样带错误的补成失败），客户端开着关着只要一条路径。开页走 `"tabs"` 的 `Tabs::open_at`（不切终端里正在看的页）；没挂分页服务时只有第 1 页。

投影每页一份（`handle.rs` 的 `transcripts`，共用一条 broadcast，事件带页身份）；订阅是「页 → 客户端订阅时用的 `threadId`」，推送时用那个 id。会话事件按 `session/page-event` 路由，`turn/completed` 只在那一页记下 `LogEvent::TurnEnd` 时发（一轮一次，流式中途不发；和其它会话事件一样按 `session/page-event` 路由），`status` 是 `completed` / `cancelled` / `failed`，失败带 `error`（错误文本，以前只回给 TUI）；`item/tool_completed` 的 `status` 照 Dock 落下的 `is_error` 报：`completed` / `failed`，停止时的中断结果（以「已中断。」开头，可能带运行时长和已产出的输出）为 `cancelled`，权限门拒绝的为 `denied`；权限 / 提问 / 计划 / elicitation 的事件载荷是 `()`，挨页按队首序号（`front_seq`）对账——同一条不重报，换了一条先报旧的 resolved。提问（`interaction/requested`）每题带 `multiSelect`（多选题可以选多个）；`interaction/respond` 的 `answers[]` 每题 `{questionId, values: [选项标签…], other?}`，`other` 是自己写的回答，既算答案也作为备注交给模型；旧形状 `{questionId, value, kind: option|other}` 仍收。线程级的斜杠命令用 `GatewayHandle::scoped(page)`，下面一串 `cmd_*` 读的 `gateway.ctx()` 就是那一页。

### 浏览器画面（能力 `browserView`，`handlers/browser_view.rs`）

看某个会话正在用的浏览器标签页，并能接手操作。本地、远程、网页端走同一条路。

| 方法 / 推送 | 作用 |
|---|---|
| `browser/view/open { threadId?, url?, viewport?, quality?, maxWidth?, maxHeight? }` | 挂到这个会话的活动标签页，回 `viewId` / `targetId` / `url` / `title` / `tabs`；见下 |
| `browser/view/resize { viewId, width, height, deviceScaleFactor? }` | 面板大小变了：改页面视口（能力 `browserViewport`） |
| `browser/view/input { viewId, event }` | 用户输入，见下 |
| `browser/view/navigate { viewId, url? \| action? }` | 地址栏；`action`：`back` / `forward` / `reload` |
| `browser/view/tab { viewId, action, targetId?, url? }` | 标签栏：`switch` / `close`（带 `targetId`）/ `new`（`url` 可省，默认空白页）；能力 `browserTabs` |
| `browser/view/close { viewId }` | 关视图（不关页）；连接断开时自动全关 |
| 推送 `browser/view/frame` | 一帧：`data`（base64 JPEG）、`mime`、`width` / `height`（视口 CSS 像素）等 |
| 推送 `browser/view/status` | 换了标签页，或地址 / 标题变了 |
| 推送 `browser/view/tabs` | 会话的标签页列表变了：`tabs` = `[{targetId, url, title, active}]`（能力 `browserTabs`） |
| 推送 `browser/view/closed` | 视图结束：`no_tab`（会话的标签页都关了）/ `browser_exited` |

- 标签页来源：浏览器 MCP 写的运行时名册 `$DOCK_HOME/browser/sessions/<pid>.json`。
- 按页的会话身份找（`cordis_spine::mcp_session_key`，和 MCP 调用带的是同一个）。
- 会话还没开标签页：`no_tab`；浏览器没在跑：`browser_unavailable`。
- 名册里的页挂不上（崩了 / 被关了、名册还没改）：先经 MCP 调 `browser_tabs` 让它清掉死页。
  - 会话一页不剩：`no_tab`（推送里同样报 `no_tab`）；带了 `url` 就开新页。
  - MCP 回「这个会话还没有标签页」也算一页不剩：名册里剩的是被杀掉的 MCP 进程留下的旧文件
    （重启 GUI 后常见），那些页跟着旧 Chromium 没了，不报 `browser_unavailable`。
- `open` 带 `url`、会话又还没有标签页：经浏览器 MCP 替它开一页（能力 `browserViewport`）。
  - 走 `cordis_spine::Mcp::call_as`：以那页的身份调 `browser_open`，不进对话流、不过权限门。
  - 开出来的页记在这个会话名下，agent 接着能用；已有标签页时 `url` 不起作用。
  - 浏览器 MCP 没连上 / 开页失败：`browser_unavailable`。
- `viewport { width, height, deviceScaleFactor? }`：页面视口跟着面板走（CSS 像素）。
  - 经浏览器 MCP 的 `browser_resize` 设在 MCP 那条 CDP 连接上：agent 看到的是同一尺寸。
  - 页面的设备像素比不改（agent 的截图不变）；`deviceScaleFactor` 只决定静止帧按几倍截（1–3）。
  - 带了 `viewport` 而没给 `maxWidth` / `maxHeight`：流里的帧不再缩小（上限 4096）。
  - agent 整页截图会清掉覆盖、换标签页新页也没有：帧尺寸对不上就重设（至多 1.5s 一次）。
  - `resize` 和 `input` 排同一条队，连发时最后那次最后生效；改完补一张静止帧。
- 静止帧（帧里 `still: true`）：无头 screencast 只出 CSS 像素，页面不动时一帧不推（刚挂上也不推）。
  - 画面停下 300ms、刚挂上、换了标签页、改了视口：补一张 `captureScreenshot`（`clip.scale` 按 DPR）。
  - 截的时候先停流再接着推：放大栅格化的过渡帧不推；内容没变的重复帧也不推，不会来回补帧。
- agent 换了活动标签页，画面跟着换（每 400ms 看一次名册），推 `status`。
- 标签栏（能力 `browserTabs`）：`tabs` 按名册顺序（和 `browser_tabs` 的序号一致），地址标题现查 Chrome。
  - 每 400ms 对一次，变了推 `browser/view/tabs`；名册里有、Chrome 里已经没了的页不列。
  - `browser/view/tab` 以那页的身份调浏览器 MCP 的 `browser_tabs`（序号按名册现查）：
    用户和 agent 共用当前页，面板切到哪页 agent 就在哪页；画面和标签栏由推送跟上。
  - 关最后一页改调 `browser_close`（关掉这个会话的浏览器页），随后推 `closed { reason: "no_tab" }`；
    页不在了回 `not_found`，MCP 回错是 `tab_failed`。
  - 和 `input` / `resize` 排同一条队：点了新标签页紧接着敲的字落在新页上。
  - 页面自己开的新页（`target=_blank`、`window.open`）要浏览器 MCP 收进会话才进名册：
    看到 opener 是本会话的页、还没进名册的，就（至多每秒一次）在后台调一次 `browser_tabs` 催它收。
- 流控：帧写到 WebSocket 之后才回 CDP `screencastFrameAck`，慢客户端不会攒帧。
- `event` 的形状（坐标是页面视口 CSS 像素，客户端按帧的 `width` / `height` 换算）：
  - `{type:"mouse", action:"move"|"down"|"up"|"click", x, y, button?, clickCount?, modifiers?}`
  - `{type:"wheel", x, y, deltaX, deltaY, modifiers?}`
  - `{type:"key", action:"down"|"up"|"press", key, modifiers?}`（DOM 键名：`Enter`、`a`）
  - `{type:"text", text}`（输入法上屏、粘贴）
  - `modifiers`：`["Alt","Control","Meta","Shift"]` 的子集。
- 地址栏：没写协议补 `https://`，本机和内网地址（`localhost`、`127.0.0.1`、`192.168.x.x`…）补 `http://`；
  只放行 http / https / about / data / file（`javascript:` 拒）。
  - 不像网址的拿去 Google 搜索（`抖音`、带空格的词）；`?` 开头强制搜索。
  - 像网址：认得的协议、本机、IP，或主机名里有点且最后一段是字母（`douyin.com`、`例子.中国`）。
- 这几个方法不占连接锁（挂上去要几秒）；视图是连接级的，不进会话、不落盘。
- 同一会话连发 `open`：最后收到的那个留下。
  - 先完成、已回了 `viewId` 的被顶掉时推 `closed { reason: "replaced" }`；
  - 晚完成、已经不是最新的回错误 `superseded`。
- 同一视图的 `input` 按收到的顺序一个个发给页面，客户端不用等回包再发下一个。

### 桌面画面（能力 `desktopView`，`handlers/desktop_view.rs`）

桌面（CUA）面板的实时画面：agent 正在操作的那个窗口，经 cua-driver 只截图。

| 方法 / 推送 | 作用 |
|---|---|
| `desktop/view/open { threadId?, maxDimension? }` | 开始推画面，回 `viewId` / `threadId` |
| `desktop/view/close { viewId }` | 关视图；连接断开时自动全关 |
| 推送 `desktop/view/frame` | 一帧：`data`（base64）、`mime`、`width` / `height`（像素）、`source`、`window` |
| 推送 `desktop/view/closed` | 视图结束：`driver_unavailable` / `replaced` |
| 推送 `desktop/view/cursor` | agent 做了一次桌面动作：光标位置 + 动作（能力 `desktopCursor`，见下） |
| 推送 `desktop/view/status` | 画面状态变了：`live` / `no_window` / `capture_failed`，带 `message` |

- 看哪个窗口：会话最近一次带 `pid` + `window_id` 的 cua-driver 调用（`source: "agent"`）。
  - 直调和经 `use_tool` 的都认；按日志版本缓存，日志没变不重扫。
  - 还没有就取最前面的普通窗口（`list_windows` 的最大 `z_index`，`source: "front"`）。
  - 跳过 Dock 自己、系统界面，和 cua-driver 画光标的全屏浮层（「Cua Driver」，截它只会失败）。
- 截图：`get_window_state { include_accessibility_tree: false, max_dimension }`（缺省 1280）。
  - 以那页的身份调（`Mcp::call_as`），不进对话流、不过权限门（只读）。
- 节奏：帧写出去了才截下一帧。
  - agent 5 秒内动过桌面：间隔不短于 250ms（约 4 帧/秒）；否则 1 秒一帧。
  - 会话日志里出现新动作就立刻补一帧；截不到时 800ms 后再试。
- 每次 cua-driver 调用最多等 5 秒；`open` 时连不上 / 超时直接回 `driver_unavailable`。
- 中途 cua-driver 断了推 `closed`。
- 截不到不默默重试：推 `status`。
  - `no_window`：没有可看的窗口（agent 没碰过、前台只有 Dock / 系统界面）。
  - `capture_failed`：`message` 带原因，比如「截不到「Grok Bot」：窗口已关闭 · 正在重试」。
  - 连续 2 次截不到才报（agent 自己的慢调用让截图排队超时一次不算）。
  - agent 的窗口连续 3 次截不到：先看最前面的窗口，agent 再动手时换回来。
  - 截到了推 `live`；状态没变不重复推。
- `cursor`：窗口截图里没有 agent 光标（cua-driver 画在屏幕浮层上），客户端自己画。
  - 来源：会话日志里 agent 新做的 cua 动作（视图打开前的历史不算）。
  - 位置：`get_agent_cursor_state`（按动作带的 `session` 问，没带问连接的隐式会话）。
  - 屏幕坐标按窗口的 `bounds` 换成相对位置：`x` / `y` 在 0–1（窗口外会超出）。
  - 问不到时 `x` / `y` 为 `null`（按键这类不动光标），客户端留在原处。
  - `action`：`click` / `double_click` / `right_click` / `drag` / `scroll` / `type` / `key`。
  - `label`：输入的字（最多 12 字）或滚动方向；`keys`：按键（`["cmd","shift","n"]`）。
  - `windowId`：画面上这个窗口；动作换了窗口时等新窗口截到再推。
  - `move_cursor` 不推：它报的位置是原样入参，和别的动作的屏幕坐标对不上。
- `window`：`pid` / `windowId`，解析得到时还有 `app` / `title` / `bounds` / `screenshotScale`。
- 同一会话再 `open`：旧的推 `closed { reason: "replaced" }`。

### 工具结果里的图（能力 `toolImages`）

- `item/tool_completed` 带 `attachments`：每张图 `{ type, index, mimeType, width, height, byteLength }`。
- 只给元数据；`index` 是在这次工具结果里的位置（空图不投影、不占号）。
- 像素：`item/image { threadId, itemId, index? }` → `{ mimeType, width, height, data }`（base64）。
- 开着的会话读内存，关着的读落盘（和 `thread/history` 同一套查找）；找不到回 `not_found`。
- 典型来源：cua-driver 的 `get_window_state` 截图；GUI 的 CUA 面板和工具卡用它。

### 撤回消息（能力 `threadRewind`，`handlers/thread.rs`）

`thread/rewind { threadId, turnId }` → `{ threadId, message, images[] }`：撤回 `turnId` 那条用户消息。

- 它和它之后的对话全部删掉，内存与落盘一起（`Sessions::rewind_to_user`）。
- `message` 是原正文，发送时补在前面的 `[Image #N]` 已去掉；`images[{ mimeType, width, height, dataBase64 }]` 是它带的图。
  客户端放回输入框，改完照常 `turn/start`（图片重新 `imageInputs/put`）。
- `turnId` 要是用户消息开的那一轮（`item/user_message` 的 `turnId`）；不是或已不存在回 `not_found`。
- 正在跑就先停（同 `turn/cancel`），等它停下最多 5 秒，停不下回 `busy`。还有排队的消息回 `queued`。
  等的期间别的连接又排进消息或开了新一轮，同样回 `queued` / `busy`，不撤。
- 已经交给会话、还没随用户消息发出的图片（`queue_user_images`）一并清掉。
- 撤回点早于最近一次压缩时，压缩作废，模型历史回到完整的显示日志。
- **工具已经做过的事（写过的文件、跑过的命令）不会撤销**，日志只是不再记得它们。
- 投影按截断后的会话重建，`seq` 重新编号：客户端整份重拉 `thread/history`。
- 关着的会话按需开页；不占连接锁（要等那一轮停下）。

### 排队、插话、停止并发送（能力 `turnSteer`，`handlers/turn.rs`）

一轮在跑时再发一条有三种意思：

| 方法 | 意思 |
|---|---|
| `turn/enqueue` | 排队：这一轮结束后单独成一轮。回 `status: "queued"` |
| `turn/steer` | 插话：交给正在跑的这一轮，在下一个步骤边界送达（采样、工具都不打断）。回 `status: "steering"`；空闲时等同 `turn/start`（`running`） |
| `turn/start` | 停止并发送：停掉正在跑的这一轮，马上发这一条（空闲时就是发） |
| `turn/queue/steer { queueId? }` | 把一条排队的消息改成插话（省略 = 最早那条）。回 `steered`；没有这条、或页没开着为 `false` |

- 送达：一步开头、采样之前（和子代理信箱同一个位置），落成插话提示 + 用户消息。模型给出最终回复时
  还有插话没送达，就不收尾、送达后再采一步。这一轮已经收尾才到的，转成排在最前的消息。
- 让路：插话在等时，前台 `bash` 超过 2 秒还没跑完就转后台（不杀，回 job_id 和已有输出）。
- 正在压缩时没有可并的采样：插话排到最前，压缩完就发。
- `turn/queue/list` 先列等着送达的插话（`kind: "steer"`、`status: "pending"`），再列排队的
  （`kind: "queue"`）；插话送达前可以 `turn/queue/remove` 撤回。
- 投影：插话从它那条 `item/user_message { steered: true }` 开新的一轮（气泡排在它之前的工作后面），
  被它截开的前一段以 `turn/completed { status: "completed", steered: true }` 收尾——不是真的结束，
  客户端别据此提示「回复完成」。回放（`thread/history`、重开会话）一致。
- 撤回插话（`thread/rewind`、取消时收回输入框）连带它前面的提示一起摘掉。

### 侧边聊天（能力 `sideChat`，`handlers/side_chat.rs`）

从一个会话分叉出来的只读旁问（同 TUI `/btw`）。GUI 开在右侧面板里。

| 方法 / 推送 | 作用 |
|---|---|
| `thread/aside/start { threadId }` | 给主会话开它的侧边聊天（已有就回那一个）。回 `{ thread, parentThreadId, existing }` |
| `thread/aside/handback { threadId }` | 侧边聊天整理一段写进主线的笔记（调一次模型）。回 `{ note }`，只起草不写 |
| `thread/aside/merge { threadId, note }` | 把笔记写进**主会话**。回 `{ delivery: "nextStep" \| "history" }` |
| 推送 `item/side_note { itemId, text }` | 主会话里写进来的笔记，挂在它落下时的那一轮上，进 `thread/history` |

- 侧边聊天就是一个线程：`turn/start`、`thread/subscribe`、`thread/close` 等照常用它的 `thread.id`。
- 带着主会话到此为止的上下文（给模型参考），工具只读；会改东西的命令要用户批准。
- 投影只从分叉那一刻往后：`thread/history` 的 `events` 不重放主线。
  - 旧的 `messages` 字段按会话日志给，仍带着主线快照。
- 不落盘、不进 `thread/list`、不占 9 页的名额；每个主会话最多一个。
- 主会话 `thread/close` 时它一起关。
- `handback`：侧边聊天还在回答回 `busy`；模型没写出正文回 `draft_failed`。
- `merge`：笔记 ≤2000 字。主会话在跑就下一个步骤边界并入（`nextStep`），
  闲着直接落进历史（`history`）。**不开新的一轮**。
  - 落盘是一条 system-reminder（前缀固定），旧客户端看不到它。

### 工作区文件（能力 `workspaceFiles`，`handlers/fs.rs`）

只读看会话 cwd 里的文件（GUI 的文件面板）。路径都相对会话 cwd，用 `/` 分隔。

| 方法 | 作用 |
|---|---|
| `fs/list { threadId?, path?, hidden? }` | 列一层：`entries[{ name, path, kind, size?, modifiedMs? }]`、`truncated` |
| `fs/read { threadId?, path, maxBytes? }` | 读一个文件，`kind` 见下 |
| `fs/find { threadId?, query, limit? }` | 按名字找文件（快速打开）：`paths` |

- `kind`：`dir` / `file` / `symlink`（列目录）；`text` / `image` / `binary`（读文件）。
- `text` 带 `text`、`truncated`（默认上限 512 KB，`maxBytes` 可调小）；截在字符边界上。
- `image`（png / jpg / gif / webp / bmp / ico，≤ 8 MB）带 `mime` 和 base64 `data`。
- svg 按文本回，另带 `mime: image/svg+xml`；其它二进制只给大小。
- 照 `.gitignore`（不管是不是 git 仓库）；默认不列点开头的文件，`hidden: true` 才列。
- 一层最多 5000 项；找文件最多走 10 万个文件，按文件名连续命中打分。
- 出了会话 cwd 就拒（`..`、绝对路径、指到外面的符号链接）：`invalid_params`。
- 只认开着的会话（`thread_not_open`）；不占连接锁，读盘在阻塞线程里。
- 没有写方法：改文件交给 agent（走权限门）。

### 选目录（能力 `directoryPicker`，`handlers/fs.rs`）

远程 GUI 新建会话时选远端的目录（本机 GUI 用系统对话框）。

| 方法 | 作用 |
|---|---|
| `fs/dirs { path?, hidden? }` | 列一层子目录：`path`、`parent`、`home`、`entries[{ name, path }]`、`truncated` |

- `path`：绝对路径，或 `~` / `~/…`（按主目录展开）；空着就是主目录。相对路径 `invalid_params`。
- 回包里的路径都是规范化后的绝对路径；`parent` 到根是 `null`。
- 只列目录（指向目录的符号链接也算），不照 `.gitignore`；默认不列点开头的，`hidden: true` 才列。
- 按名字排（不分大小写），最多 1000 项（先排序再截，`truncated: true` 时是前 1000 个）。
- 目录存在但读不了（没权限）回 `read_failed`；路径不对回 `invalid_params`。
- 不限在会话 cwd 里，所以 `trusted_only`：配对来的网页 `forbidden`。

### 画布（能力 `canvas`，`handlers/canvas.rs`）

模型用 `canvas_*` 工具写的 HTML（见 `docs/tools/canvas.md`），GUI 的画布面板读它。

| 方法 | 作用 |
|---|---|
| `canvas/list { threadId? }` | 本会话的画布，最近改过的在前：`canvases[Meta]` |
| `canvas/get { threadId?, canvasId, version? }` | 一版 HTML + 数据：`{ canvas, version, html, data, path }` |
| `canvas/setData { threadId?, canvasId, data }` | 用户在画布里改的数据，不出新版 |
| `canvas/rollback { threadId?, canvasId, version }` | 把那一版拷成新的最新版 |

- `Meta`：`{ id, title, createdMs, updatedMs, dataUpdatedMs, latest, versions[{ n, note, createdMs, bytes }] }`。
- 默认最新版；`data` 没有就是 `null`；`path` 是画布目录（本机才有意义）。
- 关着的会话从名册找目录，历史会话的画布也读得到；还没落过盘的新会话回空列表。
- `canvasId` 只认 `canvas-<n>`（`invalid`）；没有这个画布 / 这一版回 `not_found`。
- 不推通知：客户端看 `canvas_*` 的工具项刷新。不占连接锁，读盘在阻塞线程里。

### 压缩进展（能力 `compactionProgress`）

压缩那几秒没有流式输出，客户端靠这几条知道它在干什么。

| 推送 / 字段 | 作用 |
|---|---|
| 推送 `context/compacted` | 压缩进展：开始、换阶段、摘要又写了一段（最多 250ms 一条）、结束 |
| 推送 `item/compaction { itemId, status }` | 压缩完成的标记，落在当时那一轮里，进 `thread/history` |
| `thread/environment/get` 的 `context.lastCompaction` | 正在进行或最近一次压缩，外形同 `context/compacted` |

- `context/compacted` 的字段：
  - `status`：`running` / `completed` / `failed` / `cancelled`；
  - `trigger`：`auto` / `manual`；
  - `phase`：`memory`（整理记忆）/ `summary`（生成摘要）/ `apply`（替换历史）；
  - `attempt` / `maxAttempts`、`retryReason?`（上一次为什么没成）；
  - `outputTokens`（这次尝试摘要已输出，估算）、`beforeTokens`、`afterTokens?`（压成了才有）；
  - `elapsedMs`、`error?`（失败原因）。
- 只推不记：`seq` 为 0、不进 `thread/history`；中途接入的客户端从 `lastCompaction` 补。
- 顺序：`item/compaction` 先到，带 `afterTokens` 的完成推送随后。
- 「已压缩上下文。」不再投影成 `item/message_delta`；自动压缩失败那条「自动压缩失败：…」也不投影
  （失败由 `context/compacted` 的 `failed` 说）。
  - 空闲时手动压缩没有一轮包着：开一轮、推标记、马上 `turn/completed`。
- 重开会话 / 回放历史只有 `item/compaction`，没有前后占用。

### 子代理（能力 `subagents`）

模型用 `task` 派的子代理，挂在启动它的那一页：只有那一页的订阅者收到推送，
`subagent/*` 也只认那一页的 `threadId`。子代理只活在内存里，Dock 重启后就没了。

| 方法 / 推送 | 作用 |
|---|---|
| `subagent/list { threadId? }` | 这个线程启动的子代理 `{ agents: [Agent] }`，先启动的在前 |
| `subagent/history { threadId?, agentId }` | `{ agent, events }`：子代理自己的对话，和 `thread/history` 的 `events` 同形 |
| `subagent/send { threadId?, agentId, message }` | 用户对子代理说一句：在跑就在下一步读到，空闲就开下一轮 |
| `subagent/interrupt { threadId?, agentId }` | 停下这一轮，子代理留着能接着聊；没在跑回 `interrupted: false` |
| `subagent/stop { threadId?, agentId }` | 收掉子代理；已经收掉的回 `stopped: false` |
| 推送 `subagent/updated { agent }` | 子代理出现了或状态变了 |
| 推送 `subagent/event { agentId, event }` | 子代理对话里的一条事件，`event` 同 `subagent/history` 的条目 |

- `Agent` 的字段：
  - `agentId`、`toolCallId`（派它的那次 `task` 调用；workflow 派的是 `null`）；
  - `subagentType`、`role`（预设里的显示名，如「探索」）、`description`；
  - `status`：`running` / `idle`（这一轮做完，能接着聊）/ `completed`（收掉了）/
    `failed` / `cancelled`（最近一轮怎么收的尾）；
  - `startedAt`（毫秒）、`durationMs`（在跑时算到现在，停下就定住）、`toolCalls`；
  - `output`（最近一轮的回复，最多 4000 字）、`error?`（失败才有）；
  - `activity?`（在跑才有）：`{ kind: "tool", toolName, arguments }` / `{ kind: "replying" }` /
    `{ kind: "thinking" }`。
- 子代理的事件：
  - 第一条 `item/user_message` 是父级派的任务（原文，含 `[type] 描述` 开头和回报说明）；
  - 父级在它跑的时候发来的话是 `item/user_message { origin: "parent" }`；
  - 停下时收一次 `turn/completed`（failed 带 `error`）。
- 两种推送都只推不记：`seq` 为 0，不进父线程的 `thread/history`。
- 收掉的子代理（`completed`）网关不再留它的实时投影，`subagent/history` 改按它的会话回放：
  - 序号从头重排，和收掉前的推送对不上；客户端对停下的子代理以历史为准。
- 页关掉时它派出的子代理的投影一起丢掉。
  - `subagent/event` 里的 `event.seq` 是子代理自己的序号，接着 `subagent/history` 往下数。
- 找不到、或不是这个线程启动的子代理回 `not_found`；收掉的子代理 `send` 回 `subagent_closed`。
- 子代理和派它的页共用这一页的权限队列与「始终允许」：
  - 子代理发的 `permission/requested` 带 `agentId`（它的 id），主会话发的是 `null`；
  - `permission/resolve { always: true }` 记在这一页上，之后这页和它的子代理调同一颗工具都不再问。

### 上下文明细（能力 `contextBreakdown`，`handlers/context.rs`）

`thread/context/get { threadId? }`：下一次请求的上下文窗口按类别拆开，和 TUI `/context` 同一份数。

- 顶层：`usedTokens`、`maxContextTokens`、`usagePercent`、`compactionTriggerPercent` / `compactionTriggerTokens`、
  `turnCount`、`toolCallCount`、`compactionCount`、`model`。
- `slices`：互不重叠，加起来是整个窗口（已用 + 空闲）。
  - `usedTokens` 有上游真账时，分项（系统提示 / 工具定义 / 消息 / 子项 / 明细行）是本地估算缩到它里面的：
    估算之和超过已用就按同一比例缩，不足的差额在 `overhead`。分项之间的比例仍是估算。
  - `id`：`system` / `tools` / `messages` / `overhead`（推理、图片、上游真账与估算之差；为 0 时不给）/ `free`；
  - `group`：`prefix`（每轮原样重发）/ `session`（随对话增长）/ `free`；
  - `label`、`tokens`、`note?`；
  - `includes[{ label, tokens, note }]`：已算在这一片里的子项（技能 / 工作流在系统提示里，规约 / 记忆在消息里）；
  - `detail[{ heading, rows[{ label, tokens?, note? }] }]`：明细，每组最多 40 行，其余并成「其余 N 项」。
- `onDemand[{ id, label, note, detail }]`：MCP / 本地按需工具，不进窗口（`id` 为 `mcp` / `deferred`）。
- 关着的会话先开页再算。

目标（`thread/environment/get` 的 `goal`）：

- `status` 多了 `completed`：模型报了 `update_goal(completed)`，标题和最后一条进展留着，`start` / `clear` 后复位。
- `elapsedMs`：目标实际在跑的时长，暂停的时段不算；`active` 时客户端自己往上走表。

### 定时任务（能力 `schedules`，`handlers/schedule.rs`）

任务落盘在 `$DOCK_HOME/schedules.json`，属于某个会话，跨重启继续跑。到点由 `cordis-app` 的
`cron_driver` 送进那个会话；会话关着就 `Tabs::open_session` 后台开页，网关监听
`tabs/page-opened` 把这页纳入投影（回放历史、装项目插件），同 `thread/open`。

- `schedule/list {}` → `{ tasks[] }`：全部会话的任务。每项：
  - `id` / `prompt` / `everySecs` / `intervalLabel`（「every 10 minutes」）；
  - `threadId`（会话 id，老任务为 `null`）/ `threadTitle` / `cwd`；
  - `createdAtMs` / `nextAtMs` / `expiresAtMs`（7 天）/ `lastFiredAtMs`；
  - `lastError`：上次没送到的原因（开不了页之类），下次送到就清掉；
  - `heldHere`：这个进程持有它（多个 Dock 共用 `$DOCK_HOME` 时只有持有者触发）。
- `schedule/create { threadId, interval | everySecs, prompt, fireImmediately? }` → `{ task }`。
  - `interval` 同 `/loop`（`5m` / `2h` / `1d`）；`everySecs` 最小 60。
  - `threadId` 开着的页（含 `live`）取它正在写的会话；关着的按会话列表找。还没落盘的回 `invalid_params`。
- `schedule/update { id, interval? | everySecs?, prompt? }` → `{ task }`：改间隔保持相位，不续期。
- `schedule/delete { id }` → `{ deleted }`；没有这个 id 回 `not_found`，`schedules.json` 读写失败（如文件写坏了）回 `store_failed`，不混成 `not_found`。
- 推送 `schedule/changed {}`：**连接级**，初始化过就收到，不用订阅线程。任何一处改了（含模型的
  `scheduler_*`、别的 Dock 进程、到点触发）最多 1 秒后到；收到后重拉 `schedule/list`。

### 插件面板（能力 `surfaces`，`handlers/surface.rs`）

插件（Rhai `host.register_slot`）登记的面板，不分端：正文是文本，动作是按钮。

- `surface/list {}` → `{ surfaces: [{ id, title, hud, web }] }`。
- `surface/get { id }` → `{ surface: { id, title, body, view, actions: [{ id, label }], web } }`：
  - `view`：插件给了视图树就是规范化的 dock.view.1（`docs/PLUGIN-VIEWS.md`），否则 `null`；
  - 有视图时 `body` 是视图的纯文本降级，不再跑插件的 `render()`；
  画正文要跑插件脚本、正文可能带本机信息，只给受信连接。
- `surface/action { id, action }` → `{ closed, surface }`：
  - 只认面板声明过的动作或视图里的按钮 / 列表行动作，否则 `invalid_params`；
  - 动作会跑插件脚本，只给受信连接（`forbidden`，同设置页）；
  - `closed` 为真表示插件要关面板；插件在动作里注销了面板时 `surface` 为 `null`。
- `surface/web { id }` → `{ html }`（能力 `surfaceWeb`）：`web: true` 的面板自带的 HTML。
  - GUI 放进沙箱 iframe（不同源、不许联网），只经窄桥读会话数据（`docs/PLUGIN-VIEWS.md` web 面板）；
  - 插件写的代码，只给受信连接；没有 web 界面的面板回 `unavailable`，没有这个面板回 `not_found`。
- 推送 `surface/changed { id }`：连接级。增删、被操作、插件 `host.slot_changed` 时到。

### 插件状态项（能力 `statusItems`，`handlers/status.rs`）

- `status/list {}` → `{ items: [{ id, text, tone, tooltip, surface }] }`：
  - 含 `hud: true` 的插槽（文字是正文第一行，`surface` 指向它）；
  - 文字是插件内容，只给受信连接；没挂状态项服务时回空表。
- 推送 `status/changed { id }`：连接级。状态项增删改、或插槽变了时到。

### 插件工具卡视图（能力 `toolViews`，`handlers/tool_view.rs`）

- `tool/views {}` → `{ tools: [name] }`：哪些工具有卡片视图。
- `tool/view { threadId?, itemId }` → `{ view }`：
  - 按需画：GUI 展开工具卡时才调，`itemId` 是工具调用 id；
  - 插件这次不给 / 没登记时 `view` 是 `null`；找不到那次调用回 `not_found`；
  - 会跑插件脚本，只给受信连接。
- 推送 `tool/views/changed { name }`：连接级。有工具的视图登记或卸下时到。

### 插件设置卡（能力 `pluginSettings`，`handlers/plugin_settings.rs`）

都在设置页命名空间，只认受信连接。

- `plugin/settings/list {}` → `{ plugins: [{ pluginId, title }] }`。
- `plugin/settings/get { pluginId }` → `{ pluginId, schema, values, secrets }`：
  - `values`：普通字段的当前值（没写过是 default）；
  - `secrets`：密钥字段设没设，永远不回原值。
- `plugin/settings/set { pluginId, values }`：
  - 成功 `{ ok: true, settings }`；
  - 字段不合法 `{ ok: false, errors: { key: 原因 } }`，整组不写；
  - 密钥字段：空串不改，`null` 删掉。
- 推送 `plugin/settings/changed { pluginId }`：连接级。

### Pull Request（能力 `pullRequests`，`handlers/vcs.rs`）

项目的 GitHub PR，只读，经本机 `gh` CLI。两个方法都在锁外另起任务跑（一次几秒，走网络），单次 gh 调用 20 秒超时。

慢的是建连接（每次 gh 新开一条 TLS，走代理时一两秒），所以：
- 列表只发一次 `gh api graphql`（我是谁 + 仓库名 + PR；`{owner}/{repo}` 由 gh 本地解析）。
- 列表按项目缓存 60 秒；判断「变没变」本身就要一次往返（GraphQL 没有 304），只能靠有效期。
- 详情按 PR 缓存：有比它新的列表时，列表里这个 PR 的 `updatedAt` 和检查汇总都没变就复用，变了就重拉；
  没有更新的列表时按 60 秒有效期。
- 同一项目 / 同一 PR 同时来的请求只跑一次 gh。gh 用不了的结果不缓存。
- 两个方法都收 `force: true` 绕过缓存；成功的结果带 `fetchedAtMs`（从 GitHub 拿的时刻）和 `cached`。

- 项目用 `threadId`（那个线程的 cwd）或 `cwd` 指定；`cwd` 必须是 Dock 认识的项目（开着的页或会话列表里的某个 cwd），否则 `invalid_params`。
- `vcs/pr/list { threadId | cwd }` → `{ available, repo, viewer, prs[] }`，开着的 PR 最多 50 条：
  - `number` / `title` / `url` / `author` / `isDraft` / `headRefName` / `baseRefName` / `updatedAt`；
  - `reviewDecision` / `mergeable`（空串回 `null`）；
  - `checks: { state, total, failed, pending }`，`state` 为 `success` / `failure` / `pending` / `none`；
  - `mine`（作者是我）/ `reviewRequested`（请我 review）。
- `vcs/pr/get { threadId | cwd, number }` → `{ available, pr }`：上面那些，加 `body` / `state` / `mergeStateStatus` /
  `additions` / `deletions` / `changedFiles` / `comments`（条数）/ `reviews[]`（每人最近一次）/ `checks[]`（`name` / `state` / `url`）。
- **gh 用不了不是错误**：回 `{ available: false, reason, hint }`，客户端画空状态。
  - `gh_missing`：没找到 gh（提示 `brew install gh` + `gh auth login`）；
  - `gh_unauthenticated`：gh 没登录；
  - `not_git`：项目不是 git 仓库；`not_github`：没有指向 GitHub 的远端；
  - `gh_failed`：gh 自己出错或超时，`hint` 带它最后一行原话。
- 找 gh：`DOCK_GH`（给了就只认它）→ `PATH` → `/opt/homebrew/bin` / `/usr/local/bin` / `~/.local/bin`。
  从访达启动的桌面端拿到的 `PATH` 通常不含 Homebrew，所以后几处要自己看。

## 依赖注入

`mount` 声明依赖：`SESSIONS`、`SESSION_PORT`、`PERMISSIONS`、`ASK`、`PLAN_MODE`、`MCP`、`TURN`、`SETTINGS`。这些是 named service，在 `apply` 时 live-lookup，**不要**在闭包里持有 `Arc`。

## 相关文档

- 协议消费者侧：`embed-sdk/README.md`
- 产品面约束：根 `AGENTS.md` 的 Boundaries / Safety
- 架构：`docs/ARCHITECTURE.md`
