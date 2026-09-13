---
name: dock-config
description: Dock 的配置手册：config.toml 的位置与全部常用字段（模型、MCP server、browser）、环境变量、DOCK_HOME 目录布局、技能与 workflow 与 Agent 预设的发现目录。用户想配模型 / 换 API / 接 MCP / 调 browser 显示 / 加技能或 workflow / 找会话存档，或问「某个配置在哪改、某个目录存了什么」时，用本技能回答；改配置前先读它，不要猜字段。
---

# Dock 配置手册

## 配置文件

配置按顺序合并，同键后者覆盖：

1. `$DOCK_HOME/config.toml` — 用户全局（`DOCK_HOME` 未设时是 `~/.dock`）
2. `{cwd}/.dock/config.toml` — 项目级

config.toml 可能含明文 `api_key`：不要提交进仓库，不要在回复里回显。

## 模型

```toml
[models]
default = "<model-id>"

[model."<model-id>"]
name = "显示名"
api_base_url = "https://…/v1"
api_backends = ["responses", "chat_completions"]   # 端点支持哪几条协议
api_model = "上游真实模型名"        # 省略时用 <model-id>
api_key = "…"                       # 或用 env_key 从环境变量取
env_key = "MY_API_KEY"
context_window = 128000

# 协议块：只在某条 wire 上覆盖连接（入口不同时用）
[model."<model-id>".messages]
api_base_url = "https://…/anthropic"

# 单价：USD / 百万 token，写了 /usage 才算得出金额
[model."<model-id>".pricing]
input = 0.28        # 未命中输入
cache_read = 0.028
cache_write = 0.28  # 省略则按 input
output = 0.42
```

- `api_backends` 是**一个端点支持的协议列表**，第一条 = 切到该模型时的默认。
  可选值 `responses`、`chat_completions`（OpenAI 风格）、`messages`（Anthropic
  风格）。声明多条后用 `/protocol` 现场切，不用为了换协议再配一行重复的模型。
- 单数 `api_backend = "…"` = 只支持这一条。**两个都不写默认 `responses`**，
  所以只开 /chat/completions 的端点（自建代理、本地 ollama、多数长尾模型）必须
  显式写 `api_backend = "chat_completions"`，否则会打到不存在的路径。
  两个键都写时，单数当默认提到队首。
- `[model."<id>".<协议>]` 协议块：同一端点各协议入口不同时用（DeepSeek 的
  OpenAI 侧是 `https://api.deepseek.com`，Anthropic 侧是 `…/anthropic`）。只能
  覆盖三个**连接**键：`api_base_url` / `auth_scheme` / `api_model`。优先级
  协议块 > 模型级 > 协议默认。协议名接受别名 `resp` / `chat` / `anthropic`。
  能力键（`context_window`、`reasoning*`、`supports_images`、
  `max_output_tokens`、`prompt_cache`）是模型的属性，不随协议变，只写模型级。
- `[model."<id>".pricing]` 单价，USD / 百万 token，四个价位和 `/usage` 的分段
  一一对应。**不内置厂商价格表**——价格变动频繁，猜出来的单价一旦过期就是静默
  给出错误金额。本地算出的金额一律标「约 …（按 config 单价估算）」，不是账单：
  不含分时折扣（DeepSeek off-peak 半价）。上游带了费用则以上游为准。
- `env_key` 指向环境变量名，与 `api_key` 二选一。
- `context_window` 按上游真实值填，驱动上下文占用显示与自动压缩阈值。
- `auth_scheme` 不写就跟着**当前协议**走：`messages` 用 `x-api-key`，另两条用
  Bearer。写了就对该模型的所有协议一律生效（OpenRouter 的 /messages 要显式
  写 `bearer`）。

## MCP server

```toml
[mcp_servers.<name>]
command = "…"        # stdio 方式（与 url 二选一）
args = ["…"]
url = "http://…"     # Streamable HTTP 方式
enabled = true

[mcp_servers.<name>.oauth]
scopes = ["…"]
```

会话里用 `/mcps` 查看、开关（写入 config.toml）与 OAuth 认证；HTTP 服务
器的 token 存 `~/.dock/mcp_credentials.json`。

## browser

```toml
[browser]
headed = true   # 缺省 false（无头）
```

`/browser` 驾驶舱里按 `h` 同样切换并写回 config.toml；环境变量
`DOCK_BROWSER_HEADED`（任意非空）强制有头。

## 环境变量

| 变量 | 作用 |
|---|---|
| `DOCK_HOME` | 数据根目录（缺省 `~/.dock`）；多实例 / 测试隔离用它 |
| 模型 `env_key` 指向的变量 | 该模型的 API key |
| `GROK_MAX_CONCURRENT_SUBAGENTS` | 子代理并发上限 |
| `GROK_SUBAGENT_LIMIT_BEHAVIOR` | 并发满时 `queue`（排队，缺省）或 `fail`（拒绝 spawn） |
| `DOCK_BROWSER_HEADED` | 非空即强制有头浏览器 |

## `$DOCK_HOME` 布局

```
$DOCK_HOME/
  config.toml               用户配置
  sessions/<cwd-key>/<id>/  会话存档（meta.json + chat_history.jsonl）
  skills/                   用户技能
  bundled/skills/           内置技能缓存（应用自带，可改；同名被更高层覆盖）
  workflows/                用户 workflow 脚本
  bundled/workflows/        内置 workflow 缓存
  presets/                  用户 Agent 预设
  mcp_credentials.json      MCP OAuth 凭据
```

## 技能目录（同名后者覆盖）

1. `$DOCK_HOME/bundled/skills/`（内置，最低）
2. `{cwd}/skills/`
3. `~/.dock/skills/`
4. `{cwd}/.agents/skills/`
5. `{cwd}/.dock/skills/`

技能是「目录 + `SKILL.md`」，frontmatter 支持 `name`、`description`、
`when-to-use`、`paths`、`user-invocable`、`disable-model-invocation`。
带 `paths:` 的技能渐进披露：匹配文件被触碰前不进技能列表、`skill` 工具
也拒载；用户 `/name` 不受限。`/skills` 查看当前全部技能。

## Workflow

Rhai 脚本目录（同名不覆盖已有）：内置 → `{cwd}/.dock/workflows/<name>.rhai`
→ `~/.dock/workflows/`。`/<工作流名> [参数]` 直接执行，不经模型。

## Agent 预设

预设就是「目录 + `agent.yml`」，加一个目录就多一个 Agent，不必改代码。
覆盖顺序：内置 < `~/.dock/presets/<id>/agent.yml` < 项目
`.dock/presets/<id>/agent.yml`。`/preset` 打开名册：`n` 新建、`d` 复制、
`a` 应用、`x` 删除。子代理写在预设的 `agents/<type>.yml`（人设 + 工具
允许名单），写完在会话里即刻生效。
