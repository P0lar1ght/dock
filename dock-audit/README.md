# dock-audit

Dock 的审计引擎（不含插件）。接进 Dock 的是 spine 的 `audit` 插件，说明见 [docs/AUDIT.md](../docs/AUDIT.md)。

- `ca`：Dock 自己的根证书，按主机现签叶子证书。
- `proxy`：出口代理（HTTP 代理协议，回环）。每条命令一个口；HTTPS 解密，客户端不信 / 只说 h2 / 上游证书没过的原样转发。
- `env`：给命令的代理与证书环境变量。
- `record`：一次出网（`Flow`），凭据头遮住。
- `store`：只追加的审计日志，哈希链、正文块按内容存、后台写盘。
- `graph`：链路图（Mermaid、终端缩进树）与自包含的 HTML 报告。

```sh
cargo test -p dock-audit                 # 单测 + 真 curl 走代理的端到端
cargo run -p dock-audit --example egress -- curl -s https://example.com -o /dev/null
```
