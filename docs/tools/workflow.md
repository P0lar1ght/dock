# 工作流

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-workflow`

- **ctx**：`"workflows"` + `"tools"`
- **模型工具**：`workflow`

Grok Rhai 引擎（`vendor/xai/workflow`）+ 同款 oneshot ack。内置 `deep-research` 脚本在 `cordis-spine/src/tools/workflow/workflows/deep_research.rhai`；磁盘扫描 bundled → 内置 → `{cwd}/.dock/workflows/<name>.rhai` → `~/.dock/workflows/`（同名不覆盖已有）。向 `"context"` 登记 listing 段（窗口 token ×4 ×**3%**，与技能共用 `src/listing.rs`）。同样两道门：**只有主会话拿这一段**（同 `listings: true` 开关），且 `workflow` 工具必须对本会话可见才发。每个目录项登记 slash extra（`kind: tool`，`text=workflow`；不可盖 `RESERVED_SLASH` / `/skills`）。`/name` 与 `/workflow <name>` 直接 `Tools::execute`，不经模型。`tools/execute` 路径靠近 workflows 目录时中途发现。`code` / `cordis` 允许名单含 `workflow`。**`register`（进 sampler）**——理由同 `skill`

### Host（`src/tools/workflow/host.rs`）

一次 run 一个 `WorkflowHost`，引擎的十一条 `WorkflowHostRequest` 都在它的 `dispatch`。三条不变式：

- **预算**：`agent_budget`（默认 128，上限 1024）由 `ReserveAgentCalls` 兑现。引擎自己不记账（`agent()` 预留 1、`parallel()` 一次预留整批），超了回 `AgentCallQuotaExceeded`，引擎翻成 `WorkflowOutcome::BudgetExceeded`。预留即扣减，一个池子，引擎负责把没用掉的 release 回来。schema 重试不扣预算，另有 `budget × 2` 的真实调用闸门。
- **并发**：每 run 一个 Semaphore，大小取 `TaskConfig::workflow_max_concurrent_agents`（默认 32）并 clamp 到机器并行度。workflow 的子代理**不走会话限流**（`admission.rs` 对 workflow owner 直接放行），只受这个池约束。
- **取消**：子代理的 owner 带真实 run id，并共用 run 的 `CancellationToken`。host 循环退出后按 run id 取消并等 20 秒排空。
- **不递归**：`workflow` 工具在 `depth > 0` 时回 `workflow_depth_exceeded`。宽口径角色（`general-purpose`）的工具集里就有 `workflow`，而 workflow 子代理默认就是这个角色——不挡的话每一层都拿一份全新的 `agent_budget` 与一个全新的并发池，run 级预算正好管不住。

`SpawnAgent` live-lookup `"subagents"`，走 `Subagents::spawn_for_workflow`（返回 `SubagentResult` 原件，失败如实标 `success: false`）。`output_schema` 编译成 `jsonschema::Validator`（拒外部 `$ref`）拼进 prompt；产出不合约就带错误 `resume_from` 重试一次，再不合约就 `success: false`。

`capability_mode` 真正生效：解析成 `tools::capability::CapabilityMode`（`read-only` / `read-write` / `execute` / `all`，档位语义与 grok 一致，读写与执行互不包含），挂进子会话 ctx 的 `"capability"`，工具允许名单与 sampler 工具表都查它。它**压在 MCP / 动态包的允许名单豁免之上**——那两类绕过预设允许名单是有意的，但绕不过「这次委派只准读」。分类按工具名做且**默认关闭**：认不出来的名字只有 `all` 放行。档位名写错当场报错，不会按不设限跑。子会话 ctx **先 `isolate("capability")` 再 `provide`**：没隔离的名字落的是共用注册表，一次 `parallel` 起四个 read-only researcher 会有三个撞「service 已注册」起不来，而且那份档位会漏给主会话（`/deep-research` 变成「当前 Agent 预设未包含此工具」）。`"model-override"` 同理。

`model` / `effort` / `max_output_tokens` 走 `ModelOverride`，挂进子会话 ctx 的 `"model-override"`，由 `llm` 采样器读（主会话永远没有这一项 = 跟 `"settings"` 走）。**不是**给子会话隔离一份 `AppSettings`：那里面还有权限档位这类会话级状态，隔离一份等于让子代理带着一张过期的权限快照跑。

`model` 必须是**本机模型目录（`config.toml`）里真有的 id**。这几个键是照 grok 的脚本 API 写的，脚本里那个名字大概率是 grok 的模型；照单全收只会把这个孩子打到一个解析不出端点的 id 上，换来一个 404。认不出来的退回父会话的模型并记一条 warn。`effort` 在模型不支持推理时不发。

`log()` 进快照的环形缓冲（最近 50 条、单条 4KB），任务条与 `/workflow runs` 显示最新一条；不进会话历史，不吃模型上下文。`phase()` 截断到 256 字节。`Telemetry` 丢弃（dock 没有埋点通道），只留 debug 日志。

### 主线程只在收尾时被叫醒一次

workflow 子代理有两条会通到主线程的路，**都被按 owner 拦住了**：回合结束通知（`surface_completion: false`，`runner.rs` 真的会读它）与 `report`（`ChildStore::push_report` 按 owner 分流，进 run 自己的队列）。理由是同一条：run 还在跑时把「某个孩子跑完了」推给主线程，主线程就会在一份残缺的中间结果上烧一整轮——一次 deep-research 有十来个孩子。

子代理的 `report` 立刻进两处给**用户**看的通道：run 快照（overlay 那一行的 `latest_report` + 进度流）和滚动区 `LogEvent::Notice` 卡片。`Notice` **不进** `model_history`，所以看得见不等于叫醒主模型。run 收尾时**一条** `WorkflowDone` 信箱带着最终结果与全部过程上报（按发生顺序、每条截断到 600 字、最多 64 条，丢掉的在通知里如实报数）叫醒主线程，兑现工具描述里「完成后会自动汇报」；滚动区另有一张收尾卡。带的是**每一条**上报而不是每行的 `latest_report`——后者是覆盖写的，同一个孩子报第二次就把第一次挤掉了。每条上报署的是行上的 label（`researcher-0`），不是子代理 UUID。

scratch 在 `~/.dock/scratch/<run_id>-<进程标记>/`：run id 是**会话内**序号（`wf_1`、`wf_2`…），两个 dock 同时开着会各自从 `wf_1` 起，光按 run id 建目录会让两次无关的 run 共用一个 scratch（互相读到对方的 `report.md`，配额也算在一起）。旧目录不自动删——那个路径是当着用户面给出去的。文件名必须是单个相对路径组件、拒符号链接、64 文件 / 单文件 10MB / 总量 64MB 配额、先写临时文件再 rename，读写都在 `spawn_blocking` 里。`render_template` 与 `git_diff_since` 返回 `Unsupported`（dock 没有模板表；仓库读写走 bash 权限门）。

快照表只留最近 32 条**已结束**的 run（活跃的一条不动）：`WorkflowState` 活得和进程一样久，不淘汰的话跑一整天的会话会把每次 run 的上报和日志都攒着。被淘汰的 run 停不了也查不到。

### 停止

`Workflows::stop(name|run_id)` 取消一次在跑的 run。入口：`/workflow stop <name>`、`/tasks` 或 Workflow Runs 里选中行按 `x`、任务行的 `[✗]`。已收尾的 run 不能再停。

### Workflow Runs 详情页

`/workflow` 列表里 Enter 打开详情页（`/tasks` 里不展开：那是五类任务的总表）：左栏是脚本 `meta.phases` 声明的阶段（带 `已完成/总数`），右栏是该阶段的子代理行——`:: label · 状态 · tokens · 耗时`，下面缩进一行是它最新的 `report`。`↑↓` 换阶段、`x` 停这条 run、`esc` 回列表。

子代理归到哪一阶段：先看 `agent(phase:)`，没写就回退到脚本当时 `phase()` 的那一段。两者都没有的才落到末尾的「其它」；脚本完全没声明阶段时左栏整个收掉。

**没有 pause/resume**，底栏不列这两个键。两条都卡在引擎上：`Paused` 只能由脚本自己发起（`vendor/xai/workflow` 的 `engine.rs` 把 `pause()` / `needs_input()` 翻成 `ControlToken::Pause`），host 侧只有取消，没有「按一下停在这儿」；resume 要把 `Journal::new(Some(path))` 落盘再 `Journal::load` 回来，那是一份新的落盘格式。引擎是冻结副本，不为本仓需求改。
