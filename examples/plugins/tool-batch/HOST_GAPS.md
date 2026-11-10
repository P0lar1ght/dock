# Rhai 插件宿主的能力缺口（写 tool-batch 时记下的）

写 `tool_batch` 时用到、但 Rhai host 原来没有或者不对的能力。第一版（`5a6178e`）记了 14 条；
这一轮（Code Mode）修了其中大部分，状态标在每条标题上：

- **已修**：宿主已改，`tool_batch` 已改用新能力，有测试。
- **部分**：改了一部分，剩下的写在「还缺」。
- **未修**：没动，原因和建议保留。

测试都在 `cordis-spine/tests/dynamic.rs`（`plugin_*` 测宿主能力，`example_tool_batch_*` 测插件）。
宿主能力的用法以内置技能 `cordis-plugin-development` 的 `references/host-api.md` 为准。

## 1. 已修：`host.call_tool` 以调用方身份执行

- 原来：`register_tool` 的执行体在 `spawn_blocking` 里跑，丢了 task-local 的 `EXEC_CTX`，
  `call_tool` 用插件的根 ctx 过门——审批 `agent_id` 恒为 `None`、子代理档位 / 页级计划模式 /
  只读场景不生效、停止打断不了。
- 现在：进阻塞线程前取 `registry::exec_ctx()`，挂成线程局部的调用方 ctx；`call_tool`（及下面的新函数）
  走 `Tools::execute_on(调用方, …)`，并与调用方的停止赛跑；新增 `host.cancelled()`；脚本每 4096 步
  查一次停止、停了就中止。
- **注意**：只有在插件工具的 `execute` 里调才是调用方身份。`apply` 里、`host.on` 回调里（`session/event`、
  `agent/step-start`、`agent/turn-end`）没有调用方，`call_tool` 仍以**插件自己的 ctx**（根上的设置、
  没有子代理身份、没有页级计划模式 / 停止）过门——这是已知限制，没改。
- 测试：`plugin_call_tool_runs_as_the_caller`（计划模式挡写、审批记在子代理名下、停止 10 秒内返回），
  旧代码上会失败。

## 2. 已修：结构化结果 `host.call_tool_ex`

- `host.call_tool_ex(name, args[, #{ timeout_ms }]) -> #{ ok, is_error, text, images, tool }`；
  `call_tool` 不变（仍回字符串）。`images` 只给张数——图片本身仍不进脚本（见 11）。
- `tool_batch` 改按 `is_error` 判失败，删掉了照抄 `tool_output_looks_failed` 的前缀猜测。
- 测试：`plugin_call_tool_ex_reports_structured_results`。

## 3. 已修：插件工具能回 `is_error`

- `execute` 可以回 `#{ is_error: bool, content | text }`（同 MCP `isError`）；字符串和普通 map 照旧。
- `tool_batch` 不再把 `Error: …` 拼在 JSON 前面，停下 / 被拒时直接 `is_error: true`。
- 还缺：插件工具回不了图片。
- 测试：`plugin_tool_can_return_is_error`。

## 4. 已修：不存在的工具名报错

- 原来：`Tools::dispatch_local` 把未知名字的参数原样回显成成功。
- 现在：host 侧先查 `Tools::knows`，不认得回 `Error: unknown tool \`name\``（`is_error`）。
  核心的兜底分支没动（只有插件这条路会撞上）。
- 测试：`plugin_call_tool_rejects_unknown_tools`。

## 5. 已修：工具清单与 schema（插件侧的 host API，不是新的模型工具）

- `host.list_tools() -> [#{ name, description, plan_blocked }]`、
  `host.tool_schema(name) -> #{ name, description, parameters, plan_blocked } | ()`。
- 不是第二套目录：都读同一份 `Tools` 登记表，按与 `execute_on` 同一个 `outside_allowlist`（允许名单 +
  能力档位）筛；`parameters` 与 `search_tool` 回的 `input_schema` 是同一个函数（`mcp::input_schema`）。
- 计划模式：受计划门管的工具（`acp::blocked_in_plan`）标 `plan_blocked: true`，**不剔除**——计划模式下
  只读 bash 和写计划文件仍放行，按名字剔会误伤。
- 模型侧发现工具仍用 `search_tool`；`tool_batch` 的描述与 SKILL 要求先 `search_tool` 拿 schema 再拼 steps，
  批次自己的校验（工具名 + `required` 参数，不做完整 JSON Schema 校验）只是兜底。
- 测试：`plugin_lists_tools_and_reads_schemas`（含计划模式）、`example_tool_batch_rejects_bad_batches_before_running`。

## 6. 已修：并行调用

- `host.call_tools_parallel([#{ tool, args, timeout_ms? }])`：最多 16 颗，结果按序回。同一个调用方身份；
  要审批的仍进全局审批队列一个个问。
- 宿主按 `parallel_safe_with` 分批，和核心循环的 `parallel_batches` 同一规则：相邻的只读调用并发（`join_all`），
  写文件、bash（非只读命令）、浏览器、cua 动作**按传入顺序一颗颗跑**。插件不能借它让 `write_file` 和 cua
  点击同时发生。批与批之间查停止，停了剩下的直接回 `cancelled`。
- `tool_batch` 的 `parallel` 分组走它，所以同样规则。
- 测试：`plugin_call_tools_parallel_runs_concurrently_in_order`、`plugin_call_tools_parallel_serializes_unsafe_calls`、
  `plugin_call_tools_parallel_stops_mid_batch`、`example_tool_batch_parallel_group_runs_concurrently`。

## 7. 已修：单次调用时限与停止

- `timeout_ms`（`call_tool_ex` 的选项、`call_tools_parallel` 每颗、`tool_batch` 每步），1..600000，超出报错
  （不再静默截到上限）；默认不限时（等用户审批的时间也算在里面）。停止时进行中的调用回 `cancelled`。
- 超时 / 停止时真的停下：
  - `bash`：前台任务的等待 future 被丢掉时（`KillOnDrop`）对 job 发取消信号，`run_job` 杀进程；
    转后台、正常结束的不受影响。
  - MCP stdio：rpc 的 future 被丢掉时（`CancelOnDrop`）摘掉 pending，并发 `notifications/cancelled`
    （`initialize` 按规范不取消）。
  - 等审批的调用被超时 / 停止丢掉时，审批请求撤出队列（`Withdraw`）。测试：`plugin_timeout_or_stop_withdraws_a_pending_approval`。
- **超时不等于没执行**：调用可能已经跑了、副作用可能已经发生（文件写了、按钮点了），别直接重跑。
- 还缺：MCP **HTTP** 传输丢掉请求时只是断开那次 POST，不发 `notifications/cancelled`（未核实服务器侧是否停）；
  停止靠 50ms 轮询 `TurnControl`（不是事件驱动），所以停止最多晚 50ms 生效。
- 还缺：stdio 共享一条流，多路并发 `tools/call` 时 elicitation 的提问按发送顺序归属，
  不带 related id 的服务器可能把提问记到另一颗调用名下（`call_tools_parallel` 并发同一台 stdio 服务器时会遇到）。

## 8. 未修：审批粒度

- 批里每次调用各自按工具名过门、各自排队（**实测**：`example_tool_batch_each_call_goes_through_the_permission_gate`）。
  审批摘要看不出来自哪个批、第几步。
- 建议：摘要带来源前缀（`[tool_batch #3/12] …`）；可选的「整批预览一次批准」。没做：涉及审批语义，单独讨论。

## 9. 已修：JSON 编解码

- 运行期引擎上的 `parse_json` / `to_json` 换成 serde_json：任意顶层值、正确转义；`to_json` 对数组 / 标量也能用。
- `tool_batch` 删掉了「包一层再剥」的绕法。
- 测试：`plugin_json_round_trips_any_value`。

## 10. 未修：内层调用不进会话日志 / 轨迹

- 只有模型发起的调用记 `LogEvent::ToolExecute`；插件里的调用只过 `tools/execute` waterfall。
  内层调用 id 现在是 `rhai-<pluginId>`（并行时加 `-<序号>`），仍没有 `parent_call_id`。
- `tool_batch` 用返回值里的 `trace` 补。建议：内层调用发子级工具事件。本轮按要求跳过（轨迹日志）。

## 11. 未修：运算预算是全局常量

- 每次调用 1,000,000 ops、字符串 1MiB、map 16384 键。`allow_eval` 的表达式引擎用同样的上限，而且
  **每次 `eval_expr` 一份新的预算**（一个批里 50 步就是 50 份，再加脚本自己那份）。
- **实测缺口**：Rhai 1.25 不给原生数组函数（`map` / `filter` / `reduce` …）回调里的闭包计操作数，
  `on_progress` 也不触发——嵌套 `reduce` 能跑几百万步不被拦，停止也打断不了。`eval_expr` 已补上：
  另按变量读取计数（`on_var`，同样 1,000,000 上限，每 4096 次查停止），测试
  `plugin_eval_expr_hits_the_operation_limit`。**插件脚本本身的运行期引擎没补**（同样能被这种写法绕过），
  建议单独评估给运行期引擎也挂同样的计数。
- 建议：`plugin.toml` 可声明有上限的 `max_operations`。本轮按要求跳过（可配置上限）。

## 12. 已修（按插件 opt-in）：表达式求值

- **启动时从插件目录装上的磁盘插件**（`PluginOrigin::Disk`），且 `plugin.toml` 写 `allow_eval = true`，
  才有 `host.eval_expr(expr, vars?)`：只求单个表达式（不能写语句、定义函数，闭包体里也不能），跑在单独的
  引擎里——每次一份操作数预算（见 11），只有 JSON 函数，没有 host / HTTP / 文件，停止时中止，最长 4096 字节。
- 会话里 `cordis_define` 的包**永远打不开**：即使 `source_path` 指向 `.dock/plugins/<id>/`、旁边有一份
  模型自己写的 `allow_eval = true` 的 `plugin.toml`，也不读（`Plan.honor_eval` 只对 `PluginOrigin::Disk` 为 true）。
  `cordis_promote` 写成磁盘插件、下次启动装上后才生效——那一步要走权限门。
- 脚本自己的 `eval` 仍然禁用。
- 测试：`plugin_eval_expr_is_opt_in_and_expression_only`（断言具体原因）、`plugin_eval_expr_hits_the_operation_limit`、
  `plugin_eval_expr_stays_off_for_cordis_define_packages`、`example_tool_batch_when_transform_and_timeout`。
- `tool_batch` 的 `when` / `transform` 用它，所以本插件的 `plugin.toml` 开了 `allow_eval`。

## 13. 未修：bash / Python 里调不到 Dock 工具

- 没有 `dock tool call <name> <json>` 这样的入口。建议经网关本地 socket 连回当前会话、带会话 / 子代理令牌、
  每次调用仍过门。本轮按要求跳过。

## 14. 小问题

- 部分：模型一次回复里的多个调用，循环现在会把 `use_tool` 按内层工具判断、MCP 按 `readOnlyHint`
  （以及 cua-driver 的只读几颗）并行（`agent/capability.rs` `parallel_safe_with`）。动态工具本身
  （包括 `tool_batch`）仍不能声明自己只读，所以两个 `tool_batch` 不会并发。
- 未修：`host.log` / `print` 只进 stderr。
- 未修：模板没有字面量 `${` 的转义。
- 已修：`tool_batch` 不能经 `use_tool` 绕一层调自己（预校验看 `use_tool` 的 `tool_name`）。
- 已修：`with_call_ctx` 用 Drop 守卫恢复线程局部的调用方 ctx，脚本 panic 时也不会把 ctx 留在线程上。
- `readOnlyHint` 标错（写工具被标成只读）的后果只是它和相邻只读调用**排在一起跑**；审批、计划门、能力档位不变。
