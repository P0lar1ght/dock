---
name: cordis-plugin-development
description: 在 Dock 里写、改、调试动态 Cordis 插件（Rhai）：把一个 HTTP API 包成模型工具、加斜杠命令、面板 / 状态栏 / 设置卡、在回合开始或结束时插一句提醒，会话里热挂试用，再存成 .dock/plugins 下的永久插件。用户说「写个插件 / 把这个 API 做成工具 / 加个 /命令 / 加个面板」、提到 @pluginId，或要调 cordis_define / cordis_run 这组工具时用它。已有工具能直接做的事不要包插件。
metadata:
  short-description: "写一个动态 Cordis 插件"
---

# 写动态 Cordis 插件

本文件只放流程和路由。细节在 `references/`，例子在 `examples/`，**用到哪块再读哪块**。

## 0. 先选对载体

| 用户要的 | 用什么 |
| --- | --- |
| 读写文件、跑命令、抓网页 | 现成工具（`bash` / `read_file` / `web_fetch`），不要包插件 |
| 一套可复用的做事说明 | 技能（`.dock/skills/<name>/SKILL.md`） |
| 多个子代理按阶段编排 | 工作流，加载 `create-workflow` 技能 |
| 换人设、工具名单、子代理名册 | 预设 YAML，见 `dock-config` 技能「Agent 预设」 |
| 新模型工具 / 斜杠 / 面板 / 状态栏 / 设置卡 / 回合钩子 | **动态插件，本技能** |

## 1. 三个概念

- **Plugin**：稳定 id（`demo-1`），一个用途一个。
- **Package**：Plugin 下不可改的一个版本（`pkg-3`）。改任何东西都是新 Package。
- **Run**：某个 Package 挂在宿主上跑。`define` 只编译，不跑。

## 2. 标准流程（文件优先）

1. `cordis_inspect`：看现在挂着什么。
   改已有插件先 `cordis_inspect` 带 `pluginId`（+ `packageId`）。
2. 写文件：`.dock/plugins/<id>/plugin.toml` + `source.rhai`。
   从 `examples/` 里挑最近的一份抄起。
3. `cordis_define`：`factory: "rhai"` + `source_path`。
   不要把大段 Rhai 塞进 `source`。
4. `cordis_run`：`mode: "run"`。走权限确认，用户拒了就别再要。
5. `cordis_call`：同一轮里直接试调注册的工具。
6. 出错：改文件 → `cordis_define` 同一个 `pluginId`（`kind: "existing"`）
   → `cordis_run` `mode: "update"` → 再 `cordis_call`。
   **不要另开一个 Plugin**。
7. 收尾：写在 `.dock/plugins/<id>/` 下的文件下次启动自动加载；
   只在会话里试的用 `cordis_promote` 落盘。

## 3. 六颗工具

| 工具 | 干什么 |
| --- | --- |
| `cordis_inspect` | 看目录（`what`）或某个 Plugin（`pluginId`） |
| `cordis_define` | 登记一个新 Package；只编译 |
| `cordis_run` | 挂上某个 Package：`run` 首启 / 重启 / 回滚，`update` 换版本 |
| `cordis_call` | 同一轮试调任意工具（含插件刚注册的） |
| `cordis_stop` | 停掉；`drop: true` 连内存里的定义一起丢。都不删盘上文件 |
| `cordis_promote` | 把会话里的 Package 写成永久插件 |

## 4. 按需读参考

先读要用的那一份，别全读。路径可以直接给 `read_file`。

| 要做的事 | 读 |
| --- | --- |
| 脚本外形、`inject`、`plugin.toml`、状态放哪 | `$SKILL_DIR/references/package.md` |
| `host.register_tool` / `call_tool` / `provide` / `secret` | `$SKILL_DIR/references/host-api.md` |
| 斜杠、面板（slot）、视图树、工具卡、设置卡、状态栏 | `$SKILL_DIR/references/ui.md` |
| 回合开始 / 结束插提醒、观察会话 | `$SKILL_DIR/references/hooks.md` |
| 调 HTTP API、上传文件、签名、正则、时间 | `$SKILL_DIR/references/http.md` |
| 版本、`run` 与 `update`、回滚、`@pluginId`、落盘 | `$SKILL_DIR/references/lifecycle.md` |
| Rhai 语法报错 / `Function not found` | `$SKILL_DIR/references/rhai-pitfalls.md` |
| 其它报错对照 | `$SKILL_DIR/references/troubleshooting.md` |

例子（都能直接 define + run）：

| 文件 | 演示 |
| --- | --- |
| `$SKILL_DIR/examples/plugin.toml` | 插件目录的元信息 |
| `$SKILL_DIR/examples/ping.rhai` | 最小的一颗工具 |
| `$SKILL_DIR/examples/memo.rhai` | 状态 + 两颗工具 + 斜杠 + 面板 |
| `$SKILL_DIR/examples/api-tool.rhai` | HTTP API + 设置卡里的密钥 + 缺密钥时的提示 |

## 5. 最常踩的坑

- `define` 成功不等于在跑；必须 `cordis_run`。
- 修复时换了 `idPrefix` 会多出一颗重复插件；用 `kind: "existing"`。
- 已有当前版本时换版本要 `mode: "update"`，不是 `run`。
- 密钥不要写进源码：用设置卡的 `secret` 字段或 `host.secret`，
  缺了就返回一句告诉用户去哪填，别反复 `cordis_call` 撞同一个错。
- Rhai 不是 JS：`try` 只能当语句、`fn` 只能写在顶层且不捕获外层变量、
  没有 `null` / `is_string`。见 `rhai-pitfalls.md`。
- 脚本只能用 `tools` / `slash` / `tui.slots` 等几个服务；
  `inject` 一个名字只是等它挂上，不等于能调它。见 `package.md`。

## 6. 汇报

告诉用户：插件 id 和文件路径、注册了哪些工具 / 命令 / 面板、
`cordis_call` 实际验证到哪一步、要用户自己填的配置（比如密钥在设置页哪张卡）。
