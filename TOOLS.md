# 工具清单

对照当前 `install_app` 与 `cordis-spine` 源码，不是愿望列表。硬规则见 [AGENTS.md](AGENTS.md)，插件树见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)，斜杠 / 快捷键 / overlay 见 [CLI.md](CLI.md)。

本文件是**索引**：插件粒、权限门、缺口、不要做的事。单颗工具的细节在 [docs/tools/](docs/tools/)，**改一块只读对应那一份**。

核对：`cargo test -p cordis-spine --test round -- install_app_registers`。`install_fakes` 是 echo，**没有**下表能力工具——不要拿 echo 测试当产品清单。

---

## 原则

1. **一切皆插件。** 新工具是一颗（或一套）Cordis 插件，`inject: ["tools"]` 后 `ctx.tools.register()`。不要焊进 `event_loop` / `agent-loop`。
2. **一张 `"tools"` 表。** 粒度是套件：`tool-web` 同时 register `web_search` + `web_fetch`。MCP 是一颗插件连一个 server，工具仍进同一张表，公名 `mcp_{server}__{tool}`，不能盖掉 `bash`。
3. **先复制 Grok，再改成 Cordis。** 工作只在 `dock/`；不改、不 path-dep `grok-build/`。`cp` 过来再剥依赖（`register_resource!`、`tracing`、schemars、xAI 账号 client）。
4. **Live-lookup。** 调用点 `ctx.get` / `ctx.require`，不要把 `Arc<T>` 关进长生命周期闭包。
5. **扩展走 waterfall**（`tools/pre-execute`、`tools/execute`、`system-prompt/assemble`、`agent/turn-end`、`agent/step-start` 等），不在 loop 里分支。「这次调用不该这么跑」是 `tools/pre-execute`（夹在允许名单的两次检查之间——入站一次挡模型越界、改写后一次挡插件替它越界，所以改道逃不出预设的工具集——再往后才是计划门与权限门，可 `rewrite` / `deny`；门读的是改写**之后**的名字，所以 `bash` 改成 `read_file` 真的会少弹一次权限）。「出了文本但还不该收尾」是 `agent/turn-end`，「跑到一半要提醒」是 `agent/step-start`，不是再加一个 `if`。这三条子代理开轮 / 收尾时同样会跑，而 handler 只拿得到主会话的 `Sessions`——会写会话或消费一次性状态的，先看载荷里的 `identity`。
6. **MCP / 可选能力 fail-open。** 没配置或连不上时插件仍 Active，不让 `install_app` 失败。
7. **不接 Grok 账号产品。** 登录、账单、分享、marketplace、Imagine、voice、dashboard：不做。核心 agent 能力要补。本会话 token 账本不算账号产品，见 [CLI.md](CLI.md) `/usage`。
8. **产品面要跟上。** 新工具要有：register 进 specs（或 `register_deferred` 走 `search_tool`）、execute 路径、该挡的权限 / 计划门、人看得见的 slash / TUI（记 [CLI.md](CLI.md)）。用户可见文案中文；底栏 `Enter:send` 那种短 hint 保持英文无空格。
9. **工具表的顺序是缓存面，不是审美。** `specs_for_model_on` 把「名册里每个预设与角色都允许的工具」排在前面，其余（会被某个角色过滤掉的、`task` 这种 schema 随名册改写的、MCP / 按需的）排在后面；组内顺序不变。排序键只看名册、与当前预设无关，所以**子代理那张表是主会话那张的真前缀**。tools 排在整份 prompt 最前，中间少一项就让后面整段前缀作废。子代理 spawn 经 `AgentPresets::overlay_with_order` 继承父会话的排序依据——`overlay` 只带一个角色预设，自己算的分组和父会话对不上。
10. **测的是树，不是 stub。** 清单以 `install_app` 挂上的插件和 `Tools::specs()` 为准，不要留同名但走另一套 end state 的假实现。

---

## 已有（`install_app`）

挂载顺序见 `cordis-spine/src/bundle.rs`：能力插件在 `workspace_tools` 之后、`llm` 之前；`compact` 在 `llm` 之后（要注入 `"llm"`）。

| 插件 | ctx | 模型工具 | 一句话 | 细节 |
|---|---|---|---|---|
| `tools`（`workspace_tools`） | `"tools"` | `list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file` | 工作区内建七颗，不能被 register 盖掉 | [workspace](docs/tools/workspace.md) |

> 布局标准：一能力一顶层目录（`cordis-spine/src/tools/<tool_name>/`）。七颗文件工具各自一目录 + `fs_common.rs` + 薄 `workspace.rs` 套件调度；详见 [workspace](docs/tools/workspace.md#源码布局)。
| `jobs` | `"jobs"` | — | 进程表；前台 bash 也在表上 | [jobs](docs/tools/jobs.md) |
| `tool-jobs` | → `"tools"` | `job` `kill_task` | 列 / 查 / 等后台 bash 或子代理，以及杀 | [jobs](docs/tools/jobs.md) |
| `tool-monitor` | → `"tools"`（live `"jobs"`） | `monitor`（按需） | 长命令 stdout 盯梢 | [jobs](docs/tools/jobs.md) |
| `slash` | `"slash"` | — | 额外斜杠命令表，TUI live-lookup；内建 `CATALOG` 不能被盖掉。`kind`：`prompt` / `overlay` / `slot`（开已登记的 `tui.slots` id）/ `tool`（`text`=工具名，直接 `Tools::execute`，结果 Notice，权限门仍生效） | — |
| `tui.slots` | `"tui.slots"` | — | 动态包登记的 TUI 插槽（数据+回调，不是 ratatui widget）。始终挂上；TUI 用一次通用 `Overlay::Slot` 臂 | — |
| `skills` | `"skills"` | — | 发现 `SKILL.md`，登记 listing 段与 slash extras | [skills](docs/tools/skills.md) |
| `tool-skills` | → `"tools"`（live `"skills"`） | `skill` | 按需读 `SKILL.md` 正文 | [skills](docs/tools/skills.md) |
| `project-instructions` | → `agent/step-start` | — | `AGENTS.md` 进历史尾部 reminder，**不进系统提示** | [project-instructions](docs/tools/project-instructions.md) |
| `agent-presets` | `"agentPresets"` | — | YAML 人设 + 工具允许名单；`task` 的 `subagent_type` enum 来源 | [agent-presets](docs/tools/agent-presets.md) |
| `tool-web` | → `"tools"` | `web_fetch` `web_search` | Grok SSRF / 同 host 重定向 / htmd。`web_search` 无 xAI 账号，走同一套 fetch 打公开 HTML 索引 | [web_fetch](docs/tools/web_fetch.md) |
| `tool-browser` | `"browser"` + `"tools"` | `browser_*` 21 颗（按需） | BUA P2，in-process chromiumoxide CDP | [browser](docs/tools/browser.md) |
| `tool-computer` | `"computer"` | — | CUA C0 薄驾驶舱；桌面键鼠经 `cua-driver` MCP | [computer](docs/tools/computer.md) |
| `tool-todo` | `"todos"` + `"tools"` | `todo_write` | Grok merge/replace + 两条续跑 / 提醒 waterfall | [todo](docs/tools/todo.md) |
| `plan-mode` | `"planMode"` + `"tools"` | `enter_plan_mode` `exit_plan_mode` | 计划文件 `.dock/plan.md`；计划态挡住 bash / 写文件等 | — |
| `tool-ask-user` | `"ask"` + `"tools"` | `ask_user_question` | 事件 `ask/pending` | — |
| `tool-scheduler` | → `"tools"`（live `"cron"`） | `scheduler_create` `scheduler_list` `scheduler_delete`（按需） | 包着已有 `"cron"`，`register_deferred`。`fire_immediately` 立刻跑第一次；循环 7 天后过期（过期不跑最后一次，滚动区留中文说明）；最多 50 条；`task_id` 原地更新并保持相位。滚动区 Loop 卡；`/tasks` 里 `x` / `[✗]` 关闭 | — |
| `tool-task` | `"subagents"` + `"tools"` | `task` `send_message` `list_agents` `interrupt_agent` `report` | Grok coordinator + dock `ChildRunner` isolate | [task](docs/tools/task.md) |
| `tool-memory` | `"memory"` + `"tools"` | `memory_search` `memory_get`（按需） | `$DOCK_HOME/memory/{global,workspace-<slug>}/{topics,observations}/` + FTS `search.sqlite`（crate `dock-memory`）。默认关；`[memory] enabled` / `DOCK_MEMORY=1`。旧 `~/.dock/memory` 只读兼容。`register_deferred` | `/flush` `/dream` `/memory` `/remember` |
| `tool-goal` | `"goal"` + `"tools"` | `update_goal`（按需） | Grok oneshot ack + drain，带续跑 waterfall | [goal](docs/tools/goal.md) |
| `tool-lsp` | `"lsp"` + `"tools"` | `lsp`（按需） | Grok `LspManager`/`dispatch`，没服务器时 fail-open | [lsp](docs/tools/lsp.md) |
| `tool-workflow` | `"workflows"` + `"tools"` | `workflow` | Grok Rhai 引擎 + listing 段 | [workflow](docs/tools/workflow.md) |
| `mcp-client` | `"mcp"` + `"tools"` | `search_tool` `use_tool` | stdio + Streamable HTTP；BM25 检索、schema 去重、20KB 预算 | [mcp](docs/tools/mcp.md) |
| `dynamic-runner` | `"dynamicCordisRunner"` | — | 会话注册表 + 磁盘永久插件 | [cordis](docs/tools/cordis.md) |
| `tool-cordis` | → `"tools"` | `cordis_*`（按需） | 预置工厂 + Rhai；热挂插件 | [cordis](docs/tools/cordis.md) |
| `compact` | `"compact"` | — | Grok 会话压缩，85% 自动 | [compact](docs/tools/compact.md) |

另见 [工具结果图](docs/tools/images.md)（多模态，`ToolResult.images`）。

**权限门**（询问 overlay）：`bash` `search_replace` `write_file` `scheduler_create` `kill_task` `monitor` `cordis_run` `cordis_promote` `browser_evaluate`，以及全部 `mcp_cua-driver__*`。

**计划门**：同上（`enter_plan_mode` 之后返回 blocked，直到 `exit_plan_mode`）。例外：对 `.dock/plan.md` 的 `search_replace` / `write_file` 自动放行（对齐 grok）。

---

## 待做（已挂名、仍比 Grok 薄）

核心名字都已挂上。`workflow` 是 Rhai；`task` 走 Grok coordinator，child 仍是 dock isolate + `GrokStep`（无 worktree / MvpAgent）。补的时候改对应插件，不要新焊一层。

- `read_file`：文本 + 图片压缩多模态 + PDF（`pages` / `format=image|text`）+ PPTX DrawingML；无 ipynb / docx；**无单行长度帽**；binary gate 拦 docx 等（NUL / 扩展名）；技能 / 指令文件在 25k **token** 上限内整读（显式 offset/limit 仍窗口）
- `list_dir` 大目录摘要的头部采样：扩展名计数是对的，但「前 100 个文件名」对**同前缀爆炸**的目录信息量很低（`target/debug/deps` 下前 100 个几乎全是同一 crate 的 CGU 分片）。可考虑按公共前缀去重后再采样。实测踩到过
- `tool-images/` 与 `screenshots/` 没有清理，无限增长（`tool-output/` 已有，见 [workspace](docs/tools/workspace.md)）
- `web_search`：不是 xAI Responses API；结果是从 DuckDuckGo HTML 里扫出的**百分号编码**跳转链接，没有标题与摘要
- `task`：无 worktree / ACP / MCP pool；子代理共用父模型、父工具集与父工作目录
- `update_goal`：尚未自动 spawn Grok 的 goal plan writer / classifier / strategist
- MCP：`x-mcp-header` 自定义头未镜像
- `ask_user_question`：自由输入比 Grok 简单

---

## 明确不做

| 项 | 原因 |
|---|---|
| Grok 登录 / 账单 / 额度 / 分享 / marketplace | 账号产品。本会话 token 账本除外，见 [CLI.md](CLI.md) `/usage` |
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
6. 人要看见的 slash / overlay 是否记进 [CLI.md](CLI.md)？
7. 测试对着**当前树**断言 specs + 至少一条 execute，不要只 assert stub。
8. 细节写进 [docs/tools/](docs/tools/) 对应那份，本文件只留一句话 + 链接。
