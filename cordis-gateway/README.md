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
| `thread` | 会话列表 / 启动 / 改名 / 归档 / 恢复 / 删除 / 历史 / 订阅 |
| `turn` | 发消息、流式回报 |
| `environment` | 环境信息、模型 / reasoning / approval / plan / memory / goal 设置 |
| `interaction` | `ask_user` 问答、权限请求 |
| `permission` | 权限授予状态 |
| `slash` | 斜杠命令远程执行 |
| `image_inputs` | 图片输入 |

- `transcript.rs`：把会话事件流转成 `dock.1` 的增量报文
- `LIVE_THREAD_ID` = `"live"` 指代当前实时会话

## 依赖注入

`mount` 声明依赖：`SESSIONS`、`SESSION_PORT`、`PERMISSIONS`、`ASK`、`PLAN_MODE`、`MCP`、`TURN`、`SETTINGS`。这些是 named service，在 `apply` 时 live-lookup，**不要**在闭包里持有 `Arc`。

## 相关文档

- 协议消费者侧：`embed-sdk/README.md`
- 产品面约束：根 `AGENTS.md` 的 Boundaries / Safety
- 架构：`docs/ARCHITECTURE.md`
