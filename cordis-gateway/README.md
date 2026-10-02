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
- 令牌名单：`$DOCK_HOME/devices.json`（只存 sha256，unix 0600，只有 `dock device add|revoke` 写）。
- 最后使用时间：`$DOCK_HOME/devices.seen.json`（只有网关写，和名单分开，不会覆盖新加的设备）。

## 协议（`protocol.rs` / `rpc.rs`）

- `PROTOCOL_VERSION` = `"dock.1"`，WS 路径 `WS_PATH` = `/api/ws`
- `CAPABILITIES`：`(name, supported)` 数组，`initialize` 时回给宿主
- 方法按域分在 `handlers/`：

| handler | 域 |
|---|---|
| `connection` | `connection/authenticate`；设置页用的 `mcp/list`、`mcp/reload`、`mcp/reconnect`、`model/list`（见下） |
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
| `model/list` | config.toml 模型目录（只读）：`id` / `label` / `description` / `apiBase` / `contextWindow` / `backends` / `auth`（`key` / `env` / `none`，不给密钥本身）/ `default`，外加当前全局默认 `default` |

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
- `agents[].builtin`：内置预设自带的角色。
- `path`：落盘的 `agent.yml`；内置没改过为 `null`。
- 损坏的预设照样回：`available: false` + `error`。
- `preset/update`：字段同 `preset/get`，整份写回。
- `residentTools` 不传（或 `null`）= 保持原样：老客户端整份写回不会把它清掉。
- 内置预设写成 `~/.dock/presets/<id>/` 覆盖层。
- 名册里去掉的角色删掉它的文件；内置自带的角色删不掉。
- 损坏的预设整份重写。
- 校验不过回 `invalid_params`、不写盘：名为空、整份替换却没有提示词、角色 id 不合法或重复、图标名不合法。
- 常驻工具写法不对也回 `invalid_params`：单独的 `*`、中间带 `*`、含空白。
- `tool/catalog`：同 TUI `/preset` 画布左栏，默认不含 MCP。
- `includeMcp: true` 另带 MCP 行：`kind: "mcp"` + `server`（给常驻工具选）。
- 每项 `name` / `summary`（描述第一句）/ `kind`。
- `kind`：`resident` 常驻、`deferred` 按需、`dynamic` 运行中的动态包（不受允许名单限制）。

#### AI 辅助起草（`preset/draft` / `preset/rewrite` / `preset/suggestTools`）

- 用默认模型采样一次；不开会话、不进会话历史、**不写盘**，只回草稿。
- `preset/draft { description, icons? }` → `draft`：字段同 `preset/get`，另带 `toolReasons[]`。
- `draft.tools` 为 `null` = 模型没挑出目录里有的工具（按全部工具）。
- 工具只留 `tool/catalog` 里有的；图标只留 `icons`（客户端图标库）里有的；子代理 id 照规矩校验，最多 3 个。
- `preset/rewrite { persona, mode: "polish" | "expand", description? }` → `persona`。
- `preset/suggestTools { description, persona? }` → `tools[]`（`name` / `reason`）。
- 模型没配好、调用失败、回得不能用：`draft_failed`，`message` 是中文原因。
- 这三个方法不占连接锁：鉴权后另起任务，跑完再回帧，期间推送和其它请求照常。

其它线程级方法（`turn/*`、`thread/environment/*` 与各种 `set`、`thread/subscribe`、`permission/resolve`、`interaction/respond`、`plan/resolve`、`elicit/resolve`、`slash/execute`）接受任意线程的 id。关着的会话**按需开页**（同 `thread/open`，客户端不用先 open；同时来的请求只开一页）：`turn/start` / `enqueue` / `steer`、`thread/subscribe`、`thread/environment/*` 与各种 `set`、`slash/execute`。`turn/cancel` 与 `turn/queue/*` 对关着的会话回空结果（`cancelled: false`、空队列、`removed: false`），不开页；`permission/resolve`、`interaction/respond`、`plan/resolve`、`elicit/resolve` 只对开着的页有意义，仍回 `thread_not_open`；认不得的 id 回 `not_found`。`thread/history` 例外：关着的会话也给，`events` 由落盘事件按真实时间回放（`Transcript::replay`，和重开会话重建投影同一条路），每轮都以 `turn/completed` 收尾（结果照落盘的 `turn-end` 行报，停止、出错也看得出；更早的会话没有这一行，一律补成完成，最后一次采样带错误的补成失败），客户端开着关着只要一条路径。开页走 `"tui.tabs"` 的 `Tabs::open_at`（不切终端里正在看的页）；没挂分页服务时只有第 1 页。

投影每页一份（`handle.rs` 的 `transcripts`，共用一条 broadcast，事件带页身份）；订阅是「页 → 客户端订阅时用的 `threadId`」，推送时用那个 id。会话事件按 `session/page-event` 路由，`turn/completed` 只在那一页记下 `LogEvent::TurnEnd` 时发（一轮一次，流式中途不发；和其它会话事件一样按 `session/page-event` 路由），`status` 是 `completed` / `cancelled` / `failed`，失败带 `error`（错误文本，以前只回给 TUI）；`item/tool_completed` 的 `status` 照 Dock 落下的 `is_error` 报：`completed` / `failed`，停止时补的「已中断。」为 `cancelled`，权限门拒绝的为 `denied`；权限 / 提问 / 计划 / elicitation 的事件载荷是 `()`，挨页按队首序号（`front_seq`）对账——同一条不重报，换了一条先报旧的 resolved。提问（`interaction/requested`）每题带 `multiSelect`（多选题可以选多个）；`interaction/respond` 的 `answers[]` 每题 `{questionId, values: [选项标签…], other?}`，`other` 是自己写的回答，既算答案也作为备注交给模型；旧形状 `{questionId, value, kind: option|other}` 仍收。线程级的斜杠命令用 `GatewayHandle::scoped(page)`，下面一串 `cmd_*` 读的 `gateway.ctx()` 就是那一页。

### 浏览器画面（能力 `browserView`，`handlers/browser_view.rs`）

看某个会话正在用的浏览器标签页，并能接手操作。本地、远程、网页端走同一条路。

| 方法 / 推送 | 作用 |
|---|---|
| `browser/view/open { threadId?, url?, viewport?, quality?, maxWidth?, maxHeight? }` | 挂到这个会话的活动标签页，回 `viewId` / `targetId` / `url` / `title`；见下 |
| `browser/view/resize { viewId, width, height, deviceScaleFactor? }` | 面板大小变了：改页面视口（能力 `browserViewport`） |
| `browser/view/input { viewId, event }` | 用户输入，见下 |
| `browser/view/navigate { viewId, url? \| action? }` | 地址栏；`action`：`back` / `forward` / `reload` |
| `browser/view/close { viewId }` | 关视图（不关页）；连接断开时自动全关 |
| 推送 `browser/view/frame` | 一帧：`data`（base64 JPEG）、`mime`、`width` / `height`（视口 CSS 像素）等 |
| 推送 `browser/view/status` | 换了标签页，或地址 / 标题变了 |
| 推送 `browser/view/closed` | 视图结束：`no_tab`（会话的标签页都关了）/ `browser_exited` |

- 标签页来源：浏览器 MCP 写的运行时名册 `$DOCK_HOME/browser/sessions/<pid>.json`。
- 按页的会话身份找（`cordis_spine::mcp_session_key`，和 MCP 调用带的是同一个）。
- 会话还没开标签页：`no_tab`；浏览器没在跑：`browser_unavailable`。
- 名册里的页挂不上（崩了 / 被关了、名册还没改）：先经 MCP 调 `browser_tabs` 让它清掉死页。
  - 会话一页不剩：`no_tab`（推送里同样报 `no_tab`）；带了 `url` 就开新页。
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
- 流控：帧写到 WebSocket 之后才回 CDP `screencastFrameAck`，慢客户端不会攒帧。
- `event` 的形状（坐标是页面视口 CSS 像素，客户端按帧的 `width` / `height` 换算）：
  - `{type:"mouse", action:"move"|"down"|"up"|"click", x, y, button?, clickCount?, modifiers?}`
  - `{type:"wheel", x, y, deltaX, deltaY, modifiers?}`
  - `{type:"key", action:"down"|"up"|"press", key, modifiers?}`（DOM 键名：`Enter`、`a`）
  - `{type:"text", text}`（输入法上屏、粘贴）
  - `modifiers`：`["Alt","Control","Meta","Shift"]` 的子集。
- 地址栏：没写协议补 `https://`；只放行 http / https / about / data / file（`javascript:` 拒）。
- 这四个方法不占连接锁（挂上去要几秒）；视图是连接级的，不进会话、不落盘。
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

## 依赖注入

`mount` 声明依赖：`SESSIONS`、`SESSION_PORT`、`PERMISSIONS`、`ASK`、`PLAN_MODE`、`MCP`、`TURN`、`SETTINGS`。这些是 named service，在 `apply` 时 live-lookup，**不要**在闭包里持有 `Arc`。

## 相关文档

- 协议消费者侧：`embed-sdk/README.md`
- 产品面约束：根 `AGENTS.md` 的 Boundaries / Safety
- 架构：`docs/ARCHITECTURE.md`
