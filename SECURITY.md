# 安全

## 上报漏洞

不要在公开 issue 里贴漏洞细节。用 GitHub 的私有渠道：仓库 **Security → Report a vulnerability**（<https://github.com/P0lar1ght/dock/security/advisories/new>）。该渠道只对仓库维护者可见，不需要额外邮箱。

请在报告里给：影响版本或 commit、复现步骤、影响范围、是否需要本机交互。

## 范围

Dock 在本机跑一个 Agent，能读写工作区、跑命令、连模型 API、开回环 Gateway、经 MCP 驱动浏览器与桌面。这些是**预期能力**，不是漏洞：

- Agent 在你批准后读写工作区文件、执行命令。
- 回环 Gateway 在 `/pair` 开启后监听 `127.0.0.1`（占用换端口，同端口再试 `[::1]`）。
- `dock serve --remote` 常驻在回环上，由你自己挂的反向代理 / Tailscale 对外（见 [docs/REMOTE.md](docs/REMOTE.md)）。
- 持设备令牌的客户端能做 Dock 能做的一切。
- 配对成功后 host 页能投递 prompt、读会话投影、走斜杠目录。

算漏洞的：

- 绕过 `/pair` 配对 / 一次性 ticket / TTL，或让 Gateway 绑到非 loopback 地址。
- 不持有效设备令牌连上远程网关；撤销后的令牌仍能用；令牌明文落盘或进日志。
- 绕过权限模式与计划门执行工具（含 `bash`、CUA 桌面操作、`browser_*`）。
- 密钥、`~/.dock/mcp_credentials.json`、会话 jsonl 泄漏到源码、日志、PR、宿主页。
- 未信任输入（模型输出、MCP 返回、网页内容、contributor 脚本）触发的任意代码执行或路径逃逸。

## 已知设计取舍（不是漏洞）

- Gateway 的 CORS 反射 Origin：鉴权靠配对 + 回环，不靠 Origin 白名单。
- 设备令牌不绑 Origin：令牌就是凭据，只该在 https 上传（SDK 对非回环地址强制 https）。
- 远程网关只做单连接的失败次数限制（5 次断开）；全局限速交给反向代理。
- 未配置或连不上的 MCP server 保持 `Active`（fail-open），只写空 / 失败状态。
- 冻结的 `vendor/` 副本带上游代码，漏洞请按上游处理并在此仓库记录同步。

## 使用者注意

- 密钥、API key、`.env`、真实用户数据不进源码、commit、PR、日志。
- `~/.dock/mcp_credentials.json` 存 HTTP MCP 的 OAuth token，不要写进 `config.toml`。
- 不要跑未信任 contributor / fork 的脚本、构建钩子或二进制。
