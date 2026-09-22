---
name: dock-guide
description: Dock 的日常使用指南：斜杠命令速查、快捷键、权限模式、会话管理、任务与子代理面板、技能与 workflow 的调用方式、浏览器与桌面控制。用户问「怎么用 dock / 有什么命令 / 这个功能怎么开 / 怎么恢复会话 / 怎么看用量」时用本技能回答；也可以在用户不熟悉 dock 时主动按它引导操作。
---

# Dock 使用指南

Dock 是跑在终端里的编码 agent。底部输入框发消息，`/` 唤出命令下拉，
`/help` 看全部命令，`Ctrl+X` 看快捷键。

## 快捷键

| 键 | 作用 |
|---|---|
| `Enter` | 发送；下拉打开时先补全 |
| `Shift+Tab` | 切换权限模式（询问 / 始终允许） |
| `Ctrl+W` 或 `/new` | 新会话（归档当前会话，用量清零） |
| `F3` 或 `/resume` | 恢复本工作区的历史会话 |
| `Ctrl+Q` | 退出 |
| `Esc` | 停止当前回答 / 关闭 overlay / 放弃审批 |
| `↑` / `↓` | 输入历史；`/history` 搜索提示词 |
| `Tab` | 下拉补全（命令名、参数都支持） |

进程入口：`dock --resume` 启动即恢复最近会话，`--resume <id>` 指定 id。

## 权限模式

默认「询问」：模型执行 bash / kill_task 等敏感操作前弹批准框（允许一次 /
始终允许 / 拒绝）。`Shift+Tab` 切到「始终允许」后不再逐条询问。计划模式
（`/plan`）是独立开关，与权限模式正交：模型先出计划，你批准后才动手。

## 会话与上下文

- 会话落盘在 `$DOCK_HOME/sessions/<工作区>/<id>/`，跨重启可恢复。
- `/compact [说明]` 手动压缩历史；上下文用到窗口 85% 时自动压缩。
- `/context` 看占用明细（系统提示 / 消息 / 工具定义 / 技能 / 工作流 / MCP）。
- `/usage` 看本会话 token 用量与 API 耗时（费用仅在上游上报时显示）。
- `/export [path]` 导出对话。

## 模型

`/model` 切换模型；模型目录在 config.toml 的 `[models]` /
`[model.<id>]`，配法见 `dock-config` 技能。`/effort` 调推理强度，
`/timestamps` 开关时间戳，`/theme` 换配色。

## 任务与子代理

- 模型可以用 `task` 工具把活派给子代理（后台跑，完成后主动汇报）。
- `/tasks` 打开分组面板（Workflows → Subagents → Tasks → Watchers）；
  回车或点击子代理行进入其对话视图，底部输入框可以直接给它发消息。
- `/goal <目标>` 设一个持续目标，模型会跨多轮推进直到完成；
  `/goal status` 查看进度，`/goal pause` / `/goal clear` 暂停或清除。
- `/loop [间隔] <提问>` 建定时任务，`/tasks` 的 Watchers 里关闭。

## 技能与 workflow

- `/skills` 列出已发现技能；`/<技能名> [参数]` 直接调用（如
  `/dock-config`）；模型也会按任务自动调用匹配的技能。
- 想装新技能：把「目录 + SKILL.md」放进 `{cwd}/.dock/skills/`（项目）、
  `~/.dock/skills/`（全局）或 `{cwd}/skills/`。
- `/<工作流名> [参数]` 运行 workflow 脚本（放 `.dock/workflows/` 或
  `~/.dock/workflows/`）；`/deep-research <查询>` 是内置的深度调研。

## 浏览器与桌面控制

- `/browser` 打开浏览器驾驶舱（需 Chromium）：开关有头/无头、标签页、
  截图、网络与控制台。让模型操作网页用 `browser_*` 工具。
- `/computer` 查看本机桌面控制（cua-driver MCP）状态。
- `/pair` 开启回环网关，配对后在浏览器里用 companion 页面联动。

## MCP 与 LSP

- `/mcps` 管理 MCP 服务器：启停（写回 config.toml）、OAuth 认证、
  逐工具开关。
- `/lsp setup` 自动探测并配置语言服务器（rust-analyzer、tsserver、
  gopls、pyright），写入项目 `.dock/lsp.json`。

## 计划与插件

- `/plan [说明]` 进入计划模式；`/view-plan` 查看本页计划文件。
- `/cordis` 查看已挂载的插件（项目 `.dock/plugins/` 与会话内存插件）。
