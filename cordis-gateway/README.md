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

## 协议（`protocol.rs` / `rpc.rs`）

- `PROTOCOL_VERSION` = `"dock.1"`，WS 路径 `WS_PATH` = `/api/ws`
- `CAPABILITIES`：`(name, supported)` 数组，`initialize` 时回给宿主
- 方法按域分在 `handlers/`：

| handler | 域 |
|---|---|
| `connection` | `connection/authenticate` |
| `thread` | 会话列表 / 启动 / 改名 / 归档 / 恢复 / 删除 / 历史 / 订阅；多线程：`thread/open` / `thread/close` / `thread/start {cwd}` / `thread/list {scope:"all"}` |
| `turn` | 发消息、流式回报 |
| `environment` | 环境信息、模型 / reasoning / approval / plan / memory / goal 设置 |
| `interaction` | `ask_user` 问答、权限请求 |
| `permission` | 权限授予状态 |
| `slash` | 斜杠命令远程执行 |
| `image_inputs` | 图片输入 |

- `transcript.rs`：把会话事件流转成 `dock.1` 的增量报文
- `LIVE_THREAD_ID` = `"live"` 指代第 1 页（根）的会话；老客户端只用它

### 多线程（能力 `openThreads`）

一个线程 = 一页（`threads.rs`）。`threadId` 是**落盘会话 id**，`live` 仍是第 1 页的别名；分页身份 `main#N` 只在网关内部用来路由事件。

| 方法 | 作用 |
|---|---|
| `thread/list { scope: "all" }` | 开着的页 + 所有目录的落盘会话（跨目录名册，不走 2 秒备忘），每项带 `cwd` / `open` / `presetId`（开着的页是它当前的预设，落盘的是 `meta.json` 记的，老会话为 `null`），第 1 页另带 `alias: "live"` |
| `workspace/list { scope: "all" }` | 有会话的所有目录（`id` = 路径，`hasOpenThreads`） |
| `thread/start { cwd, title?, presetId? }` | 在 `cwd` 另开一页（不动第 1 页），回它的会话 id；预设在创建时定下：`presetId` 只切这一页并记进会话，不改全局默认；预设不存在就报 `invalid_params`、不开页；不带 `cwd` 是老语义 |
| `thread/open { threadId }` | 把落盘会话开成一页（在它自己的 cwd 下），预设切回会话记的那个（不改全局默认）；已开着就回那一页 |
| `thread/close { threadId }` | 关页，会话留在磁盘上；第 1 页关不掉 |
| `preset/list`（能力 `presets`） | 可选的 Agent 预设（顺序同 TUI `/preset`）：`id` / `label` / `description` / `origin`（`shipped`/`user`/`project`）/ `available`（坏掉的为 `false` 并带 `error`），外加 `defaultId`。只读：预设在 `thread/start` 时定下，没有中途切换的方法 |

其它线程级方法（`turn/*`、`thread/environment/*` 与各种 `set`、`thread/subscribe`、`permission/resolve`、`interaction/respond`、`plan/resolve`、`elicit/resolve`、`slash/execute`）接受任意**开着的**线程的 id，没开着回 `thread_not_open`。`thread/history` 例外：关着的会话也给，`events` 由落盘事件按真实时间回放（`Transcript::replay`，和重开会话重建投影同一条路），每轮都以 `turn/completed` 收尾（结果照落盘的 `turn-end` 行报，停止、出错也看得出；更早的会话没有这一行，一律补成完成，最后一次采样带错误的补成失败），客户端开着关着只要一条路径。开页走 `"tui.tabs"` 的 `Tabs::open_at`（不切终端里正在看的页）；没挂分页服务时只有第 1 页。

投影每页一份（`handle.rs` 的 `transcripts`，共用一条 broadcast，事件带页身份）；订阅是「页 → 客户端订阅时用的 `threadId`」，推送时用那个 id。会话事件按 `session/page-event` 路由，`turn/completed` 只在那一页记下 `LogEvent::TurnEnd` 时发（一轮一次，流式中途不发；和其它会话事件一样按 `session/page-event` 路由），`status` 是 `completed` / `cancelled` / `failed`，失败带 `error`（错误文本，以前只回给 TUI）；`item/tool_completed` 的 `status` 照 Dock 落下的 `is_error` 报：`completed` / `failed`，停止时补的「已中断。」为 `cancelled`；权限 / 提问 / 计划 / elicitation 的事件载荷是 `()`，挨页按队首序号（`front_seq`）对账——同一条不重报，换了一条先报旧的 resolved。线程级的斜杠命令用 `GatewayHandle::scoped(page)`，下面一串 `cmd_*` 读的 `gateway.ctx()` 就是那一页。

## 依赖注入

`mount` 声明依赖：`SESSIONS`、`SESSION_PORT`、`PERMISSIONS`、`ASK`、`PLAN_MODE`、`MCP`、`TURN`、`SETTINGS`。这些是 named service，在 `apply` 时 live-lookup，**不要**在闭包里持有 `Arc`。

## 相关文档

- 协议消费者侧：`embed-sdk/README.md`
- 产品面约束：根 `AGENTS.md` 的 Boundaries / Safety
- 架构：`docs/ARCHITECTURE.md`
