---
name: tool-batch
description: 一次调用顺序跑多个工具（tool_batch），以及桌面操作（cua-driver）少轮次、少截图的做法。要连续调好几个工具、或者操作桌面 App 时先读。
---

# tool_batch 与少轮次的桌面操作

每一轮来回都很贵：模型要重新读上下文、再出一次工具调用。顺序已知的一串操作，**一次调用做完**。

## 选哪个

| 场景 | 用 |
| --- | --- |
| 纯桌面动作序列（点、输、按键、等某个元素出现） | cua-driver `run_steps`，带 `wait_for` / `expect`，最后 `observe` 只回 diff |
| 有分支 / 循环 / 要按中间结果决定下一步 | cua-driver `run_script`（沙箱 JS，最多 500 次 driver 调用，只回结果） |
| 跨工具组合：cua + bash + 读写文件 + 浏览器 + 其它 MCP | `tool_batch` |
| 一次做多个只读查询再汇总（几个窗口状态、几个文件、几次 grep） | `tool_batch`，用 `return` 只拿要的几项 |
| 下一步完全取决于你对结果的判断 | 正常一步一调 |

`tool_batch` 是动态工具：先 `search_tool` 搜 `tool_batch`，再 `use_tool` 调。

## tool_batch 要点

- `steps[i] = { tool, args, save_as?, expect?, on_error? }`；工具名写全名（`mcp_cua-driver__click`）。
- 前面的结果用 `${name}` / `${name.a.b}` / `${name.items.0}` 引用；每步也自动叫 `step0`、`step1`…。
  整串只有一个引用时传原值（对象 / 数字原样）。
- 每步尽量写 `expect`（结果里必须出现的字），这样走偏了马上停，不会接着在错的窗口上乱点。
- `return` 只点名真正要看的变量，`max_chars` 压小（例如 1500）；中间的大结果不会进上下文。
- 串行执行；拿不到截图；每个要审批的工具（bash、cua 动作）仍会逐次弹审批——用户可以选「始终允许」。
- 结果第一行是 `Error: tool_batch stopped at step N` 时，看 `trace` 和 `error`，从那一步接着做，别整批重跑。

## cua 少截图

- 查窗口：`get_window_state` 带 `query`（要找的控件文字 / 角色）并设 `include_screenshot: false`，只拿文本树。
- 定位：按 role / name（如 button「保存」、text field「搜索」）找元素，**不要靠坐标**；坐标随窗口和缩放变。
- 断言：用 `expect`（tool_batch / run_steps）或 cua 的 `verify_state` 确认状态，不要截图再看。
- 真要看图（布局、颜色、验证码）才单独截一次图。

## 例 1：打开 App 并输入文字

```json
{
  "steps": [
    {"tool": "mcp_cua-driver__launch_app", "args": {"name": "TextEdit"}},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "TextEdit", "query": "text area", "include_screenshot": false},
     "expect": "text", "save_as": "win"},
    {"tool": "mcp_cua-driver__type_text", "args": {"app": "TextEdit", "text": "会议纪要：明天 10 点"}},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "TextEdit", "query": "会议纪要", "include_screenshot": false},
     "expect": "会议纪要：明天 10 点", "save_as": "check"}
  ],
  "return": ["check"],
  "max_chars": 1500
}
```

（参数名以 `search_tool` 查到的 cua-driver schema 为准。这一段是纯 cua，也可以改写成一次 `run_steps`。）

## 例 2：读几个窗口的状态并汇总

```json
{
  "steps": [
    {"tool": "mcp_cua-driver__list_windows", "args": {}, "save_as": "wins"},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "Safari", "query": "address bar", "include_screenshot": false},
     "save_as": "safari", "on_error": "continue"},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"app": "Mail", "query": "unread", "include_screenshot": false},
     "save_as": "mail", "on_error": "continue"},
    {"tool": "bash", "args": {"command": "date '+%H:%M'"}, "save_as": "now"}
  ],
  "return": ["safari", "mail", "now"],
  "max_chars": 800
}
```

只读查询各自 `on_error: "continue"`：某个 App 没开不影响其它几项。看 `trace` 知道哪项失败了。
