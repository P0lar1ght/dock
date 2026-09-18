# 网络取回（tool-web）

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-web`

- **ctx**：`"tools"`
- **模型工具**：`web_fetch` `web_search`

一颗插件两个 register（`mod.rs:30`）。命名沿用 DSH `tool-web`，粒度是**套件**——两个工具共用同一条 fetch 管道，拆成两颗插件只会让 SSRF 策略出现两份。注册走 `tools.register` 而非 `register_deferred`，所以两颗默认进 sampler 与 `specs_for_model`（`round.rs:314` 断言在册）；`own_registered`（`registry.rs:471`）把 dispose 交给 fiber，插件卸载时两个 register 一起撤。

实现是 Grok `xai-grok-tools` 那套 `web_fetch` 的复制件（`ssrf` / `http` / `domain` / `error`），剥掉了 `register_resource!`、`tracing`、schemars 与 xAI 账号 client。

## 管道

`fetch_url(raw, params)`（`fetch.rs:194`），两个工具共用：

1. **`validate_url`**（`fetch.rs:27`）——scheme 只收 `http`/`https`；长度 ≤ 2000（`MAX_URL_LENGTH`）；**URL 里带账密直接拒**（`CredentialsInUrl`）；主机名点分段少于 2 且不是显式本地主机时拒（`SingleLabelHost`，`https://intranet/` 进不来）。
2. **`upgrade_to_https`**（`fetch.rs:61`）——`http` 无条件抬到 `https`，**显式 loopback 主机除外**（否则 `http://127.0.0.1:8080` 会被抬成 https 而连不上）。
3. **域名 allowlist**（`fetch.rs:198`）——`params.allowed_domains()` 为 `None` 时整段跳过，SSRF 仍生效。
4. **`HttpClient`**（`http.rs:15`）——reqwest，`redirect::Policy::none`（跳转靠手搓），10s connect / 默认 60s total，gzip+brotli+deflate，`pool_max_idle_per_host(2)`。客户端存在 `ArcSwapOption` 里，遇到 `HttpRequest` 错误就 `invalidate()`（`fetch.rs:220`），下次调用重建。
5. **`fetch_hops`**（`fetch.rs:74`）——手搓跳转循环，**每一跳都重跑 `check_ssrf`**。同 host 才跟（`is_same_host` 只比 `host_str`，不比端口/scheme），最多 10 跳。跨 host 不当错误抛，而是返回一条 `FetchResult::CrossHostRedirect`，由 `to_prompt` 渲染成「Make a new web_fetch call with the redirect URL if needed.」——模型自己决定跟不跟。
6. **正文处理**（`to_prompt`，`fetch.rs:165`）——`text/html` / `application/xhtml` 交给 `htmd` 转 markdown，跳过 `script` / `style` / `noscript` / `svg` / `iframe` / `object` / `embed`；非 HTML 原样。超过 `max_markdown_length`（默认 100 000 字符）就**截断并追加 `[truncated]`**。
7. **成品形状**：`HTTP {status} {final_url}\n\n{text}`。

请求头固定 `Accept: text/markdown,text/html,…`、`Accept-Language: en-US;q=0.9`，UA 是 `Mozilla/5.0 (compatible; grok-agent/1.0; +https://x.ai)`（`config.rs:26`）。

### 参数与边界

| 项 | 默认 | 备注 |
|---|---|---|
| `timeout_secs` | 60 | connect 恒 10s，不可配 |
| `max_content_length` | 10 MB | 超了报 `ResponseTooLarge`，**已经下载完才判**，不是流式中断 |
| `max_markdown_length` | 100 000 | 只截模型看到的那份 |
| `MAX_URL_LENGTH` | 2000 | 安全边界，不可配 |
| `MAX_REDIRECTS` | 10 | 安全边界，不可配 |

**「报错」有两种壳。** 真错误经 `WebFetchError` 变成 `tool_result`，文案如 `SSRF blocked: …`、`HTTP request failed: …`。但**域名被 allowlist 拒**（`fetch.rs:198`）和**跨 host 重定向**（`fetch.rs:167`）返回的是 `Ok(String)`，内容却以 `Error:` 开头——调用方看到的是「成功返回了一段以 Error 开头的文本」。这是 Grok 带过来的形状，不是本仓引入的，改它要连模型侧话术一起调。

## SSRF

`ssrf.rs` 的策略，`check_ssrf`（`ssrf.rs:149`）在**每跳**执行：

- **DNS 先解**（`tokio::net::lookup_host`），解析出的地址**只要有一个非公网就整条拒**（`addrs.iter().find(...)`）——不是「挑一个公网的用」。空结果报 `DnsEmpty`。DNS rebinding 由此关闭：`evil.example.com` 指向 `127.0.0.1` 一样拦。
- 非公网清单（`is_non_public_ipv4`，`ssrf.rs:50`）：loopback、RFC 1918、link-local、`0.0.0.0/8`、`100.64.0.0/10`（CGNAT，云元数据那类）、`192.0.0.0/24`、三个 TEST-NET、`198.18.0.0/15`（RFC 2544 benchmarking）、`240.0.0.0/4`。IPv6 侧：loopback / unspecified / multicast / ULA / link-local，IPv4-mapped 递归按 v4 判。
- `allow_local`（`[toolset.web_fetch]`）只在**主机名本身是显式本地**（`localhost`、`127.0.0.0/8` 字面量、`::1`，含 IPv4-mapped 形式）时开 loopback，且**私有/link-local 永不开**。所以它救不了内网，也不给 rebinding 留口（`rebinding_hostname_to_loopback_stays_blocked`）。
- 被拦的文案：`SSRF blocked: {host} resolves to private/internal IP {ip}`。若主机名含 `github` **且 PATH 上真有 `gh`**，追加一句 ``. Use the `gh` CLI instead (e.g. `gh pr view` or `gh api`).``（`error.rs:73`）——为的是别让模型把「私网不可达」误判成「没有这个资源」。

### 配了代理时跳过本地 DNS 预检

`check_ssrf` 的第三个参数 `via_proxy` 来自 `WebFetchParams::via_proxy()`（即 `proxy_endpoint` 非空，**空白串不算**）。`skips_local_dns(host, via_proxy)` 为真时整个 DNS 预检直接跳过，连 `lookup_host` 都不发。

理由：出口是代理，连接目的地址由**代理侧**解析，本地这次查询的结果跟请求会连到哪里没有关系。fake-ip / 分流 DNS 下它返回的是保留段假址（本机实测 `example.com` → `198.18.0.164`，正好落在上面那条 RFC 2544 段里），拿它当证据就会把每一次取页面都判成 SSRF。

两处**仍然不跳过**，它们不依赖本地解析：

| host 形态 | 配了代理 | 没配代理 |
|---|---|---|
| 普通域名 `example.com` | 跳过预检，交给代理 | 照旧查 DNS 并逐地址判 |
| IP 字面量 `10.0.0.1` / `169.254.169.254` / `1.1.1.1` | 照旧按 `is_blocked_for_host` 判 | 同左 |
| 显式本地主机名 `localhost` / `127.0.0.1` / `::1` | 照旧，仍由 `allow_local` 决定 | 同左 |

**这是配置代理隐含的信任转移，说明白写在这里**：代理若是本机进程（Clash 那种），它自己会去解析并连内网，也就是说代理成了信任边界。`skips_local_dns` 放开的只是「本地解析结果」这个**证据**，不是权限本身。

### 在 fake-ip 机器上的可用配置

```
[toolset.web_fetch]
proxy_endpoint = "http://127.0.0.1:7897"   # Clash Verge 的 mixed 端口，默认 7897
```

**`proxy_endpoint` 必须指向一个真有监听者的 HTTP/SOCKS 口**——它是真的会去连的，不是只当开关用。纯 TUN（系统代理关、核心只起 utun 不起 mixed 端口）的机器上填一个没人听的口，会把每个请求从「被 SSRF 拦」变成「连代理失败」，等于没修。先 `nc -z 127.0.0.1 7897` 确认。

注意 Clash Verge 的 mihomo 核心以 **root** 跑（TUN 需要），`lsof` 用普通用户看不到它的 socket——`lsof -iTCP -sTCP:LISTEN | grep mihomo` 会是空的，那不是「没在听」，用 `nc -z` 探。

> **踩过的坑（触发这次接线的原因）**：接线之前，Clash / Surge 的 TUN fake-ip 模式下 `web_fetch` / `web_search` **100% 被 SSRF 闸拦死**（`example.com` → `198.18.0.164`），而同一个 shell 里 `curl` 完全正常。`ssrf.rs` 的 `blocks_testnet_reserved_and_this_network` 至今仍断言拦 `198.18.0.1`——那条是给**直连**路径的，代理路径现在走 `skips_local_dns` 绕过。

## `web_search` 不是搜索 API

Grok 的 `web_search` 打 xAI Responses API，dock 没有账号 client，所以（`fetch.rs:228` 的注释原话）**复用同一条 fetch/SSRF 管道去打一个公开 HTML 索引**：

1. `query` 为空报 `empty query`（塞在 `InvalidRedirect` 里）。
2. 拼 `https://html.duckduckgo.com/html/?q=…`（`fetch.rs:236`）。
3. `fetch_url` 走完上面整条管道。
4. `extract_results`（`fetch.rs:247`）在正文里 `find("http")`，往后切到空白 / `"` / `<` / `'`，丢掉含 `duckduckgo.com` 的、去重，**最多 8 条**。

于是它的输出**只有 URL 列表，没有标题也没有摘要**。零命中时返回 `No results for "…". Snippet:` 加正文前 800 字符——把原文吐出来是刻意的，否则模型只知道「没有」，不知道是页面结构变了还是真没结果。

**实测形状**（2026-09，走 Clash 7897 真打一次）比源码看起来更糙：DuckDuckGo 的结果链接是 `//duckduckgo.com/l/?uddg=<百分号编码的真 URL>&rut=<token>` 这种跳转壳，`extract_results` 切出来的是**百分号编码后**的串：

```
Search results for "rust 1.88 release notes":
1. https%3A%2F%2Fblog.rust%2Dlang.org%2F2025%2F06%2F26%2FRust%2D1.88.0%2F&rut=eb24be86…
```

要真用得上得先 percent-decode 再拆 `uddg`，那一层解码 + 标题/摘要提取没做。另外端点常直接回 `HTTP 202` 的机器人挑战页（那次正文里除了 doctype 什么都没有）。

`arg_query` 除了 `query` 还认 Grok 的 `queries[0]`（`mod.rs:80`）。

**`allowed_domains` 在 schema 里是个哑参**：`SEARCH_PARAMS`（`mod.rs:20`）向模型声明了它，但 `arg_query` 从不读它（只读 `query` / `queries[0]`）——模型写进去不会报错，也不会有任何效果。域名白名单真正的入口是 `[toolset.web_fetch] allowed_domains`，且它对搜索路径不生效，见下节。

## 配置

`[toolset.web_fetch]`，读 `~/.dock/config.toml` < 项目 `.dock/config.toml`，**逐字段**覆盖（项目只写 `timeout_secs` 不会把用户那份的 `proxy_endpoint` 清掉）。解析在 `cordis-base/src/config.rs`（`load_web_fetch_config`），装配在 `bundle.rs:117` 的 `ctx.plugin(tool_web(), web_fetch_params())`——插件体不 live-read 磁盘，拿到的是已定好的 `WebFetchParams`（和 `tool-task` 收 `TaskConfig` 同一个形状）。

| 键 | 默认 | 作用 |
|---|---|---|
| `timeout_secs` | 60 | 请求总超时 |
| `max_content_length` | 10 MB | 响应体上限 |
| `max_markdown_length` | 100 000 | 喂给模型的 markdown 上限 |
| `allowed_domains` | 不设 = 不限 | 域名白名单，见下 |
| `proxy_endpoint` | 不设 | 转发代理；**设上即跳过 SSRF 本地 DNS 预检** |
| `allow_local` | false | 只放行显式本地主机名 |

**只有会生效的键进配置面。** Grok 的 `WebFetchParams` 还带 `cache_ttl_secs` / `max_cache_entries` / `context_window_tokens`，但 dock 没有页面缓存、也没实现那个 3% 上下文帽——把死键摆出来只会让人以为自己配上了。写进配置的未知键会被 `warn_unknown_web_fetch_keys` 记一条 `tracing::warn!`（结构体**故意不加** `deny_unknown_fields`：`read_file` 是一把 `toml::from_str` 整份文件的，一个 typo 不该让 models 一起消失）。

**`allowed_domains` 的三种状态不是一个意思**：不写 = 不设闸（SSRF 仍在）；写了 = 只许这些域；写 `[]`（显式空表）= **全部拒绝**——`DomainMatcher` 对空表是「没有一条允许」（`blocks_all_when_empty`）。

`web_search` **不受** `allowed_domains` 约束：它打的是自己的固定端点，白名单是给模型点名的 URL 用的闸，`html.duckduckgo.com` 不可能出现在任何人的白名单里，照判就是整颗 `web_search` 报废（`search_web` 里把 `allowed_domains` 清掉再进 `fetch_url`）。SSRF 与其余参数对搜索照常生效。

注意 `WebFetchParams` 上还有 `#[serde(deny_unknown_fields)]` 和一个 `register_resource!` 时代留下的形状；从 `[toolset.web_fetch]` 进来的是 `WebFetchToolConfig`（`cordis-base`），两者靠 `WebFetchParams::from_tool_config` 对接。

## 覆盖

| 面 | 用例 |
|---|---|
| 走通管道 / URL 校验 / htmd / 客户端重建 | `web_fetch::fetch` `::http` 单测（离线） |
| SSRF 地址判定与 `allow_local` 双闸 | `web_fetch::ssrf` 单测（离线，只用 IP 字面量） |
| 代理跳过判定的**决策** | `skips_local_dns_only_for_plain_names_under_proxy`（纯函数，环境无关） |
| 代理跳过判定的**行为** | `via_proxy_skips_dns_for_plain_hostname`：同一 `.invalid` 名字，`via_proxy=true` 必须 Ok、`false` 必须 Err |
| 配了代理仍拦 IP 字面量 | `via_proxy_still_blocks_ip_literals` |
| 配置解析 / 逐字段 overlay / 空表 / typo 不毁文件 | `cordis-base` 的 `web_fetch_toolset_*`、`unknown_web_fetch_key_does_not_drop_the_rest_of_the_file` |
| 配置真的到了工具上 | `tests/web_toolset.rs` 三条：白名单在 I/O 前生效、不配 `allow_local` 时本地地址被拦、配了之后放行到发请求 |

`tests/web_toolset.rs` 三条都刻意不碰外网：白名单在发请求前就拒，`127.0.0.1:1` 是必然连不上的显式本地地址。所以离线 CI 结果确定。

**fake-ip 那条原始故障本身没法进 CI**（跑测试的机器不一定有 fake-ip 解析器），所以拆成两半覆盖：决策用纯函数、I/O 行为用 `.invalid` 名字的 `Err`/`Ok` 对照。真机的端到端验证是手动做的——临时把 `proxy_endpoint` 指到本机 Clash 的 mixed 端口，`web_fetch` 打 `https://example.com/` 与 `https://docs.rs/htmd/latest/htmd/` 都拿到真实正文，`web_search` 拿到结果链接。那次没有留下测试文件，因为结果依赖本机代理端口。
