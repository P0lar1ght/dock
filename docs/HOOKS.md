# hooks 桥接（Claude Code / Codex 格式）

已经写好的 Claude Code / Codex command hook 拿来就能在 Dock 里跑，不用学 Rhai。
实现在 `cordis-spine/src/host/hooks_bridge/`（插件 `hooks-bridge` + `/hooks` 命令
`command-hooks`），语义照 DSH 的 `hooks-claude-code` / `hooks-codex` 对齐。要 Dock
独有的东西（卡片、定时器、叫醒 agent、问模型）就写 Rhai 插件，见技能
`cordis-plugin-development`。

## 读哪些文件

| 文件 | 格式 | 什么时候读 |
| --- | --- | --- |
| `~/.dock/hooks.json` | Dock | 一直 |
| `<项目>/.dock/hooks.json` | Dock | 一直（第一次跑之前要点头） |
| `~/.claude/settings.json` 的 `hooks` | Claude Code | `[hooks] claude_code = true` |
| `<项目>/.claude/settings.json`、`settings.local.json` | Claude Code | 同上（第一次跑之前要点头） |
| `~/.codex/hooks.json` | Codex | `[hooks] codex = true` |
| `<项目>/.codex/hooks.json` | Codex | 同上（第一次跑之前要点头） |

- 开关只认**用户级** `~/.dock/config.toml`：项目配置能打开的话，克隆来的仓库就能让自己
  的 hooks 在本机跑起来。
- `<项目>` 是这一页会话的工作目录。文件每次触发时现读，改了马上生效。
- 三家外形一样：`{ "hooks": { "<事件>": [ { "matcher": "…", "hooks": [ { "type":
  "command", "command": "…", "timeout": 秒 } ] } ] } }`。Dock 的 `hooks.json` 也可以
  直接是事件表。
- 只跑 `command` 类型；`http` / `prompt` / `agent` 和 `async: true` 跳过，`/hooks` 里列出来。

## 项目文件要点头

项目里的 hooks 文件就是一串会在本机跑的 shell 命令。第一次有命令要跑时，在这一页弹确认
（列出要跑的命令），**自动批准和「始终允许」都不算数**。点了头按「路径 + 内容指纹」记在
`$DOCK_HOME/hooks-trust.json`，文件内容一改就重新问；这个进程里拒过的不再问。用户目录下
的文件不问。确认框里每条命令后面写着它的超时（没写 `timeout` 的是默认 600 秒），一条卡住会
挡这一轮那么久。项目自带的插件（`.dock/plugins/<id>/`）同样要先点头，见
[`docs/tools/cordis.md`](tools/cordis.md)。

## 时机

| hook | Dock 的时机 | matcher 对着什么 | 能做什么 |
| --- | --- | --- | --- |
| `SessionStart` | 一个会话第一次采样前（`agent/step-start`） | `startup` / `resume` | stdout / `additionalContext` 作为上下文送进去 |
| `UserPromptSubmit` | 用户消息进来、第一次采样前（`agent/pre-step`） | 不看 | 退出码 2 / `decision: block` 拦下这一轮；stdout / `additionalContext` 作为上下文 |
| `PreToolUse` | 工具跑之前（`tools/pre-execute`） | 工具名 | 退出码 2 / `deny` 拒绝；`ask` 要用户点头（自动批准下也问） |
| `PostToolUse` | 工具跑完（`tools/execute`） | 工具名 | 退出码 2 / `block` 的理由和 `additionalContext` 补在结果后面 |
| `Stop` / `SubagentStop` | 回合想结束时（`agent/turn-end`） | 不看 | 退出码 2 / `block` 让它接着干，理由给模型看 |

- **工具名**两种都认：Claude 的叫法（`Bash` `Read` `Write` `Edit` `Glob` `Grep` `LS`
  `WebFetch` `WebSearch` `Task` `TodoWrite`，MCP 工具 `mcp__<server>__<tool>`）和 Dock 的
  （`bash` `read_file` …）。`tool_input` 里照 Claude 的键名补一份（`file_path`、`content`）。
- **matcher**：Claude Code / Dock 是整串匹配（`Edit|Write`、`mcp__github__.*`），Codex 是不
  锚定的正则。空或 `*` = 全部。
- **合并**：同一个事件上的命令按配置顺序一条条跑，结论取最严（deny > ask > allow），上下文
  按顺序攒。`allow` 不会绕过 Dock 的权限门——hook 只能收紧，不能放宽。
- **Stop 防转圈**：stdin 里有 `stop_hook_active`（上一次就是被 Stop hook 拦回去的）；
  hook 自己该据此放行。宿主兜底：一轮最多续 8 次。

## stdin 与环境

```json
{ "session_id": "…", "transcript_path": "", "cwd": "<项目>", "hook_event_name": "PreToolUse",
  "tool_name": "Bash", "tool_input": { "command": "…" } }
```

`PostToolUse` 另有 `tool_response: { output, is_error }`，`UserPromptSubmit` 有 `prompt`，
`Stop` 有 `stop_hook_active` 和 `last_assistant_message`，`SessionStart` 有 `source`。
`transcript_path` 是空串（Dock 的会话日志不是 Claude 的 transcript 格式，DSH 也这样）。
环境变量 `CLAUDE_PROJECT_DIR` / `DOCK_PROJECT_DIR` 都是项目目录，进程在项目目录里跑。

`timeout` 支持小数秒，最多 86400 秒（24 小时，避免阻塞一轮无限等待），最小可表示值为
1 纳秒。正数超限或换算后为 0 时，只跳过那条命令并记警告，不回退到默认值执行。
未填、非数字或非正数仍沿用默认 600 秒。

## 失败

- 退出码 2：拦下，stderr 是理由。
- 其它非零退出码、起不来、超时（默认 10 分钟，`timeout` 改；超时连同子进程一起杀）：不拦，
  记一行 `[hooks] …` 到 stderr。hook 坏了不该让 agent 停摆。
- `{"continue": false}`：记一句，不据此停掉会话（和 DSH 一样）。
- `systemMessage`、`updatedInput`：不支持。

## 看生效了哪些

`/hooks`：每个文件、格式、命令数、信任状态、警告，以及 Claude Code / Codex 开关怎么打开。
