# tool-batch / Code Mode 设计依据

目标：减少模型来回。两条路一起走：

1. **一轮多调用**：模型一次回复里发多个互不依赖的工具调用，Dock 并发跑完一起交回（核心循环，
   `parallel_safe`）。
2. **代码里编排**：一次 `tool_batch` 调用跑一串工具，中间结果留在插件里，只回最后要的几项
   （本插件 + Rhai host 能力）。

下面每条决定都对应一个公开来源。

## 来源

| # | 来源 | 要点 |
| --- | --- | --- |
| S1 | [MCP 规范 2025-06-18 · Tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools) | 未知工具是协议错误；执行失败用结果里的 `isError` 报；客户端 SHOULD 给调用设超时、留人工确认、记录调用 |
| S2 | [MCP 规范 2025-06-18 · Schema `ToolAnnotations`](https://modelcontextprotocol.io/specification/2025-06-18/schema#toolannotations) | `readOnlyHint` 等都只是**提示**；不可信服务器给的注解不能拿来做工具使用决策 |
| S3 | [openai/codex PR #23750](https://github.com/openai/codex/pull/23750)（`codex-rs/core/src/tools/handlers/mcp.rs`） | 标了 `readOnlyHint: true` 的 MCP 工具可并行，无需服务器另行声明；没标的照旧串行；只读不绕过沙箱 / 审批 |
| S4 | [Anthropic · Parallel tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/parallel-tool-use) | 推荐系统提示 `<use_parallel_tool_calls>`；独立只读操作并行、有副作用 / 有顺序要求的串行；computer use 一批按顺序跑、遇错即停，没跑的回 `is_error` |
| S5 | [Anthropic · Prompting best practices（并行调用）](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices) | 有依赖（后一个调用的参数要用前一个的结果）就不要并行，不要用占位符猜参数 |
| S6 | [Anthropic · Introducing advanced tool use（Programmatic Tool Calling）](https://www.anthropic.com/research/advanced-tool-use) | 代码里调工具，中间结果不进上下文，只有最终输出进；工具按 `allowed_callers` 显式 opt-in；适合 3 步以上的依赖链与对很多项并行 |
| S7 | [Anthropic · Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp) | 同上思路；代码执行要沙箱、资源上限与监控 |
| S8 | [Cloudflare · Code Mode](https://blog.cloudflare.com/code-mode/) | 沙箱断网、无文件系统，只能通过绑定（bindings）调外部；结果只回脚本输出 |
| S9 | [CodeAct（arXiv 2402.01030）](https://arxiv.org/abs/2402.01030) | 带控制流 / 数据流的代码动作轮数更少、成功率更高；把错误信息回给模型便于自我修正 |
| S10 | [LLMCompiler（arXiv 2312.04511）](https://arxiv.org/abs/2312.04511) | 规划出带 `$1` 占位符的 DAG，取数单元代入变量，执行器把互不依赖的任务并行；慢任务与规划开销限制加速 |

## 决定 → 来源

| 决定 | 来源 |
| --- | --- |
| 插件里的 `host.call_tool` 以**调用方**身份执行（审批身份、子代理档位、计划模式、只读场景、停止） | S1（人在回路）、S7 / S8（编排层不能扩大权限） |
| 不认得的工具名报错，不再回显参数当成功；`tool_batch` 跑之前用 `host.tool_schema` 校验工具名和必填参数 | S1（未知工具是错误）、S9（错误要清楚可修） |
| 结构化结果：`host.call_tool_ex` 回 `is_error`；插件工具可回 `#{ is_error, content }`；`tool_batch` 按工具自己的 `is_error` 判失败，不再猜 `Error:` 前缀 | S1（`isError`）、S4（`is_error`） |
| `host.call_tools_parallel`（`join_all`）与 `tool_batch` 的 `parallel` 分组：只放互不依赖的调用；组里的引用只能指向前面的步骤 | S4、S5、S10 |
| 并行时每颗调用照样过门，审批仍走全局队列一个个问 | S1、S3（只读不绕审批） |
| 单步 `timeout_ms`、`host.cancelled()`、停止时中止脚本与进行中的调用；默认不设时限（等审批的时间也算在里面） | S1（超时）、S7（资源上限） |
| 中间结果留在插件变量表，只回 `return` 点名的几项并截断 | S6、S7、S8 |
| `when` / `transform` 用 `host.eval_expr`：按插件 `allow_eval = true` 显式打开；只求单个表达式；单独的引擎，没有 host / http / 文件，同样的操作数上限 | S6（opt-in）、S7（沙箱 + 资源上限）、S8（只有绑定） |
| `${var}` 引用 + 每步自动 `stepN` 变量 | S10（占位符代入） |
| 核心循环：`use_tool` 派发的 MCP 工具，`readOnlyHint: true` 才和别的只读调用一起并行；没标的、要审批的、写操作照旧串行 | S3、S4 |
| `readOnlyHint` 只用来**排并发**，不用来跳过审批或放宽能力档位 | S2、S3 |
| cua-driver 只把几颗纯读取工具（`list_apps` / `list_windows` / `get_window_state` / …）列为可并行，动作类（点、输、按键）一律串行 | S4（computer use 按顺序、遇错即停） |
| 系统提示加并行调用的指引：独立操作同一轮一起发，有依赖的分开 | S4、S5 |
