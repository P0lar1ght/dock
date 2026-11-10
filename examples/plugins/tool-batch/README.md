# 批量调工具 tool-batch（示例插件 · 模型工具）

一颗 Rhai 磁盘插件，注册模型工具 **`tool_batch`**：一次调用跑一串工具（Dock 内置、MCP 如
`mcp_cua-driver__*`、别的插件工具），互不依赖的几颗可以放进 `parallel` 分组同时跑。中间结果
存在插件里的变量表，**不进模型上下文**，最后只回 `return` 点名的几项（每项截断到 `max_chars`）。
目标是把「查几样 → 汇总」「定位 → 点击 → 输入 → 确认」这类好几轮来回压成一轮。

用到的宿主能力（Rhai host，见内置技能 `cordis-plugin-development` 的 `host-api.md`）：
`host.call_tool_ex` / `host.call_tools_parallel`（结构化结果、以调用方身份执行、时限、停止）、
`host.tool_schema`（跑之前兜底校验；与 `search_tool` 的 `input_schema` 同源）、`host.eval_expr`（`when` / `transform`，`plugin.toml` 里
`allow_eval = true` 才有）、`host.cancelled`。不联网、不读写文件。
设计依据与出处见 [`DESIGN.md`](DESIGN.md)，宿主还缺什么见 [`HOST_GAPS.md`](HOST_GAPS.md)。

## 装上

```bash
mkdir -p ~/.dock/plugins
cp -R examples/plugins/tool-batch ~/.dock/plugins/tool-batch
# 让模型知道什么时候用（可选，推荐）
mkdir -p ~/.dock/skills/tool-batch
cp examples/plugins/tool-batch/SKILL.md ~/.dock/skills/tool-batch/SKILL.md
```

重启 Dock（`plugin.toml` 里 `enabled = true`，启动时自动加载）。`/cordis` 里能看到「批量调工具」。
`tool_batch` 是动态工具，不进采样表：模型用 `search_tool` 搜 `tool_batch`，再用 `use_tool` 调；
同一轮里手动试可以用 `cordis_call`。

`plugin.toml` 里的 `allow_eval = true` 只为 `when` / `transform`：表达式在单独的引擎里求值
（只求单个表达式；没有 host、HTTP、文件；每次求值一份操作数预算）。只有启动时从插件目录装上的
磁盘插件认这个键；在会话里用 `cordis_define` 定义的包，即使 `source_path` 旁边有这份 `plugin.toml`
也打不开。不需要这两个字段可以删掉这一行，
其余功能不受影响，用到时会报错说明。

## 输入

```jsonc
{
  "steps": [
    // 一步是一个调用……
    {
      "tool": "工具全名，如 bash / read_file / mcp_cua-driver__get_window_state",
      "args": { "...": "字符串里可写 ${var} / ${var.a.b} / ${var.items.0}（-1 = 最后一个）" },
      "save_as": "可选，结果变量名（不能含点）",
      "expect": "可选，结果文本必须包含的子串，不含就算失败",
      "when": "可选，表达式；false 就跳过这步，如 \"win.count > 0\"",
      "transform": "可选，表达式；结果叫 result，求得的值代替结果存下，如 \"result.items.map(|x| x.id)\"",
      "timeout_ms": 30000,
      "on_error": "stop（默认）| continue"
    },
    // ……或者一个并行分组：组里相邻的只读调用同时跑，写 / bash / 桌面动作按顺序一颗颗跑；整组跑完再往下。
    { "parallel": [ { "tool": "...", "args": {}, "save_as": "a" }, { "tool": "...", "save_as": "b" } ],
      "on_error": "stop" }
  ],
  "return": ["要返回的变量名或路径，如 win 或 win.title；省略 = 最后跑的那一步"],
  "max_chars": 4000
}
```

- 每步结果自动存成 `step0`、`step1`…，`save_as` 再多存一个名字。并行分组的 `stepN` 是数组
  （`stepN.0`、`stepN.1`…，被 `when` 跳过的是 `null`）。
- 结果文本是 JSON 就解析后存（能按路径取字段），否则存字符串。
- 引用：整串**恰好**是一个 `${...}` 时代入原值（对象 / 数组 / 数字原样传给下一个工具）；
  夹在文字里时拼成文本，非字符串值拼成 JSON。没有转义写法，字面量 `${` 写不出来。
  并行分组里的调用只能引用**前面步骤**的结果，组内互相引用不了。
- `when` / `transform` 是 Rhai 表达式，前面存的变量直接当名字用（变量名要是合法标识符才能引用）。
  `when` 必须得出 true / false；表达式写错或不是布尔都算批次写错、直接停。
- 失败：按工具自己的 `is_error`（宿主给的结构化结果），或者 `expect` 不满足、`transform` 出错、
  超过 `timeout_ms`。`on_error: "continue"` 记下失败接着跑；并行分组看组上的 `on_error`。
- 引用写错（变量没定义、字段不存在、下标越界）一律停，错误里点名第几步、哪个引用。
- 模型应先用 `search_tool` 拿每颗工具的 `input_schema` 再拼 steps（已在上下文的回 `schema_in_context`）。
  下面的预校验只是兜底，不代替 `search_tool`。
- 跑之前整批校验：形状、`on_error` 拼写、工具在不在（`host.tool_schema`，按调用方可见的工具算）、
  必填参数有没有给、`timeout_ms` 范围、并行分组不嵌套且不超过 16 颗。有错整批拒绝，一步都不跑。
- 上限：总共 50 次调用；总 120 秒（步与步之间检查；单步用 `timeout_ms` 限时，最多 600000，超出报错）。
  超时只是没等到结果：bash 进程会被杀、MCP stdio 会发 `notifications/cancelled`，但调用可能已经执行、
  副作用可能已经发生。
  用户点停止时，正在跑的调用被中止、后面的不再跑。

## 输出

```json
{"ok":true,"completed":3,"skipped":0,"failed_step":null,"error":null,
 "results":{"win":{"...":"..."}},
 "trace":["#0.0 mcp_cua-driver__list_windows ok 140ms","#0.1 bash ok 140ms","#1 mcp_cua-driver__get_window_state ok 95ms"],
 "stopped":false,"elapsed_ms":250}
```

- `ok`：所有调用都成功或被 `when` 跳过；`completed`：成功的调用数；`skipped`：跳过的；
  `failed_step`：第一处失败的步下标。`trace` 每颗调用一行（并行分组记成 `#N.j`，耗时是整组的）。
- 中途停下（`stopped: true`）或整批被拒时，这次 `tool_batch` 调用标记为失败（`is_error`），
  内容仍是上面这份 JSON，看 `error` / `trace`。`on_error: continue` 跑完的批次不算失败，看 `ok`。
- `return` 里的名字没被设上（比如前面就停了）时值是 `null`，原因列在 `unset`。
- 工具回的图片不会带回（`trace` 里注明丢了几张）；要看界面用 `get_window_state` 的文本树。

## 示例：并行读几样，再按结果决定

```json
{
  "steps": [
    {"parallel": [
      {"tool": "mcp_cua-driver__list_windows", "args": {"app": "Mail"}, "save_as": "mail"},
      {"tool": "bash", "args": {"command": "git status --porcelain"}, "save_as": "dirty"},
      {"tool": "read_file", "args": {"target_file": "TODO.md"}, "save_as": "todo"}
    ]},
    {"tool": "bash", "args": {"command": "git diff --stat"}, "when": "dirty != \"\"", "save_as": "diff"}
  ],
  "return": ["mail", "diff", "todo"],
  "max_chars": 1500
}
```

cua-driver 各工具的参数名以 `search_tool` 查到的 schema 为准，上面只示意结构。**纯 cua** 的动作序列
更适合 cua-driver 自带的 `run_steps`（带 `observe`，在 driver 里跑、更快）或 `run_script`；
`tool_batch` 的价值在**跨工具**组合（cua + bash + 文件 + 浏览器 + 其它 MCP）。

## 测试

```bash
cargo test -p cordis-spine --test dynamic -- example_tool_batch plugin_
```

照上面的装法把插件拷进临时插件根、走启动时的磁盘加载（`boot_disk_from`），用工作区工具
（`bash` / `read_file` / `list_dir` / `grep`）真跑：链式 `save_as` + 路径引用、`expect` 失败即停、
`on_error: continue`、引用错点名、`max_chars` 截断、50 次上限与整批校验（含未知工具、缺必填参数）、
并行分组（真并发、组内失败）、`when` / `transform` / `timeout_ms`、每次调用逐条过权限门、
以调用方身份执行（计划模式、子代理审批身份、停止）。cua-driver 的部分没在测试里跑（CI 和 Linux 上没有 driver）。
