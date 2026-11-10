# 批量调工具 tool-batch（示例插件 · 模型工具）

一颗 Rhai 磁盘插件，注册模型工具 **`tool_batch`**：一次调用按顺序跑一串工具（Dock 内置、
MCP 如 `mcp_cua-driver__*`、别的插件工具），中间结果存在插件里的变量表，**不进模型上下文**，
最后只回 `return` 点名的几项（每项截断到 `max_chars`）。目标是把「定位 → 点击 → 输入 → 确认」
这类十几轮来回压成一轮。

不改核心源码，只用 `host.call_tool`；不联网、不读写文件、不用 `eval`。
宿主（Rhai host）还缺的能力和这里的绕法见 [`HOST_GAPS.md`](HOST_GAPS.md)。

## 装上

```bash
mkdir -p ~/.dock/plugins
cp -R examples/plugins/tool-batch ~/.dock/plugins/tool-batch
# 让模型知道什么时候用（可选，推荐）
mkdir -p ~/.dock/skills/tool-batch
cp examples/plugins/tool-batch/SKILL.md ~/.dock/skills/tool-batch/SKILL.md
```

重启 Dock（`plugin.toml` 里 `enabled = true`，启动时自动加载）。`/cordis` 里能看到「批量调工具」。
`tool_batch` 是动态工具，不进采样表：模型用 `search_tool` 搜 `tool_batch`（归 `dynamic` 组），
再用 `use_tool` 调；同一轮里手动试可以用 `cordis_call`。

## 输入

```jsonc
{
  "steps": [
    {
      "tool": "工具全名，如 bash / read_file / mcp_cua-driver__get_window_state",
      "args": { "...": "字符串里可写 ${var} / ${var.a.b} / ${var.items.0}（-1 = 最后一个）" },
      "save_as": "可选，结果变量名（不能含点）",
      "expect": "可选，结果文本必须包含的子串，不含就算失败",
      "on_error": "stop（默认）| continue"
    }
  ],
  "return": ["要返回的变量名或路径，如 win 或 win.title；省略 = 最后跑的那一步"],
  "max_chars": 4000
}
```

- 每步结果自动存成 `step0`、`step1`…，`save_as` 再多存一个名字。
- 结果文本是 JSON 对象 / 数组就解析后存（能按路径取字段），否则存字符串。
- 引用：整串**恰好**是一个 `${...}` 时代入原值（对象 / 数组 / 数字原样传给下一个工具）；
  夹在文字里时拼成文本，非字符串值拼成 JSON。没有转义写法，字面量 `${` 写不出来。
- 失败：工具结果以 `Error` / `error` / `exit ` / `已中断。` / `权限被拒绝` / `计划模式已阻止` /
  `只读模式：` / `MCP 工具未启用或已关闭` / `当前 Agent 预设未包含此工具` 开头，或者 `expect`
  不满足。`on_error: "continue"` 记下失败接着跑。
- 引用写错（变量没定义、字段不存在、下标越界）一律停，错误里点名第几步、哪个引用。
- 形状不对（没有 steps、超过 50 步、`on_error` 拼错、嵌套调用 `tool_batch`…）整批拒绝，一步都不跑。
- 上限：50 步；总 120 秒（只在步与步之间检查，正在跑的那一步打断不了）。

## 输出

```json
{"ok":true,"completed":3,"failed_step":null,"error":null,
 "results":{"win":{"...":"..."}},
 "trace":["#0 mcp_cua-driver__launch_app ok 812ms","#1 mcp_cua-driver__type_text ok 95ms","#2 mcp_cua-driver__get_window_state ok 140ms"],
 "stopped":false,"elapsed_ms":1050}
```

- `ok`：所有步都成功；`completed`：成功的步数；`failed_step`：第一处失败的下标。
- 中途停下（`stopped: true`）或整批被拒时，第一行是 `Error: tool_batch stopped at step N: …`
  / `Error: tool_batch rejected: …`，第二行才是 JSON——这样宿主把这次调用标成失败
  （插件没法直接设 `is_error`，见 HOST_GAPS.md）。
- `on_error: continue` 跑完的批次即使有失败也不带 `Error:` 前缀，看 `ok` / `failed_step` / `trace`。
- `return` 里的名字没被设上（比如前面就停了）时值是 `null`，原因列在 `unset`。

## 最小示例（cua）

```json
{
  "steps": [
    {"tool": "mcp_cua-driver__launch_app", "args": {"name": "TextEdit"}},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "TextEdit", "query": "text area", "include_screenshot": false}, "save_as": "win"},
    {"tool": "mcp_cua-driver__type_text", "args": {"app": "TextEdit", "text": "hello from tool_batch"}},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "TextEdit", "query": "hello from tool_batch", "include_screenshot": false},
     "expect": "hello from tool_batch", "save_as": "check"}
  ],
  "return": ["check"],
  "max_chars": 1500
}
```

cua-driver 各工具的参数名以 `search_tool` 查到的 schema 为准，上面只示意结构。
这种**纯 cua** 的序列其实更适合 cua-driver 自带的 `run_steps`（带 `wait_for` / `expect` /
`observe`，在 driver 里跑、更快）；`tool_batch` 的价值在**跨工具**组合（cua + bash + 文件 + 浏览器）。

## 测试

```bash
cargo test -p cordis-spine --test dynamic -- example_tool_batch
```

照上面的装法把插件拷进临时插件根、走启动时的磁盘加载（`boot_disk_from`），用工作区工具
（`bash` / `read_file` / `list_dir` / `grep`）真跑：链式 `save_as` + 路径引用、`expect` 失败即停、
`on_error: continue`、引用错点名、`max_chars` 截断、50 步上限与整批校验、每次调用逐条过权限门。
cua-driver 的部分没在测试里跑（CI 和 Linux 上没有 driver）。
