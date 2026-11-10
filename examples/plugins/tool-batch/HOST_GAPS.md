# Rhai 插件宿主缺的能力（写 tool-batch 时记下的）

写 `tool_batch` 时用到、但 Rhai host 没有或者不对的能力。每条：现状（带 `文件:行`，以
`891122c` 为准）、对 tool-batch 的影响 / 绕法、建议加的 host API。按影响从大到小排。
「实测」= `cargo test -p cordis-spine --test dynamic -- example_tool_batch` 里跑出来的；
「读码」= 只看代码推的，没跑。

## 1. `host.call_tool` 用插件的根 ctx，不是调用方那一页 / 那个子代理（安全 + 正确性）

- 现状：`register_tool` 的执行体放进 `spawn_blocking`（`cordis-spine/src/tools/dynamic_runner/rhai_host.rs:438`），
  task-local 的 `EXEC_CTX`（`cordis-spine/src/tools/registry.rs:22-45`）就丢了；`call_tool` 走
  `tools.execute(call)`（`rhai_host.rs:1108`），而 `Tools::execute` = `execute_on(&self.ctx, …)`
  （`registry.rs:417-419`），用的是工具表自己的 ctx。
- 后果（门全部按根 ctx 判）：
  - 审批请求的 `agent_id` 恒为 `None`（**实测**：`example_tool_batch_each_call_goes_through_the_permission_gate`
    断言了这一点）。子代理调 `tool_batch`，审批框显示成主会话；子代理单独设的档位
    （`Permissions::set_agent_mode`）不生效——页是自动批准、子代理设成「请求批准」时，批里的 bash 会直接跑（读码）。
  - 能力档位 / 预设允许名单（`registry.rs:286-298` `outside_allowlist(exec, …)`）、只读场景
    （`cordis-spine/src/tools/read_only.rs:18-26`）、计划模式门（`registry.rs:518`）都查根 ctx。
    只读子代理本身调不到 `tool_batch`（未归类动态工具只有 `all` 档放行），但**页级**的计划模式 / 只读预设
    如果挂在页 ctx 而不是根上，批里的写操作就绕过去了（读码，GUI 多页场景需确认）。
  - 取消：工作区工具按 `exec.get::<TurnControl>(TURN)` 判停（`registry.rs:600-612`），根 ctx 上没有这一轮的
    TurnControl，用户点停止打断不了批里正在跑的 bash（读码）。
  - 会话身份（`Sessions::identity()`，`registry.rs:439-442`）也是根的。
- tool-batch 的绕法：无。只能在 README / 描述里写清「每步仍过审批」。
- 建议：执行体进 `spawn_blocking` 之前先取 `registry::exec_ctx()`，跟着这次调用传进 Rhai（例如放进
  `Host` 的一个 per-call 槽，或者给 `execute` 第二个参数 `call_ctx`），`call_tool` 改走
  `tools.execute_on(&exec, call)` 并包一层 `with_exec_ctx_async`。没有调用方 ctx 时（`host.on` 回调里）才退回根。
  这一条修好之前，不建议把 `tool_batch` 推荐给子代理用。

## 2. `call_tool` 只回 `content`：`is_error`、图片、实际工具名全丢

- 现状：`block_on_tool` 只 `Ok(result.content)`（`rhai_host.rs:1109`）；`ToolResult` 其实有 `is_error` 和
  `images`（`cordis-base/src/types.rs:494-506`），改道后的实际工具名在 `result.name`。
- 影响：失败只能猜。tool-batch 照抄宿主的兜底规则 `tool_output_looks_failed`（`types.rs:511-517`）再补上
  几条中文拒绝文案（`权限被拒绝`、`计划模式已阻止`、`只读模式：`、`MCP 工具未启用或已关闭`、
  `当前 Agent 预设未包含此工具`）——文案一改就漏判。MCP 工具 `isError: true` 但正文不以 Error 开头的也漏判。
  截图拿不到，cua 流程只能靠 `get_window_state` 的文本 / `verify_state`。
- 建议：`host.call_tool_ex(name, args) -> #{ content, is_error, name, images: [#{ mime, digest | path }], ms }`；
  `call_tool` 保持原样兼容。图片给摘要 / 落盘路径，不给 base64（1MiB 字符串上限，见 9）。

## 3. 插件自己的工具也只能回字符串：设不了 `is_error`、回不了图片

- 现状：`register_tool` 执行体返回值一律 `tool_result(call, content)`（`rhai_host.rs:438-447`），抛错才变成
  `Error: …`。
- 影响 / 绕法：tool-batch 中途停下时把第一行写成 `Error: tool_batch stopped at step N: …`、第二行才是 JSON，
  让 `finish()` 按前缀补上 `is_error`（`registry.rs` `finish` → `tool_output_looks_failed`）。模型拿到的不是纯 JSON。
- 建议：执行体可以返回 `#{ content, is_error?, images? }`（字符串照旧兼容）。

## 4. 不存在的工具名不报错，当成功回参数原文

- 现状：名字不在动态表、也不是工作区工具时，`dispatch_local` 直接把 `call.arguments` 当 content 回
  （`registry.rs:600-622`），`is_error` 为 false。**实测**（开发时临时探针，没留成测试——不想把这个行为锁死）：`tool_batch` 调 `no_such_tool {"x":1}` 得到
  `ok:true, results.step0 = {"x":1}`。MCP 形状的名字有单独的「MCP 工具未启用或已关闭」，内置形状的没有。
- 影响：模型把工具名拼错，批次会「成功」地往下跑。tool-batch 没法事先校验（见 5）。
- 建议：`call_tool` 对未注册的名字直接抛错；或者注册表本身对未知名字回 `is_error`。

## 5. 拿不到工具清单 / schema

- 现状：`host.get(name)` 只认服务名（`tools` / `slash` / bag，`rhai_host.rs:1247` `rhai_name_live`），没有工具级查询。
- 影响：做不了「跑之前先检查每步的工具在不在、参数对不对」，只能跑到那一步才知道。
- 建议：`host.has_tool(name) -> bool`、`host.tool_schema(name) -> Map | ()`、`host.list_tools(filter?) -> Array`
  （按调用方 ctx 的可见性过滤，见 1）。

## 6. 只能串行：`call_tool` 同步阻塞

- 现状：`tokio::task::block_in_place(|| handle.block_on(tools.execute(call)))`（`rhai_host.rs:1108`），
  Rhai 没有 async；`set_max_call_levels` 等限制下也没有线程。
- 影响：几个互不相关的只读查询（多个窗口状态、多个文件）只能一个个等。桌面写动作本来就该串行，影响有限。
- 建议：`host.call_tools(calls: Array) -> Array`（宿主内部 `join_all`，结果按序回），只对只读工具并发、
  有写操作的退回串行；或者 `host.spawn_tool(...) -> handle` + `host.join(handles)`。并发时审批仍走全局队列。

## 7. 没有单次调用超时，也没有取消信号

- 现状：`call_tool` 没有 timeout 参数；插件里看不到这一轮是否被取消（且取消本身按根 ctx 判，见 1）。
- 影响：tool-batch 的 120 秒总预算只能在**步与步之间**检查（`timestamp().elapsed`），一步卡住就一直卡。
- 建议：`call_tool(name, args, #{ timeout_ms })`；`host.cancelled() -> bool`；宿主在取消时让 `call_tool` 抛
  可识别的错误（如 `#{ kind: "cancelled" }`）。

## 8. 审批粒度：只有逐次调用一个粒度，提示里看不出来自哪个批

- 现状：`tool_batch` 自己不过门（`cordis-base/src/acp.rs:55-57` `needs_permission` 不认识它），批里每次调用
  各自按工具名过门、各自排队。**实测**：3 步 bash，前两步各弹一次审批，第二次拒绝后停在 step 1、第三步没跑。
  审批摘要只有那一次调用的参数，看不出是 `tool_batch` 第几步。
- 影响：十几步的 cua 批次第一次用要点十几次（「始终允许」之后才顺）；用户审批时看不到整批要干什么。
- 建议：审批摘要带来源前缀（`[tool_batch #3/12] …`）；可选的「整批预览一次批准」：插件先 `host.request_approval(summary)`
  拿一次整体批准，批里同一工具的后续调用在本批内免问（只对普通档，不对只读严格门）。

## 9. JSON 编解码不全

- 现状：Rhai 编译时没开 `metadata` 特性（`cordis-spine/Cargo.toml:36` `features = ["serde", "sync"]`），
  脚本里的 `parse_json` 走 `Engine::parse_json`，**顶层只能是对象**；`to_json` 只有 `Map` 上有，用 Rust 的
  `{:?}` 格式化键和字符串（rhai `src/api/json.rs:153-170`），控制字符会写成 `\u{1b}` 这种非法 JSON。
  宿主自己有 serde 版的 `dynamic_to_json` / `json_to_dynamic`（`rhai_host.rs:1190-1203`），没暴露给脚本。
- 绕法：数组包成 `{"v":[...]}` 再解析取 `.v`；任意值包成 `#{ v: x }.to_json()` 再剥掉外壳（见 `source.rhai`
  `tb_parse` / `tb_json`）。
- 建议：注册 `json_parse(text) -> Dynamic`（任意顶层）、`json_stringify(value) -> String`，都走 serde。

## 10. 内层调用不可见：不进会话日志、不进轨迹、调用 id 全一样

- 现状：只有模型发起的调用由循环记 `LogEvent::ToolExecute`（`cordis-spine/src/agent/runtime.rs:220`）；插件里的
  调用只过 `tools/execute` waterfall。所有内层调用的 id 都是 `rhai-<pluginId>`（`rhai_host.rs:729`）。
- 影响：GUI / TUI 只看到一张 `tool_batch` 卡片；轨迹面板看不到批里每一步。tool-batch 用返回值里的 `trace`
  （每步工具名 / ok|fail / 耗时）补一点。
- 建议：内层调用带唯一 id 和 `parent_call_id`，发子级工具事件（客户端可折叠显示在父卡片下）。

## 11. 运算预算是全局常量

- 现状：运行期每次调用 1,000,000 ops（`rhai_host.rs:39`、`:184`），字符串 1MiB（`:188`），map 16384 键。
- 影响：模板替换、截断、遍历大结果都吃 ops；50 步 × 大结果时有可能撞上限（测试里 50 步 echo 没撞到，没压测大结果）。
  单个工具结果超过 1MiB 时 `call_tool` 回来的字符串会怎样没实测。
- 建议：`plugin.toml` 里可声明 `max_operations`（有上限），或者给 `call_tool` 的返回值单独豁免字符串上限、只对
  脚本自己拼的串计数。

## 12. 没有 eval：模型只能写 JSON 步骤，写不了条件 / 变换

- 现状：`engine.disable_symbol("eval")`（`rhai_host.rs:197`），模块解析是 `DummyModuleResolver`（`:196`）。
- 影响：`tool_batch` 只能顺序 + `expect` 子串断言，没有 `when` 条件、没有对结果的变换（取某行、正则、算个数）。
  这次**没有**开 eval——顺序批次用不上；分支 / 循环交给 cua `run_script`。
- 如果以后要加（例如 step 上的 `when: "win.count > 0"` 或 `transform`）：按插件 opt-in，
  `plugin.toml` 加 `allow_eval = true`，宿主只给这个插件的引擎开 `eval`，且在一个独立的 `Engine`
  （同样的 ops / 字符串上限、无 host、无 http）里只求值表达式，变量表以只读 Scope 传进去。作为单独一个
  commit 改 `rhai_host.rs`。

## 13. bash / Python 里调不到 Dock 工具

- 现状：没有 `dock tool call <name> <json>` 这样的入口，插件是唯一的「代码里调工具」途径。
- 建议：之前讨论的 `dock tool call|list|schema`，经网关本地 socket 连回当前会话、带会话 / 子代理令牌、每次调用仍过门。
  和本插件互补：插件适合固定流程，CLI 适合模型现写脚本。

## 14. 小问题

- 动态工具没法声明自己只读 / 可并发：`register_tool` 没有 `read_only` 之类的字段，模型一次发多个 `tool_batch`
  时循环不会并发它们。建议 `register_tool #{ read_only: true }`（宿主再按内层工具复核）。
- `host.log` / `print` 只进 stderr（`rhai_host.rs:267`、`:895`），GUI 里看不到；调试插件只能看终端。
- 没有字面量 `${` 的转义：Rhai 普通字符串里 `${` 不特殊，但 tool-batch 的模板没设计转义（可以加 `$${`）。
- 计时够用：`timestamp()` + `.elapsed`（单调时钟）就能算每步毫秒，墙钟有 `unix_time_ms()`（`rhai_time.rs`）。这一项不缺。
