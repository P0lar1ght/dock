# 工具清单

对照当前 `install_app` 与 `cordis-spine` 源码，不是愿望列表。架构规则仍以 [AGENTS.md](AGENTS.md) 为准；本文件只记 **模型工具、插件粒、缺口、不要做的事**。

核对：

```bash
cargo test -p cordis-spine --test round -- install_app_registers
```

`install_fakes` 是 echo，**没有**下表能力工具。不要拿 echo 测试当产品清单。

---

## 原则

1. **一切皆插件。** 新工具是一个（或一套）Cordis 插件，`inject: ["tools"]` 后 `ctx.tools.register()`。不要焊进 `event_loop` / `agent-loop`。
2. **一张 `"tools"` 表。** DSH 不是「一个工具名一个 ExtraTools key」。Grain 是套件：`tool-web` 同时 register `web_search` + `web_fetch`；`tool-todo` 只有 `todo_write` 因为那就是整套。MCP 是 **一个插件连一个 server**，工具仍 `register` 进同一张表，公名 `{server}__{tool}`，不能盖掉 `bash`。
3. **先复制 Grok，再改成 Cordis。** 工作只在 `dock/`。不要改、不要 path-dep `grok-build/`。Grok 已有逻辑就 `cp` 再剥依赖（`register_resource!`、`tracing`、schemars、xAI 账号 client）。
4. **Live-lookup。** 调用点 `ctx.get` / `ctx.require`。不要把 `Arc<T>` 关进长生命周期闭包。
5. **扩展走 waterfall**（`tools/execute`、`system-prompt/assemble` 等），不要在 loop 里分支。
6. **MCP / 可选能力 fail-open。** 没配置或连不上时插件仍 Active，不要让 `install_app` 失败。
7. **不接 Grok 账号产品。** 登录、账单、分享、marketplace、Imagine / 视频生成、voice、dashboard 账号面：不做。核心 agent 能力（读改跑、搜网、todo、提问、计划、后台任务、调度、MCP）要补。
8. **产品面要跟上。** 新工具要有：register 进 specs（LLM 看得到）、execute 路径、权限/计划门（该挡的挡）、对应 slash / TUI（需要人看的才做）。用户可见文案中文；Grok 底栏 `Enter:send` 那种短 hint 保持英文无空格。
9. **测的是树，不是 stub。** 清单以 `install_app` 挂上的插件和 `Tools::specs()` 为准。不要留一个同名但走另一套 end state 的假实现。

---

## 已有（`install_app`）

挂载顺序见 `cordis-spine/src/bundle.rs`。能力插件都在 `workspace_tools` 之后、`llm` 之前。

| 插件 | ctx | 模型工具 | 备注 |
|---|---|---|---|
| `tools`（`workspace_tools`） | `"tools"` | `list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file` | 工作区内建，不能被 register 盖掉 |
| `jobs` | `"jobs"` | — | 后台进程表；bash `is_background` / `block_until_ms: 0` 用它 |
| `tool-web` | → `"tools"` | `web_fetch` `web_search` | Grok SSRF / 同 host 重定向 / htmd。`web_search` 无 xAI 账号，走同一套 fetch 打公开 HTML 索引 |
| `tool-todo` | `"todos"` + `"tools"` | `todo_write` | Grok merge/replace |
| `plan-mode` | `"planMode"` + `"tools"` | `enter_plan_mode` `exit_plan_mode` | 计划文件 `.dock/plan.md`。计划态挡住 bash / 写文件等 |
| `tool-ask-user` | `"ask"` + `"tools"` | `ask_user_question` | 事件 `ask/pending`；TUI overlay |
| `tool-jobs` | → `"tools"` | `get_task_output` `wait_tasks` `kill_task` | 查/等/杀后台 bash |
| `tool-scheduler` | → `"tools"`（live `"cron"`） | `scheduler_create` `scheduler_list` `scheduler_delete` | 包着已有 `"cron"`；`/loop` 仍直接加 cron |
| `tool-task` | `"subagents"` + `"tools"` | `task` | 子代理；isolate `"sessions"`+`"turn"` |
| `tool-memory` | `"memory"` + `"tools"` | `memory_search` `memory_get` | 本地 `~/.dock/memory` / `.dock/memory` |
| `tool-monitor` | → `"tools"`（live `"jobs"`） | `monitor` | 长命令 stdout 盯梢 |
| `tool-goal` | `"goal"` + `"tools"` | `update_goal` | Grok oneshot ack + drain；无 `/goal` 时 `HarnessDisabled` |
| `tool-lsp` | `"lsp"` + `"tools"` | `lsp` | Grok `LspManager`/`dispatch`。没 `lsp.json` 时 fail-open |
| `mcp-client` | `"mcp"` + `"tools"` | `{server}__{tool}` | stdio JSON-RPC，fail-open |

权限门（询问 overlay）：`bash` `search_replace` `write_file` `scheduler_create` `kill_task`。

计划门：同上（`enter_plan_mode` 之后这些返回 blocked，直到 `exit_plan_mode`）。

### TUI / 斜杠（已接上的能力面）

| 面 | 行为 |
|---|---|
| `/plan [说明]` | 开计划模式；有说明则当一条 prompt 发出 |
| `/goal <目标>` | 开目标模式并提交 Grok `goal_instruction`；无参数闪用法 |
| `/tasks` | 列出 `"jobs"` + `"cron"` |
| `/mcps` | 列出 `"mcp"` 状态 |
| `/loop` | 已有，写 `"cron"` |
| 提问 overlay | 听 `ask/pending`，和权限 overlay 同款 |
| prompt 底栏 | 计划模式显示 `计划`；目标模式显示 `目标` |
| 滚动区 | live-lookup `"todos"` |

---

## 待做（核心 agent，还没挂上）

对照 Grok 默认 toolset / 原目标。做的时候：**cp Grok → 独立插件 → `register` → 测 specs/execute**。不要改 loop。

| 工具 / 套件 | 建议插件 | Grok 对照 | 说明 |
|---|---|---|---|
| `workflow` | `tool-workflow` | `grok_build/workflow` | 很重（Rhai）。要做就整套 register，不要空壳名字 |

对应 slash / TUI 有需要再加（例如 `/lsp` 不是必须）。

`task` 已挂名，但 coordinator 仍是 dock isolate spawn，不是整份 Grok `grok_build/task`。补的时候 **cp Grok，不要另写一套**。

### 已有工具上仍薄的地方（不是新名字）

相对 Grok 完整实现，这些 **已经在表里** 但行为更窄。补的时候还是改对应插件，不要新焊一层。

- `read_file`：纯文本，无 PDF / 图 / PPTX
- `grep` / `list_dir`：参数比 Grok 少
- `web_search`：不是 xAI Responses API
- `task` 已 register，但 spawn 路径仍是 dock isolate，不是整份 Grok task coordinator
- MCP：stdio + Content-Length；无 HTTP MCP、无 elicitation UI
- `ask_user_question` overlay：有 Other，自由输入比 Grok pager 简单

---

## 明确不做

| 项 | 原因 |
|---|---|
| Grok 登录 / 账单 / 用量 / 分享 / marketplace | 账号产品 |
| `image_gen` / `image_edit` / video gen / Imagine | 账号 + 生成；不要用 stub 占名 |
| voice、vim 模式、agent dashboard | 产品面，不是核心工具 |
| ExtraTools / `tools.mcp` 分发表 | 已废弃。MCP 和能力工具都 `register` 进 `"tools"` |
| path-dep 或修改 `grok-build/` | 只复制进 dock |
| 把新工具写进 `event_loop` / `agent-loop` | 违反一切皆插件 |

---

## 加一个工具时的检查

1. 是新插件、换现有插件，还是接到已有 named service / waterfall？
2. Grok 有没有现成实现可 `cp`？
3. `install_app` 是否 `plugin(...).wait()`？`install_fakes` 是否仍是 echo？
4. `Tools::specs()` 是否出现该名字？execute 是否走到 register 的 body？
5. 该挡权限 / 计划的是否进了 `acp.rs`？
6. 人需要看见的：slash、overlay、系统 prompt 一句。
7. 测试对着 **当前树** 断言 specs + 至少一条 execute，不要只 assert stub。
