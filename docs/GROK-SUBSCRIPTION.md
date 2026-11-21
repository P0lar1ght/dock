# Grok 订阅通道（SuperGrok / X Premium）

让 Dock 用 **SuperGrok / X Premium 订阅额度**跑推理。
在 `auth.x.ai` 做 OAuth，拿到的 access token 当 Bearer 打公开 API：
`https://api.x.ai/v1/responses`。

代码在 `cordis-spine/src/auth/grok.rs`（provider 插件 `grok-auth`）。
框架与 ChatGPT 那条共用，见 `docs/CHATGPT-SIWC.md`。

## 来源与边界

xAI 2026 年 5 月起让订阅在第三方 agent 里用。
它点名接入的有 OpenCode、Kilo Code、Hermes Agent 等。

- `auth.x.ai` **没有自助注册 client**（discovery 里没有 `registration_endpoint`）。
- 那几家都复用 Grok CLI 的公开 client id，用 `referrer=<应用名>` 标明来源。
  - OpenCode：`referrer=opencode`；Kilo：`referrer=kilocode`。
- Dock 照做：同一个 client id，`referrer=dock`。
- xAI 没有成文说「没被点名的工具也能这样用」。
  - 哪天它收紧了，登录会报 `invalid_client` 之类，Dock 会说清楚、让你改用 `api_key`。

**不做的事**：

- 推理不打 Grok Build 自用的 `cli-chat-proxy.grok.com`（唯一的例外是只读查额度，见「订阅额度」）。
- 不带它的身份头（`X-XAI-Token-Auth`、`x-grok-client-identifier`……）。
- 不读 `~/.grok/auth.json`。

## 怎么用

```text
/auth login grok        # device code：打开填好用户码的授权页，在浏览器里点同意
/auth status            # 各家的活动账号、订阅授权、token 剩余时间
/auth models grok       # 重拉可用模型并入模型目录
/model grok/grok-4.6    # 切过去
/auth logout grok       # 撤销 refresh token、清本地 token
/auth login new grok    # 再加一个 xAI 账号
```

- 登录在后台跑，提示里给出授权页地址和用户码。
- 授权页地址带着用户码；确认页面上的码和 Dock 里的一致再点同意。
- 用户码上游给 30 分钟，Dock 只等 10 分钟。
- 不起回环监听：VPS / SSH 上在任何设备上打开那个地址就行。
- 桌面端：设置 › 模型 › 订阅通道（网关 `auth/login { provider: "grok" }`）。

手写模型条目也行：

```toml
[model."my-grok"]
api_backends = ["responses"]
api_model = "grok-4.7"
auth = "grok"   # 地址由 provider 给，api_base_url 可以不写
```

## 订阅额度

查的是 xAI 订阅的额度池：本周期已用百分比、周期起止、套餐名、预付余额。

- 终端：`/auth usage [grok]`，一行：`Grok（SuperGrok）：本周已用 27%，还剩 73% · 2 天 14 小时后重置`。
- 桌面端：输入框旁「Using Grok plan」换成剩余额度的小圆环（点开是详情卡）；设置 › 模型 › 订阅通道里一张额度卡。
- 网关：`auth/usage { provider }` → `{ provider, usage }`；查不了的那家（ChatGPT）`usage` 是 `null`。只认受信连接。
- 每次都问上游，Dock 不缓存；界面自己定多久查一次（打开时、每 5 分钟、一轮结束后）。

公开 API 上**查不到**订阅额度，只有 Grok Build 自用的后端有：

| 接口 | 用途 |
|---|---|
| `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` | 已用百分比、周期、预付余额 |
| `GET https://cli-chat-proxy.grok.com/v1/settings` | 套餐名 `subscription_tier_display`（查一次就记住） |

- 这是「推理不走 cli-chat-proxy」之外**唯一的例外**：只读、只带 Bearer。
- 实测只带 `Authorization` 就回 200，不带 Grok Build 的身份头，也不冒充它的版本号。
- xAI 点名接入的 OpenClaw 查额度也是这个地址（它带 `x-grok-client-mode` / `x-grok-client-version`）。
- token 被拒（403 `unauthenticated:*`）就刷新一次再查。

实测返回（SuperGrok，2026-11-21）：

```json
{"config": {
  "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "start": "2026-11-17T02:21:38…", "end": "2026-11-24T02:21:38…"},
  "creditUsagePercent": 27.0,
  "productUsage": [{"product": "GrokBuild", "usagePercent": 27.0}],
  "prepaidBalance": {"val": 0}, "onDemandCap": {"val": 0}, "isUnifiedBillingUser": true}}
```

- 只给百分比，没有 token 数：界面上只写百分比，不编数。
- 本周期刚开始、一点没用时上游会把 `creditUsagePercent` 省掉（proto3）：正处在周期里按 0 算，否则当作不知道。
- 老的账单形状（`monthlyLimit` / `used`，美分）也认。

**没核实的一点**：这个池子算不算 Dock 经 `api.x.ai` 的用量。

- 同一个 token 在 `api.x.ai` 上发了 3 次约 5 万 token 的请求（xAI 折算约 0.1 美元），5 分钟后再查仍是 27.0%。
- 可能是 `api.x.ai` 的 OAuth 用量不进这个池，也可能是池子大、这点花费不到显示精度。
- 所以界面上标的是「xAI 订阅额度池（与 Grok Build 共用、按它计量）」。

## 落盘

```text
$DOCK_HOME/credentials/grok/<账号>.json   0600，目录 0700
$DOCK_HOME/credentials/grok/<账号>.lock   刷新 / 登出的跨进程锁
$DOCK_HOME/credentials/grok/active        当前活动账号
```

- 所有账号共用同一个 OAuth client，文件**按账号分**：`<账号>` 是 ID token 的 `sub`。
- 凭据格式与 ChatGPT 那条相同（`Credential`）。
  - 这一家的 `client_id` 字段存的是账号 id，发给 `auth.x.ai` 的永远是公开 client id。
  - `ext_agent_host_id` 空着：xAI 没有这个概念。
- 不支持 `/auth import`：device code 本来就能在 VPS 上直接登录。

## 实测（2026-11-21，SuperGrok 账号）

用 `/tmp` 下的探针脚本（只用标准库）直接打 xAI，结论：

登录与 token：

- device code：`POST /oauth2/device/code` 回 200，`expires_in` 1800、`interval` 5。
- 授权页是 `https://accounts.x.ai/oauth2/device?user_code=…`。
- 等待期间 token 端点回 400 `authorization_pending`。
- token 响应：`access_token` / `refresh_token` / `id_token` / `expires_in` / `scope`。
  - access token 6 小时（21600 秒），是 ES256 的 JWT，带 `tier`、`principal_type`、`team_id`。
  - ID token 也是 ES256；JWKS 里两把 P-256 公钥。Dock 先验签名再看声明。
  - scope 给全了：`openid profile email offline_access grok-cli:access api:access`。
- 刷新：refresh token 会轮换，刷新响应**不带** `id_token`。
  - 旧 refresh token 刷过之后仍能用（没有复用检测），Dock 照样存新的、加锁刷新。
- 撤销：`POST /oauth2/revoke` 回空的 200；之后再刷 400 `invalid_grant`。

推理：

- `GET /v1/models` 12 条，混着 `grok-imagine-*` 图像 / 视频模型，带 `context_length`。
- `GET /v1/language-models` 7 条语言模型，带 `input_modalities`。
  - Dock 以它为准列模型，上下文长度从 `/v1/models` 补。
- `POST /v1/responses` 与 `/v1/chat/completions` 都通；默认 `store: true`。
- 响应里有 `cost_in_usd_ticks`（`/usage` 里显示的就是它，是订阅额度的折算，不另外扣钱）。
- 响应头有限流：`x-ratelimit-limit-requests: 480`、`x-ratelimit-limit-tokens: 10000000`。

错误形状（`{"code": "...", "error": "说明"}`）：

| 情形 | 上游 | Dock |
|---|---|---|
| token 签名被改坏 | 403 `unauthenticated:bad-credentials` | 当 token 失效：刷新后重发一次 |
| token 被截断 | 400 `invalid-argument`（Incorrect API key） | 原样报 |
| 模型不存在 | 404 `not-found` | 原样报 |
| 模型不收 `reasoning.effort` | 400 `…does not support parameter reasoningEffort` | 去掉 `reasoning.effort` 重发一次，记住 |
| 不收 `effort: none` | 400 ``…does not support `reasoning_effort` value `none` `` | 同上 |
| 套餐档位不够（别人报过） | 403 `The caller does not have permission…` | 不刷新、不重发，说明可能是档位 |
| 429 | 限流或额度用完，body 分不清 | 不重试，原话带出来 |

推理参数（每个模型发一次「回一个词」）：

| 模型 | 只发 `summary` | `effort: none` | `effort: low/high` |
|---|---|---|---|
| grok-4.7 | 过 | 400 | 过 |
| grok-4.6 | 过 | 400 | 过 |
| grok-4.5 | 过 | 400 | 过 |
| grok-4.3 | 过 | 过 | 过 |
| grok-4.20-0309-reasoning | 过 | 400 | 400 |
| grok-4.20-0309-non-reasoning | 过 | 400 | 400 |

- 上游不说哪些档位可用：刷新模型时按 models.dev 的 `xai` 条目填（`reasoning_options`），和上表一致。
  - grok-4.5 / 4.6 / 4.7：low / medium / high / xhigh（没有 none，也**没有 max**）；
  - grok-4.3：none / low / medium / high / xhigh；
  - grok-4.20-0309-reasoning：会推理但没有档位（菜单不列，不发 effort）；
  - grok-4.20-0309-non-reasoning：不推理（不发任何推理字段）。
- 订阅模型的菜单里只有列表里有 none 的才给「关」（不收 none 的给了也关不掉）。
- 目录拉不到（离线）就留空，用通用档位；Dock 不另外维护一张表。
- 不管从哪来，`/think off` 或 `/effort` 撞上 400 时，Dock 去掉 `reasoning.effort` 再发一次，这个会话里记住。
- `reasoning: {}`（去掉 effort 之后剩下的）各模型都收。

前缀缓存（约 6–9k token 的前缀，每步追加上一步的输出与一条新消息；不算冷启动那一步）：

| 发法 | 后续请求命中 |
|---|---|
| 探针，带 `prompt_cache_key`（grok-4.6 / 4.7，4 组） | 18/22 |
| 探针，带 key + `x-grok-conv-id` 头 | 6/7（与只带 key 一样） |
| 探针，不带 | 1/3，另两步只中 512 |
| 真 Dock（隔离的 `dock serve`，真系统提示与工具表，两次） | 9/12 |

- 命中的那一步一般占输入的 95% 以上；没中的只剩 512 / 1152 左右。
- 没中的几次前缀都是纯追加（`DOCK_CACHE_DEBUG` 里「同前 N/N」），参数没变也会碰上：上游换了机器。
  - 换推理参数（`summary` ↔ `{}`）不见得不中：探针里切过去那一步中了、切回来那一步没中。
- 再加 `x-grok-conv-id` 头没有帮助，Dock 不带。
- xAI 文档：`prompt_cache_key` 等同于 Chat Completions 的 `x-grok-conv-id` 头。
- Dock 取会话 id（子代理取它自己的 agent id），见 `grok_policy()`。
- 命中时那一步的折算费用约降到三分之一。

真 Dock 端到端（隔离的 `DOCK_HOME` + `dock serve --origin` + Node WS 驱动）：

- `auth/login { provider: "grok" }` → 授权页地址 → 同意后 `auth/status` 里 `planUsage: true`。
- `auth/models` 回 7 条 `grok/*`，上下文长度 1000000（grok-4.20）等。
- 三轮对话：`list_dir` + 三次 `read_file`、推理摘要、回答都对。
- 第三轮先 `/think off`：Dock 发 `effort: none` 被拒，去掉后重发成功。
- 全新的 `DOCK_HOME`（只有凭据、没有任何 config）：同一个进程里直接用上，不用重启。

## 还没做 / 没测

- 订阅额度池算不算 `api.x.ai` 的用量（见「订阅额度」）。
- 档位不够的 403 只见别人报过，本机账号没撞上。
- access token 过期后上游回什么（6 小时才过期）：推测同是 403 `unauthenticated:*`，Dock 会先刷新。
- WebSocket 传输（OpenCode 默认走 `wss://api.x.ai/v1/responses`）：Dock 只有 HTTP/SSE。
