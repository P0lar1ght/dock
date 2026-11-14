# ChatGPT 订阅通道（Sign in with ChatGPT）

让 Dock 用**ChatGPT 套餐额度**跑推理：本地完成 OAuth，拿到短期 access token，
打 `https://api.openai.com/v1/responses`。

来源：OpenAI 给开源工具开的官方通道（设计稿核实于 2026-11-14）。官方文档在
<https://developers.openai.com/siwc/token-sharing-open-source>（sign-in /
profiles-and-sessions / models-and-inference / errors-and-recovery /
self-hosted-vms / ui-ux-guidelines 几页）。
**本仓库不碰 Codex 的内部后端，也不复用 Codex 的 client_id**——官方明令禁止
第三方这么做。

## 怎么用

**登录就有模型，不用写 config。** 登录成功、切账号、导入凭据之后，Dock 向
`/v1/models` 列一次活动账号能用的模型（官方：取 `visibility == "list"` 的、保持
上游顺序、显示 `display_name`、请求里传 `slug`；换账号就重拉），写进
`$DOCK_HOME/auth_models.json`，模型目录读的时候并进来——`/model`、桌面端模型菜单、
网关 `model/list` 都直接看得到，id 形如 `chatgpt/gpt-6-luna`：

- 上下文窗口、推理档位（`/effort` 菜单）、默认档位、能不能收图取上游实际给的
  `context_window` / `supported_reasoning_levels` / `default_reasoning_level` /
  `input_modalities`（官方文档没承诺这些字段，有就用、没有就空着）；
- 排在自己写的模型后面：没配 `[models] default` 时登录一下不会把默认模型换掉；
- 活动账号**登出了、没开套餐额度**就把那一家的模型从目录里拿掉；网络抖一下不动；
- 启动时列表缺了、是别的账号的、超过一天才在后台重拉；`/auth models` 随时手动重拉。

要改某一条的参数，在 config 里写同 id 的条目逐字段覆盖（只写一个键也行）：

```toml
[model."chatgpt/gpt-6-luna"]
context_window = 128000   # 其余照上游
```

自己手写订阅模型也照旧可以（比如给它起个短 id）：

```toml
[model."chatgpt"]
api_backends = ["responses"]
api_model = "<从 /auth models 里挑的 slug>"
auth = "chatgpt"        # 走订阅通道；不写就照旧用 api_key / env_key
# 地址由 provider 给（https://api.openai.com/v1），api_base_url 可以不写
```

```text
/auth                   # 不带参数 = 直接开始登录（打开官方授权页）
/auth login new         # 再加一个 ChatGPT 账号
/auth login <账号>      # 指定账号重新登录
/auth status            # 当前账号、套餐授权、token 剩余时间、登录进度
/auth cancel            # 中断正在等授权的登录
/auth models            # 重拉可用模型并入模型目录，列出 id
/model chatgpt/gpt-6-luna   # 切过去
```

登录在**后台**跑（`"auth"` 服务管着，**同一家同时只跑一个**：终端 `/auth` 和
桌面端按钮走同一个口子，再发起一次 = 掐掉旧的换成新的）：命令立刻返回，授权页
地址弹成一条提示（浏览器没自动打开时，复制那个地址就行）；用户同意完，终端弹
「登录成功」，失败也会弹一句。

两种鉴权**并存**：一个模型用配置好的 API 密钥，另一个走 OAuth，互不影响。
`auth` 与 `api_key` / `env_key` 同时写算配置错误（会明确报出来）。

## 开关与开关之外

- 整个通道：`DOCK_CHATGPT_SUBSCRIPTION=off`（`0` / `false` / `no` 也认）→ provider
  **根本不注册**：写了 `auth = "chatgpt"` 的模型会明确报错「订阅通道被开关关掉了」，
  没写 `auth` 的老配置一点不受影响。
- 通道的请求体约束**不给逐模型关掉**：官方要求这条通道上每个请求都
  `store: false` + `stream: true`，关掉约束发出去只会是 400。早先的
  `plan_usage` 键已删。

## 落盘

```text
$DOCK_HOME/host_id                                urn:uuid:…，本机一份，永不随凭据复制
$DOCK_HOME/credentials/chatgpt/<client_id>.json   0600，目录 0700
$DOCK_HOME/credentials/chatgpt/<client_id>.lock   刷新 / 登出的跨进程锁
$DOCK_HOME/credentials/chatgpt/active             当前活动账号的 client_id
```

写入走「同目录临时文件（创建即 0600）→ fsync → rename」；**读到权限不是 0600
直接报错**，不静默修。刷新全程持跨进程锁：refresh token 每次刷新都轮换（旧的立刻
作废），两个进程同时刷有一方会撞上 `refresh_token_reused`。刷新响应万一没带新的
refresh token（RFC 6749 允许），沿用旧的。

**登出只清 token，注册留着**（官方：Revoking this session does not delete the
registered client；保留 account/client 对应与本机 host id）：`<client_id>.json`
还在，只是 token 清空了，下次 `/auth login` 直接用它的正式 client id 重新授权，
不再走首登。刷新被判死（`invalid_grant` / `refresh_token_reused` /
`refresh_token_expired`…）也是这样处理；网络抖一下不清。

活动账号**每次读盘**，不缓存在进程里：另一个 Dock 进程登录 / 登出 / 切账号，
这边不重启就看得到。

## VPS / 远端机器

官方那套：本地登录 → 把凭据文件传过去 → 远端 `/auth import` 导入。
**远端用它自己的 host_id 接管刷新**（导入不动本机 host_id）；
两边都刷会撞上 refresh token 轮换，所以传过去之后本地那份要停用。

## 通道的硬约束

SIWC 那条通道比普通 OpenAI 兼容端点严，多一个字段就是 400：

- `store: false`、`stream: true`。
- system 提示走 `instructions`，`input` 里不许有 `role: "system"` 消息。
- 不许发这 15 个字段：`background` `conversation` `max_output_tokens`
  `max_tool_calls` `metadata` `moderation` `multi_agent` `prompt`
  `prompt_cache_retention` `safety_identifier` `temperature` `top_logprobs`
  `top_p` `truncation` `user`；也不能用 `previous_response_id`。
- 托管工具只剩 `web_search`。

这些写成 `WirePolicy` 数据（`cordis-spine/src/auth/mod.rs`），由
`cordis-spine/src/llm/http/responses.rs::apply_policy` 统一改写。
**没写 `auth` 的模型请求体与以前逐字节相同。**

## 错误分类

按官方「Errors and recovery」那一页。推理路径上：**401** 先问 provider「换一份
还是放弃」——换到了就用新 token 重发**一次**（再 401 就是账号本身的问题，不重试；
刷新只是网络抖了就照实报，不叫用户重新登录）。三种重试各有预算，互不挤占。

| 上游 | Dock 的反应 |
|---|---|
| 401 / `…_invalid_user` | 刷新重试一次；仍被拒才让重新登录（带 request-id） |
| 403 `…_user_not_eligible` | 这个账号没资格，不重试、不循环走 OAuth |
| 403 `…_route_not_supported` | 只认 `POST /v1/responses`，不重试 |
| 403 `chatpass_v2_*` / 没有码的 403 | 说明限制（client / 授权配置、地区策略），不重试、不换计费 |
| 429 `…_usage_limit_exceeded` | **不重试**；提示可能是套餐的、也可能是应用的上限，指去 Usage 页 |
| 503 `…_usage_unavailable` / `…_user_unavailable` | 凭据留着，退避重试（有上限） |
| 400 `…_unsupported_capability` | 按 `error.param` 去掉那一项**再发**（最多 3 次）；去不掉就不重发、直接报。去掉的字段记在 `"auth"` 服务里，下一轮一开始就不发 |

## 实测清单

```bash
python3 scripts/chatgpt-siwc-check.py                    # A 段：公开端点
python3 scripts/chatgpt-siwc-check.py --token "$TOKEN"   # B 段：需要账号
```

每项只发一次短请求（回复一个词就够），请求 id 会打出来。

**A 段（已实测通过，2026-11-14）**：`issuer` / 授权端点 / 换码端点 / 撤销端点 /
PKCE 只支持 S256 / ID token 只用 RS256 / 支持 refresh_token / 公开客户端
（auth method `none`）/ JWKS 4 把 RSA-RS256 公钥 / 两条推理路由存在（无 token → 401）。

**B 段（需要账号，尚未实测）**：Free 账号能不能首登拿到
`chatgpt.tokens.use.direct`；`/v1/models` 的 slug 列表；最小请求能否拿到
`response.completed`；工具调用（namespace 里的 function）的格式；reasoning
effort / summary 收不收；refresh 轮换后旧 token 是否立即失效；撤销后刷新失败；
VPS 导入后由远端接管刷新。

**已知未知**：官方对「哪些套餐能用额度」的说法不一致（2026-11-14 查）：

- [Sign in with ChatGPT](https://learn.chatgpt.com/docs/sign-in-with-chatgpt)
  与帮助中心：**Go / Plus / Pro / Business** 可以让应用用套餐额度；Free 没列。
- [Quickstart](https://developers.openai.com/siwc/quickstart)：只写了 Plus / Pro
  （发布时的说法）；又说「plan usage is available to all open-source partners」。
- 帮助中心另有一句「Supported open-source tools remain available to all ChatGPT
  users」，**没说 Free 在开源通道里能不能用额度推理**。
- 开源通道是 `dynamic_agent_client` 自助注册，**文档里没有审核 / 进目录的步骤**；
  目录里的开源集成只有 OpenClaw / OpenCode / Pi / T3。实测的同意页写着「该智能体及其
  名称未经 OpenAI 认证。允许访问后，该智能体即可使用你的 ChatGPT 使用额度」，
  说明没进目录也能被授予额度（见下面实测）。

### 实测结论（2026-11-14，真实账号）

**已跑通（真实端点、真实账号）**：

| 环节 | 结果 |
|---|---|
| 动态注册 → 回调给正式 client id | `oaiapp_…`，回调带 `code` / `scope` / `client_id` / `state` |
| 换码 | 用正式 client id 成功；用动态 client → 400 `invalid_client` |
| ID token | **RS256 验签通过**（官方 JWKS 4 把公钥），iss / aud / exp / nonce 全对 |
| 凭据落盘 | `~/.dock/credentials/chatgpt/oaiapp_….json`（0600，目录 0700）+ `active` + `host_id` |
| 刷新 | 成功，**refresh token 轮换**（generation 1→2，新 access token 3600s） |
| 授权下来的 scope | `email offline_access openid profile`（只要身份时） |

**没跑通的**：用套餐额度推理。

| 结论 | 依据 |
|---|---|
| **这个账号带套餐 scope 时授权页过不去，多半卡在套餐** | 第二轮（正式 client id）的同意页「将 Dock 连接到 ChatGPT」列出「身份」和「ChatGPT 套餐：允许 Dock 使用你的 ChatGPT 套餐额度」，下面写「所需权限不可用。你无法继续使用此工作空间和套餐」，「继续」置灰，只给「升级订阅」。同一页底部写着「该智能体及其名称**未经 OpenAI 认证**。允许访问后，该智能体即可使用你的 ChatGPT 使用额度」——没认证的智能体也能被授予额度，所以**不是卡在目录**，是这个账号 / 工作空间的套餐。升级之后能不能过还没实测；ID token 里没有套餐字段，看不出账号是什么套餐 |
| **身份登录人人可用** | 只请求 `openid profile email offline_access`（不带套餐 scope）时，授权页正常放行 |
| 换码要用**回调给的正式 client id** | 用动态 client 换码 → 400 `invalid_client`；用正式 client 换码 → 400 `invalid_grant`；官方写明这时要**丢掉这个码、用正式 client id 重新授权一次** |
| 回调带 `code` `scope` `client_id` `state` | 没有 `id_token_hint`；正式 client id 形如 `oaiapp_…` |

所以现在的状态：**OAuth、验签、落盘、刷新整条链路可跑通（`/auth login identity`），
「用套餐额度推理」还没跑通，多半卡在这个账号的套餐**。

按官方文档「ChatGPT plan use isn't enabled」那条分支处理：换码成功但 scope 里没有
`chatgpt.tokens.use.direct` 时，**保留登录、标记套餐额度关闭**，并给用户两条路：
升级订阅，或在 `config.toml` 里配 `api_key` 走按量计费。

> **拿 Pi 对照过，分不出来**（2026-11-14，同一账号，Pi 1.5.2）：Pi 首轮也拿到了
> 回调和 code，换码同样 400 `invalid_grant`；但 Pi 不做官方要求的第二轮（它每次都用
> `dynamic_agent_client`，换码失败就报错），走不到带套餐那一页。Dock 的首轮
> 同样拿到 code、同样被拒，挡人的是**第二轮**那一页。所以拿 Pi 判断套餐够不够不成立，
> 判断依据是上表那页的原文。
>
> 被挡时用户只能点「取消」→ 回调 `error=access_denied`。Dock 按官方「ChatGPT plan
> use isn't enabled」那条处理：先校验 state，不换码，告诉用户是套餐的事，给出升级套餐 /
> `/auth login identity` / 配 `api_key` 三条路（浏览器失败页上也是这段话）。

### 请求这一段：靠回归测试钉住（`cordis-spine/src/llm/http/wire_test.rs`）

真账号验不到请求（上面那两个阻塞点），所以请求这一段由**本地假上游**钉死，
下面这些用例（`cargo test -p cordis-spine --lib wire_test`）：

| 用例 | 钉住的行为 |
|---|---|
| 订阅那条带 lease 的 bearer 并按策略改请求体 | `Authorization: Bearer <lease 的 token>`；`store:false`；`stream:true`；system 走 `instructions`；`input` 里不留 system 消息；官方不收的 15 个字段一个都不发 |
| 遇到 unsupported_capability 去掉字段再发 | 第二次请求体里没有 `error.param` 那一项、与第一次不同；下一轮一开始就不发 |
| 去不掉的字段不重发 | 请求体里本来就没有 → 只发一次，直接报 |
| 密钥那条带自己的 key 且请求体不动 | `Authorization: Bearer <配置里的 key>`；请求体**不**多出 `store` / `instructions`，system 消息原样留在 `input` 里 |
| 两个模型并存时各自带各自的鉴权 | 同一进程里两个模型各带各的，不串 |
| 订阅和密钥同时写是配置错误 | 同一模型同时写 `auth` 与 `api_key` → 配置错误，不静默挑一个 |
| 只有身份 scope 的凭据不给 lease | 没套餐额度 → `missing_plan_usage_scope`，不许推理 |
| 带套餐 scope 的凭据给出 lease | 给的正是落盘那个 access token |

「没走订阅通道的模型请求体逐字节不变」这条还有一条更早的回归：
`cordis-spine/src/llm/http/responses.rs` 里的 `default_policy_leaves_the_body_untouched`。

## 界面与入口

**终端**：`/auth`（login / logout / cancel / status / list / use / models / import）。
命令体在 spine 里，终端与桌面端跑的是同一个；它只经 `"auth"` 服务和 provider
trait 说话，不认识哪一家的凭据文件。

**桌面端**（文案按官方 UI 规范）：

- 输入框旁：当前模型走订阅通道时标「Using ChatGPT plan」，点它打开用量页。
- 设置 › 模型 ›「订阅通道」：各 provider 的账号（套餐额度开没开、已登出、哪个是活动
  账号）、登录进度（等授权 / 失败原因 / 取消）、「Continue with ChatGPT」「添加账号」
  「Manage usage」与逐账号的「登出」。连的是远程 Dock 时登录按钮置灰并提示走
  `/auth import`（官方：127.0.0.1 回调落在开浏览器的那台机器上）。
- `/auth` 也在桌面端的斜杠下拉里（命令表是同一份）。

**浏览器回调页**（`cordis-spine/src/auth/callback_page.{rs,html}`，MCP 登录共用）：
收到回调先发前半页「正在连接 ChatGPT…」，连接不关；换码有了结果再补一段
`settle(结果)` 把页面切过去——连上了（邮箱、套餐额度开没开、Manage usage）/
还差一步（首登换码被拒，浏览器马上开第二轮）/ 没连上（一句原因 + 接下来可以
怎么做）。登录被取消、出错时连接直接关，页面显示「这次登录中断了」。样式取自
dock-gui（玻璃卡片、品牌标描线、跟随系统深浅色）；页面只用 `textContent` 填字、
`Referrer-Policy: no-referrer`（地址里有授权码），并把地址栏里的 code / state 抹掉。

协议侧：`model/list` 与线程环境投影的模型项带 `authProvider`；`auth/status` 给
provider（`displayName` / `usageUrl`）、账号表（`signedIn` / `planUsage`，**不含任何
token**）和最近一次登录（`login`）；`auth/login { provider, scope?, account? }` /
`auth/logout` / `auth/cancel` 驱动登录登出；变化推 `auth/changed { provider }`。
`auth/*` **全部只认受信连接**（`auth/status` 里有账号邮箱）。
