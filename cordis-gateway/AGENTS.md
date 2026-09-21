# cordis-gateway / AGENTS.md

本包的编码约束。根 `AGENTS.md` 是总政策，本文只写 gateway 特有的安全边界；没提的以根文件为准。

## 安全边界（不可逾越）

Gateway 是 Dock 唯一把会话暴露到进程外的地方，三条设计是**有意的**，不是待修的缺陷：

### 1. 只绑 loopback

`bind::parse_bind` 拒绝非回环地址，`bind_loopback` 绑定后**二次校验** `local_addr()` 仍是 loopback（防 OS / 容器把 `0.0.0.0` 解析成回环之类的怪事）。

- **不要**改成可绑非 loopback，不要为"远程访问"加口子
- 需要远程？走宿主页面 + `embed-sdk`，让页面连本机回环，而不是让网关监听公网

### 2. CORS 反射 Origin（不是白名单）

`http.rs` 的 `cors` 中间件按请求头反射 Origin，源码注释写明了理由：

> Authorization is Origin pairing + loopback bind, not a CORS allowlist. A
> malicious page can hit the API but cannot get a ticket without TUI approval
> for that Origin.

- **不要**加 Origin 白名单 / allowlist 校验
- 本地开发页面（vite、`file://` 旁的 http、别的 app）都要能敲回环 bootstrap，白名单会挡死这些合法场景
- 真正的鉴权在下一层：配对 + 一次性 ticket

### 3. 鉴权靠 `/pair` + 一次性 ticket

- 宿主页面发配对请求 → 用户在 TUI 确认（`PairingStore::confirm`）→ 换得 ticket（`TICKET_TTL` = 1 小时）→ WS 用 ticket 鉴权
- ticket **一次性**，交换后失效
- 未配对时**不监听**：`gateway()` / `gateway_idle()` 只 mount，`start_listen` 由 TUI `/pair` 触发

因此：任何"简化鉴权"、"默认信任 localhost Origin"、"允许跳过用户确认"的改动都要先和用户确认。

## 编码约束

- named service 在调用点 live-lookup（`ctx.get` / `ctx.require`），**不要**把 `Arc<Sessions>` 等关进长生命周期闭包。`mount` 的 `Inject` 只声明 service key，这既是约定也是结构上的强制
- `companion_listener` 的绑定失败必须走 `CompanionStatus::Failed` 报给 UI，**从不静默**
- 端口冲突按 `PORT_SEARCH`（=32）向上找；`port 0` 交给 OS 分配。不要静默退化到别的端口而不告诉用户
- dispose 必须 `stop_listen`（`ctx.effect("gateway-http", …)` 注册的 `Disposable`）

## 测试

```bash
cargo test -p cordis-gateway
cargo clippy -p cordis-gateway --all-targets --no-deps -- -D warnings
```

`gateway_bind(addr)` 是给集成测试用的（立即绑定）；生产路径是 `gateway()`，不要在产品代码里调 `gateway_bind`。

## 改协议时

- `PROTOCOL_VERSION` = `"dock.1"`。协议变更属于 public API 面，按根 AGENTS.md 的 **Ask first**
- 改方法/能力同步 `embed-sdk/README.md`（消费侧）与 `docs/`（如有协议文档）
- `CAPABILITIES` 数组与 `initialize` 的回包要一致
