# 审计

开了审计的页，它做过什么都记进一份和聊天记录分开的审计日志：
工具调用、命令、命令和浏览器的出网（HTTPS 解密）。
任务结束可以导出报告，看链路图、时间线、每次请求的头和正文。

审计不减少 agent 能做的事：工具一颗不少，命令照常跑，连不上的不会因为审计而连不上。
它只是看得见、记得住、证明得了。

审计是[沙箱](SANDBOX.md)的观测与审计部分：开着沙箱的页才记。
引擎在 crate `dock-sandbox` 的 `audit` 模块（不含插件），
接进 Dock 的是 spine 的 `audit` 插件（`cordis-spine/src/host/audit.rs`）。

## 开关

- 跟着沙箱走：`/sandbox on` 开当前页（别名 `/audit`），`/sandbox off` 关。
- `$DOCK_HOME/config.toml` 写 `[sandbox] enabled = true`，新开的页默认开。
- 只认用户配置：项目 `.dock/config.toml` 里的 `[sandbox]` 不认，仓库作者不能替你关掉审计或者停止遮凭据。
- 子代理（含工作流派的）记进派它那一页的审计。
- **关着时什么都不做**：不起出口代理、不注入环境变量、不写盘，开销为零。

## 记什么

| 记录 | 什么时候 | 内容 |
|---|---|---|
| `audit` | 开 / 关 / 出错 | 页、会话 id、工作目录、是否解密 |
| `tool_call` | `tools/pre-execute` | 工具、调用 id、参数（16KB 内全文，另记长度和哈希）、是否被拦 |
| `tool_result` | `tools/execute` | 调用 id、成败、结果开头 4KB、长度、哈希、图片数 |
| `command` / `command_end` | `jobs` 起进程 / 结束 | job id、调用 id、程序、命令全文、目录 / 退出码、耗时 |
| `net` | 一次出网 | 见下 |

每条出网的 `tag` 是发起它的命令的 job id（浏览器的是 `browser:<标签页>`），
所以链路是通的：工具调用 → 命令 → 出网。

## 出网怎么记

| 出口 | 怎么记 | 看得到 |
|---|---|---|
| 命令和它的子进程 | 每条命令一个回环出口代理口，注入代理和证书的环境变量 | 解密时：方法、网址、状态、头、正文开头；不解密时：目的地、字节数 |
| 内置浏览器 | CDP Network 事件（浏览器服务自己写 `browser.jsonl`） | 方法、网址、状态、头、远端 IP；页面文档、XHR、fetch 的正文开头 |
| Dock 自己（模型请求、web_fetch、MCP HTTP） | 工具调用记录 | 模型请求本身在聊天记录里，不重复存 |

### 命令的出口代理（`dock_sandbox::audit::proxy`）

- HTTP 代理协议，只听 `127.0.0.1`，每条命令一个口，流量记录带上那条命令的 job id。
- 出网先过沙箱策略（`[[sandbox.rules]]` 的 `host`，见 [SANDBOX](SANDBOX.md)）：`deny` 回 403 并记一条，`ask` 在发起命令的那一页问。
- 环境变量（`dock_sandbox::audit::env`）：
  - 代理：`HTTP(S)_PROXY` / `ALL_PROXY` 大小写两套、`NO_PROXY`、`NODE_USE_ENV_PROXY=1`。
  - 证书：`SSL_CERT_FILE` / `CURL_CA_BUNDLE` / `REQUESTS_CA_BUNDLE` / `GIT_SSL_CAINFO` / `CARGO_HTTP_CAINFO` / `PIP_CERT` / `npm_config_cafile` 指向「系统根证书 + Dock 根证书」，`NODE_EXTRA_CA_CERTS` 指向 Dock 根证书。
  - 写成命令前面的 `export …;`：命令是 `bash -lc` 起的，`~/.bash_profile` 里自己 export 的代理会盖掉进程环境。
- 发往本机（`localhost`、`127.0.0.1`、`::1`）的不走代理：那不是出网，本地开发服务器照常连。
- 用户本来就有代理（Dock 进程环境里，或者登录 shell 的 profile 里 export 的）：出口代理往外连时经它走，本来要靠它才连得上外网的命令照样连得上。只认 `http://` 代理。

### HTTPS 解密

- Dock 自己的根证书在 `$DOCK_HOME/audit/ca/`（私钥 0600），第一次用时生成。
  只经命令的环境变量信任，**不装进系统钥匙串**，不影响 Dock 以外的程序。
- 代理按主机现签叶子证书（ECDSA P-256，30 天，带 AKI：Python 3.13+ 的严格校验要它）。
- **先连上游、先验上游证书**（系统证书库），过了才对客户端做中间人。
- 下面这些原样转发，只记目的地和字节数，不让命令因为审计而失败：
  - 上游证书过不了系统校验（自签的开发服务器、客户端加了 `-k`）：交给客户端自己判断。
  - 客户端只说 h2（gRPC）：解密后只能讲 HTTP/1.1。
  - 客户端拒绝了 Dock 根证书：那一次失败，之后这个主机原样转发。两种都认：
    - 握手里收到证书类的 alert（UnknownCA、BadCertificate 等，macOS 的 curl、rustls 客户端会发）：确定被拒，记进 `$DOCK_HOME/audit/passthrough.json`，30 天内都原样转发。
    - 说不准的：握手里别的失败（断网、进程被杀、macOS 系统 TLS 不发 alert 直接断开），或握手做完 3 秒内一个字节不发就断开（Linux 上 curl 8 + OpenSSL 握手后才验证书）。只在这次运行里原样转发，不落盘，重启后再试着解密。
    - `/sandbox passthrough` 列出名单（标出哪些只在这次运行里），`/sandbox passthrough clear [主机]` 从盘上和正在跑的代理里一起清掉。
  - 已知不读证书环境变量的：macOS 上的 Go、Java（看 CONNECT 的 User-Agent）。
  - 不是 TLS 的隧道（SSH over CONNECT 之类）。
- 凭据类的头（`Authorization`、`Cookie`、`Set-Cookie`、`X-Api-Key`…）遮住，留长度和 sha256 前 12 位：
  能比对是不是同一个，看不到原文。`redact_secrets = false` 关掉。
  **正文不遮**：只按 `body_cap_kb` 截断。登录接口回的 token、表单里提交的密码会原样留在 `bodies/` 里，导出报告前留意。
- 本机实测（2026-11，macOS）：curl、git、python urllib、npm、Node 原生 fetch 都能解密；Go 原样转发。

## 存哪、怎么证明

- `$DOCK_HOME/audit/<开始时刻>-<会话>/`：`events.jsonl`（Dock 写）、`browser.jsonl`（浏览器服务写）、`bodies/<sha256>`（正文块，同内容只存一份）。
- 只追加。每行带上一行的 sha256（`prev`），改过或删过中间一行，之后的链就断了；记录引用的正文块不见了、内容对不上哈希也算。`/sandbox` 状态里和导出的 `verify.txt` 都会说。
- 写盘在单独的线程上：调用方只往有界队列（8192 条）里放，满了丢掉、计数，下一条记录里报 `dropped`。审计不拖慢 agent。关掉沙箱、退出时等这个线程把队列写完。
- 正文最多记 `body_cap_kb`（默认 256KB），之后只计长度。浏览器只取页面文档、XHR、fetch 的正文，大于 4MB 的不取。

## 看和导出

- `/sandbox`：状态（记录在哪、多少条、哈希链完不完好、出口代理、直通名单）。
- `/sandbox graph`：链路图。会话 → 工具 → 命令里的程序 → 目的主机，边上是次数和流量，
  主机多了每个来源只列流量最大的 8 个。终端里是缩进树，后面附 Mermaid（桌面端画成图）。
- `/sandbox export`：`<审计目录>/export-<时刻>/`，里面有：
  - `report.html`：自包含、不带脚本、打开不出网。概况、完整性、链路图（离线渲染成 SVG）、没解密的连接和原因、时间线；每次出网可以展开看头和正文开头。
  - 原始 `*.jsonl`、`bodies/`、`verify.txt`。

## 已知缺口

- 不认代理环境变量、自己直连的程序看不到（老版本 Node 的原生 fetch、写死地址的二进制）。下一步：Linux 用 seccomp 用户通知观察 `connect()`，macOS 扫命令进程树的 socket。
- MCP stdio 服务器、LSP、cua-driver 驱动的桌面应用不经出口代理。
- 改了 `[sandbox]` 的 `decrypt` / `body_cap_kb` / `redact_secrets` 要重启 Dock（出口代理第一次用时按当时的配置起）。
- 不认代理环境变量的程序：开了限制并设 `network = "proxy"` 时（macOS）它们连不上，出网一条不漏；没开时看不到。

## 配置

在 `[sandbox]` 里（见 [SANDBOX](SANDBOX.md)「配置」）：`decrypt`、`body_cap_kb`、`redact_secrets`。

## 手动试

```sh
cargo run -p dock-sandbox --example egress -- curl -s https://example.com -o /dev/null
```

跑完列出出网、打印链路树，并导出一份报告。
