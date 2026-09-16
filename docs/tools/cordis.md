# 动态 Cordis 插件

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `dynamic-runner`

- **ctx**：`"dynamicCordisRunner"`
- **模型工具**：—

会话注册表 + 磁盘永久插件。热挂体走 `ctx.plugin` / `fiber.dispose`。会话定义盖章 `Sessions::identity()`；磁盘插件 `session_id` 为 `*`，所有会话可见。`cordis_promote` 写 `{cwd}/.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/`（目录名即 pluginId）；`install_app` 自动加载（fail-open，不走权限 overlay）

## `tool-cordis`

- **ctx**：→ `"tools"`（inject `"dynamicCordisRunner"` + `"context"`）
- **模型工具**：`cordis_*`（按需）

预置工厂 `echo` / `note` / `hold` / `slash`，加上 `factory: "rhai"`（`source` 在 define 时 compile，run 时 eval `apply`）。`register_deferred`。系统提示只留短指针（`search_tool` 查 cordis）；教程在 `skills/cordis-plugin-development/SKILL.md`。`host.on` 可挂三个事件：`session/event`（事后观察）、`agent/step-start` 与 `agent/turn-end`（拦截，返回 `<system-reminder>` 正文或 `()`，宿主代跑 `next`、脚本吞不掉链，order 槽 50 排在内建 todo(10) / goal(20) 之后；用户已排队下一条时宿主直接不问脚本；抛错或返回非字符串都当没意见）。`cordis_call` 经 `Tools::execute` 试调任意 live 工具。`cordis_inspect` `what`: `services` / `builtins` / `events` / `slots` / `temporary` / `permanent`。`inspect_self` 对 Rhai 包回传 source。用户文本 `@pluginId` 在 `agent/pre-step` 注入身份 reminder（不含源码）。审批走权限 overlay（`cordis_run` `cordis_promote`）。`/cordis` 列出内存与磁盘层。Skill：`/cordis-plugin-development` 或 `skill` 工具加载 `skills/cordis-plugin-development/SKILL.md`
