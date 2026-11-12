# 待办

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-todo`

- **ctx**：`"todos"` + `"tools"`
- **模型工具**：`todo_write`

状态：`pending` / `in_progress` / `completed` / `cancelled` / `awaiting_user`。`awaiting_user` = 在等用户回答或确认：不算完成（不进 `n/total done` 的分子），也不算模型欠的活（`TodoStats::open()` 不含它），下面所有提醒都跳过它。落盘只体现在 `chat_history.jsonl` 里 `todo_write` 的参数（`"status":"awaiting_user"`）和结果（`[awaiting_user]`）上；不含它的旧文件照常读。更早的版本读到它：旧版显示成和 pending 一样（GUI 是空心圆，TUI 是 `□`）（见 CHANGELOG）。

描述要求 `todo_write` 与下一步工作调用放在同一次回复；同时最多一个 `in_progress`；收尾前**如实记账**——做完的标 `completed`、等用户的标 `awaiting_user`、不做 / 堵住的标 `cancelled`，「还有未完成项」不是继续干活的理由（用户说先讨论、先看看，就停下来说清楚）。基座不写这条：极简等预设没有这颗工具。Grok merge/replace，含 Grok `effective_merge` 自动升级：`merge:false` 但每一项都只带已存在 id + status 时按合并处理，不会把 content 抹成 id。

工具约束：一次写入之后会留下两个及以上 `in_progress` 的，整次拒绝（`Error: only one item may be in_progress…`，列表不动），提示先把前一项标 `completed` / `cancelled` / `awaiting_user`（同一次调用里一起交接就行）。工具结果在列表后追加 `n/total done (… in_progress, … pending[, … awaiting_user])` 与状态提示：全部 `completed` / `cancelled` 且至少一项 `completed` 时写「清单已全部完成。」（`TODO_ALL_DONE`），全部 `cancelled` 时写「清单已全部关闭。」（`TODO_ALL_CLOSED`）；只剩 `awaiting_user` 时提示等用户、用户答了再改回；没有 `in_progress` 时提示开工前再标，停下来汇报 / 讨论 / 等用户都可以，但状态要如实——**不再**要求「别带着 pending 回复用户」。

`Todos` 另外暴露 `stats()`（TUI 折叠条）/ `revision()` / `bookkeeping_reminder()` / `focus_reminder()` / `next_turn_reminder()`。**注意 `"todos"` 不随子代理 isolate**，下面的 waterfall 都靠载荷里的 `identity`（或 `tools/execute` 时执行 ctx 的会话身份）判断，只在本页主会话生效；分页各有一份清单。

### 回合末：只记账（`agent/turn-end`，`ORDER_TURN_END_TODO = 10`）

条件全部满足才投票：以文字收尾、用户没排队下一条、是本页自己的回合；还有 `pending` 或没有后台任务（live job / running subagent）托底的 `in_progress`；本条用户消息里还没记过账（`MAX_TODO_BOOKKEEPING_FIRES = 1`，`keep_working_with` 式的胜出才扣，`agent/pre-step` 清零）。以下情况跳过：

- 本轮调过 `ask_user_question` 或 `handoff_to_user`（`tools/execute` 上置位一个 `AtomicBool`，只认执行 ctx 的会话身份等于本页的——子代理自己问用户不算父级的；结果 `is_error` 的不算，没问到用户；经 `use_tool` 转调的也算，`tools/execute` 报的是真实工具名；新的用户消息到 `agent/pre-step` 时复位）；
- 清单自上次记账步之后没变过（`Todos::revision()` 和上次记账步**跑完后**的值相同；这个值在下一条用户消息的 `agent/pre-step` 读，因为记账步自己可能写清单）。模型没理会的清单不会每轮都记一次账——每次记账都换一张工具表采样，前缀缓存会作废；
- 本页计划模式开着（`PlanMode::active()`）；
- 未完成项全是 `awaiting_user`。

投的是 `TurnEnd::bookkeep_with(…, ["todo_write"], …)`：循环追加提醒（以 `TODO_BOOKKEEPING_SENTINEL` 开头，列出欠着的项，要求按实际进度改状态、等用户的标 `awaiting_user`、不要开始新工作），**只再采一步**、这一步发给模型的工具表只有 `todo_write`（模型硬调别的工具会收到「没有执行：这一步只做 todo 记账…」的错误结果，不会真跑），这一步不跑 `agent/step-start`；跑完不管结果如何都结束这一轮，回给调用方的仍是模型上一段正文。这一步的正文**不落记录**（`Sessions::mute_llm_text`：不推给界面、`LlmStream.text` 留空，所以也不进下一轮 `model_history()`），推理和工具调用照常落——模型顺嘴的「好的」不是第二条回复。

这一步跑之前先确认跑得成，否则直接收尾、不落提醒（免得留下没有下文的孤儿提醒）：本页的工具表里没有 `todo_write`（预设可以不给）、已经按了 Stop、或上下文已到自动压缩门槛（`at_auto_threshold`，和 `Compact::maybe_auto` 同一把尺，85%）。记账步**不自动压缩**：收尾时为一次可有可无的记账压缩一遍不值得，到门槛就不记，下一轮第一步照常压缩。记账步里按 Stop：已落的工具结果照常落、没跑完的封口，但这一轮早已答完，仍按完成收尾、回上一段正文，不记成取消。记账票输给任何「继续干活」的票（`keep_working`，如目标、磁盘插件）——记账会结束这一轮，不能把别人正当的「还没完」吞掉；记账票之间照常按槽位。

### 回合中提醒（`agent/step-start`，`ORDER_STEP_START_TODO = 10`）

watchdog 在 `step == 0` 重置，每步最多说一件事，按顺序：

1. **清单刚变过**：上一步有过写入 → 追加一条「当前清单：正在做「a」；下一项「b」。（建议…）」，每轮最多 `MAX_TODO_FOCUS_REMINDERS = 4` 条；
2. **刚跑完测试 / 构建**：上一步的 `bash` 命令（取日志里最后一次模型输出之后的工具行）命中 `TEST_BUILD_COMMANDS`，有 `in_progress` 项，且同一步没写清单 → 「刚跑完测试 / 构建（`cmd`），「a」还是 in_progress。这项做完了就标 completed…」；
3. **连续 `TODO_NUDGE_AFTER_STEPS = 6` 步没动清单** → 带当前清单的 stale 提醒。

2、3 合计每轮最多 `MAX_TODO_NUDGES = 3` 次。没有欠着的项（只剩 `awaiting_user` 或全关）时都不说。

测试 / 构建识别（`is_test_or_build_command`）：命令按 `&&` `||` `;` `|` 换行切段，去掉段首的 `VAR=value`，段等于或以「词 + 空格」开头即算：`cargo test|nextest|build|check|clippy`、`npm test`、`npm run test|build`、`pnpm test|build`、`pnpm run test|build`、`yarn test|build`、`npx vitest|jest|tsc`、`vitest`、`jest`、`tsc`、`pytest`、`python(3) -m pytest`、`go test|build`、`make`、`cmake --build`、`ctest`、`mvn`、`gradle`、`./gradlew`、`swift build|test`、`xcodebuild`。

### 下一轮开头（`agent/pre-step` → 第 0 步）

用户发新消息时（本页、`enter`），若清单里还有 `pending` / `in_progress`，第 0 步追加软提醒「上轮清单还剩 N 项未完成」+ 列表，说明用户的新消息优先：改了方向就先用 `todo_write` 更新或清空（标 `cancelled`，或 `merge:false` 换新清单）。有 `awaiting_user` 项时同一条里点名它们，提醒看这条消息是否回答了——答了就改回 `pending` / `in_progress` 或直接 `completed`。清单为空或全部关闭时不说。

### API 变化（相对引入记账之前的 main）

`cordis-spine` / `cordis-base` 对外接口里不兼容的几处，及改法：

| 变化 | 改法 |
| --- | --- |
| `cordis_spine::TODO_GATE_SENTINEL` 不再导出（回合末不再发「continue working」那条提醒） | 改用 `TODO_BOOKKEEPING_SENTINEL`（`"Todo bookkeeping only — update statuses, do not start new work."`）。含义也变了：它只要求记账、这一轮随后结束，不再代表「回合会继续」。Rhai 钩子按字符串匹配的同样换成新开头。 |
| `Todos::gate_reminder` 改名 `Todos::bookkeeping_reminder` | 旧名保留为 `#[deprecated]`，转调新名，返回值是记账提醒正文。 |
| `TodoStatus` 新增 `AwaitingUser`（枚举不是 `#[non_exhaustive]`） | 穷尽 `match` 要补一支：它既不算完成也不算模型欠的活；没有单独展示的地方按 `Pending` 画即可（旧版就是这么读的）。 |
| `TodoStats` 新增公有字段 `awaiting_user` | 结构体字面量补 `awaiting_user: 0` 或 `..Default::default()`。`total()` 含它，`open()` 不含，`settled()` 不含。 |
| `stale_reminder(steps)` 改为 `stale_reminder(steps, &TodoState)`，不再从 `tools::todo_write` 重导出 | `tools` 模块在 crate 内私有，`cordis_spine` 从没导出过它，外部代码碰不到；crate 内改走 `Todos` 的方法。 |
| `TurnEnd::settle()` / `decision()` 的胜者现在可能是记账票 | 只读正文的调用方会把记账当成「继续干活」。驱动自己的循环时改用 `settle_round()`，`only_tools` 为 `Some` 时只再采一步、只给这些工具、然后收尾；`decision_tools()` 是只读版本。新增的只有 API：`TurnEnd::bookkeep_with`、`TurnEndRound`。`Continuation` 是私有类型，不算。 |
