---
name: cordis-plugin-development
description: 在 Dock 里写、改、调试插件（Rhai）。插件最大的用处是改 agent 本身的行为——拦下或要确认某些工具调用、没满足完成标准不许收尾、给每一轮挂一张带按钮的卡片、定时检查外部状态并叫醒 agent、统计成本和缓存命中——这些 MCP 做不到；其次才是把 HTTP API 包成工具、加斜杠命令、面板、状态栏、设置卡。插件就是 .dock/plugins/<id>/ 目录，写好用 cordis_plugin reload 热装。用户说「以后每次…」「不许…」「总是要…」「改完必须…」「写个插件 / 加个 /命令 / 加个面板」、提到 @<插件 id>，或要用 cordis_plugin / cordis_inspect 时用它。已有工具能直接做的事不要包插件。
metadata:
  short-description: "写一个 Dock 插件"
---

# 写 Dock 插件

本文件只放流程和路由。细节在 `references/`，例子在 `examples/`，**用到哪块再读哪块**。

## 0. 先选对载体

| 用户要的 | 用什么 |
| --- | --- |
| 「以后 / 每次 / 不许 / 必须」：要 agent **一定**照做的规矩 | **行为插件**（钩子）。写进记忆或提示词只是建议，模型可能不照做 |
| 用户已经有 Claude Code / Codex 的 hooks 配置 | 不用写插件：在 `~/.dock/config.toml` 写 `[hooks] claude_code = true`（或 `codex = true`），`/hooks` 看生效了哪些 |
| 读写文件、跑命令、抓网页 | 现成工具（`bash` / `read_file` / `web_fetch`），不要包插件 |
| 一套可复用的做事说明 | 技能（`.dock/skills/<name>/SKILL.md`） |
| 多个子代理按阶段编排 | 工作流，加载 `create-workflow` 技能 |
| 换人设、工具名单、子代理名册 | 预设 YAML，见 `dock-config` 技能「Agent 预设」 |
| 别的语言写的重能力、要给别的客户端也用 | MCP 服务器（`dock-config` 技能） |
| 新模型工具 / 斜杠 / 面板 / 状态栏 / 设置卡 | 插件，本技能 |

## 插件能做、MCP 做不到的

MCP 要等模型来调；插件包在模型外面，每一次调用、每一轮都会经过它：

| 想要 | 怎么做 | 例子 |
| --- | --- | --- |
| 规矩变成保证 | `tools/pre-execute` 返回 `deny` / `ask` | `examples/guard.rhai` |
| 完成标准 | `agent/turn-end` 返回字符串 = 不许收尾，接着干 | `examples/done-check.rhai` |
| 每一轮下面挂卡片和按钮 | `host.card`（`ui.md`） | `examples/done-check.rhai` |
| 外部事件叫醒 agent | `host.every` + `http_request` + `host.followup`（`host-api.md`） | — |
| 省 token / 保缓存 | `tools/execute` 改写结果 | `examples/trim-output.rhai` |
| 成本、缓存命中、每一步复盘 | `session/entry` 的 `assistant` 条目带用量（`hooks.md`） | — |
| 让模型复审 / 分类 | `host.llm` | — |
| 跨重启的计数和状态 | `host.load` / `host.save` | — |

## 1. 插件就是一个目录

```
.dock/plugins/<id>/source.rhai    项目级（覆盖同名的用户级）
.dock/plugins/<id>/plugin.toml    可选：名字、用途、开关、免确认的 HTTP host
~/.dock/plugins/<id>/…            用户级，所有项目都加载
```

- 目录名就是插件 id（2–32 位，小写字母开头，`a-z0-9-`）。一个用途一个目录。
- 写在这里就是永久的：启动时自动装上，所有会话可见。
- 没有「会话插件」「版本号」要管：改文件、reload，就这样。

## 2. 标准流程

1. `cordis_inspect`（不带参数）：看现在有哪些插件、各自状态。
   改已有插件：`cordis_inspect` 带 `plugin: "<id>"`，再 `read_file` 它的 `source.rhai`。
2. 写文件：从 `examples/` 里挑最近的一份抄进 `.dock/plugins/<id>/source.rhai`。
   要包 HTTP API 就在 `plugin.toml` 写 `[permissions] http = ["api.example.com"]`。
3. `cordis_plugin` `action: "reload"`、`id: "<id>"`：编译并跑起来。
   走和 `bash` 一样的确认；用户拒了就别再要。
4. 看回执：`登记：` 那行列着装上的工具 / 斜杠 / 面板 / 钩子；`下一步：` 告诉你怎么验证。
5. 验证：用 `use_tool` 调刚登记的工具（`tool_name` + `tool_input`）；钩子就直接触发一次
   （比如调被拦的那个工具）。
6. 出错：回执里有原因和行号。**改同一个目录的文件，再 reload**。新版没起来时上一版
   照跑，用户手上的功能不会断。不要另起一个目录。

## 3. 两颗工具

| 工具 | 干什么 |
| --- | --- |
| `cordis_inspect` | 只读。不带参数 = 插件概览；`plugin` = 一个插件（目录、登记了什么、最近的错误）；`what` = 参考段（`services` / `tools` / `builtins` / `events` / `slots`） |
| `cordis_plugin` | `reload`：读目录、编译、装上（替换正在跑的那版）；`disable`：停掉并写 `enabled = false`，文件留着 |

## 4. 按需读参考

先读要用的那一份，别全读。路径可以直接给 `read_file`。

| 要做的事 | 读 |
| --- | --- |
| 脚本外形、`plugin.toml`、`inject`、状态放哪 | `$SKILL_DIR/references/package.md` |
| `host.register_tool` / `call_tool` / `secret`、`followup`、定时器、`llm`、存储 | `$SKILL_DIR/references/host-api.md` |
| 斜杠、面板（slot）、视图树、工具卡、设置卡、状态栏、回答下面的卡片 | `$SKILL_DIR/references/ui.md` |
| 回合开始 / 结束插提醒、观察会话（`session/entry`）、拦 / 要确认工具调用 | `$SKILL_DIR/references/hooks.md` |
| 调 HTTP API、上传文件、签名、正则、时间 | `$SKILL_DIR/references/http.md` |
| reload / disable / 回滚 / `@<id>` | `$SKILL_DIR/references/lifecycle.md` |
| Rhai 语法报错 / `Function not found` | `$SKILL_DIR/references/rhai-pitfalls.md` |
| 其它报错对照 | `$SKILL_DIR/references/troubleshooting.md` |

例子（拷进插件目录就能 reload）：

| 文件 | 演示 |
| --- | --- |
| `$SKILL_DIR/examples/plugin.toml` | 插件目录的元信息与 HTTP 声明 |
| `$SKILL_DIR/examples/ping.rhai` | 最小的一颗工具 |
| `$SKILL_DIR/examples/memo.rhai` | 状态 + 两颗工具 + 斜杠 + 面板 |
| `$SKILL_DIR/examples/api-tool.rhai` | HTTP API + 设置卡里的密钥 + 缺密钥时的提示 |
| `$SKILL_DIR/examples/guard.rhai` | 项目规约守门：不可回滚的命令拒、冻结目录不许改、`git push` 要点头 |
| `$SKILL_DIR/examples/done-check.rhai` | 完成标准：改了 `.rs` 没跑测试不许收尾 + 「跑测试」卡片 |
| `$SKILL_DIR/examples/trim-output.rhai` | 去掉 ANSI 颜色码、超长输出只留头尾 |

## 5. 交付规矩

- 先交付一个能跑的小版本，装上、验证，再按用户反馈改。不做猜测性的变体和样品页。
- 用真能力验证：装上 ≠ 能用。工具要真调一次，面板请用户打开看一眼。
- 密钥不写进源码：用设置卡的 `secret` 字段或 `host.secret`，缺了就**返回**一句告诉
  用户去哪填，别抛错让自己反复重试。
- Rhai 不是 JS：`try` 只能当语句、`fn` 只能写在顶层且不捕获外层变量、函数参数是
  值拷贝、没有 `null` / `slice`。报 `Function not found` 先读 `rhai-pitfalls.md`。
- 脚本只能用 `tools` / `slash` / `tui.slots` 等几个服务；`inject` 一个名字只是等它
  挂上，不等于能调它。见 `package.md`。

## 6. 汇报

告诉用户：插件 id 和目录、登记了哪些工具 / 命令 / 面板 / 钩子、实际验证到哪一步、
没验证到的是什么、要用户自己填的配置（比如密钥在设置页哪张卡）。
