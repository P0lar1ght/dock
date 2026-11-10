---
name: tool-batch
description: 少来回地调工具：一轮里并行发多个独立调用、用 tool_batch 一次跑一串（可并行分组、带条件），以及桌面操作（cua-driver）少轮次、少截图的做法。要连续调好几个工具、或者操作桌面 App 时先读。
---

# 少来回地调工具

每一轮来回都很贵：模型要重新读上下文、再出一次调用。能一次做完的，一次做完。

## 选哪个

| 场景 | 用 |
| --- | --- |
| 几个互不依赖的查询（读几个文件、几处搜索、几条只读命令、几个只读 MCP 查询） | **同一次回复里一起发**，Dock 会并发执行 |
| 纯桌面动作序列（点、输、按键、等某个元素出现） | cua-driver `run_steps`，加 `observe` 只回变化 |
| 桌面上有分支 / 循环 / 重试 | cua-driver `run_script`（只回结果） |
| 跨工具的一串操作（cua + bash + 文件 + 浏览器 + 其它 MCP），或者查完要筛选 / 汇总，不想让中间结果进上下文 | `tool_batch` |
| 下一步完全取决于你对结果的判断 | 正常一步一调 |

有依赖（后一个的参数要用前一个的结果）就别放在同一次回复里，也别用占位符猜参数。

`tool_batch` 是动态工具：先 `search_tool` 搜 `tool_batch`，再 `use_tool` 调。

**先拿 schema 再拼 steps**：批里要用的每颗工具，先用 `search_tool` 查到它的 `input_schema`
（本会话里已经拿过的会回 `schema_in_context: true`，不用重查），照着写 `args`。`tool_batch`
跑之前的校验只查工具名和必填参数，是兜底，不能代替 `search_tool`。

## tool_batch 要点

- `steps[i]` 是一个调用 `{ tool, args?, save_as?, expect?, when?, transform?, timeout_ms?, on_error? }`，
  或者一个并行分组 `{ parallel: [调用, …], on_error? }`。工具名写全名（`mcp_cua-driver__click`）。
- 互不依赖的读放进 `parallel`；写文件、bash、桌面动作这类有先后的按顺序排（放进 `parallel`
  宿主也会一颗颗按顺序跑，不会真并发）。
- `timeout_ms` 到点只表示没等到结果：那颗调用可能已经执行了（写了文件、点了按钮），别直接重跑。
- 前面的结果用 `${name}` / `${name.a.b}` / `${name.items.0}` 引用；每步也自动叫 `step0`、`step1`…。
- `when: "hits.len() > 0"` 条件跳过；`transform: "result.items.pluck(\"id\")"` 只留要的部分
  （表达式里不能写闭包 `|x| …`；取字段用 `pluck(xs, "字段")`，按字段等值过滤用 `filter_eq(xs, "字段", 值)`；
  也不能写 `xs.map("函数名")` 或 `sleep`，数组最多 65536 个元素）。
- 每步尽量写 `expect`（结果里必须出现的字），走偏了马上停。
- `return` 只点名真正要看的变量，`max_chars` 压小（例如 1500）。
- 跑之前会整批校验工具名和必填参数；批次被拒或中途停下时这次调用标记为失败，看 `error` 和 `trace`，
  从失败那一步接着做，别整批重跑。
- 每个要审批的工具（bash、cua）仍会逐次弹审批；用户可以选「始终允许」。

## cua 少截图

- 查窗口：`get_window_state` 带 `query`（要找的控件文字 / 角色）并设 `include_screenshot: false`，
  只拿文本树；动作之后重读用 `since: "latest"`，只回变化。
- 定位：按 role / name（如 button「保存」、text field「搜索」）找元素，**不要靠坐标**。
- 断言：用 `expect`（tool_batch / run_steps）或 `verify_state` 确认状态，不要截图再看。
- 不同窗口的 `get_window_state`、`list_windows` 可以同一轮一起发；**同一个窗口**别并发读两次
  （新快照会让旧快照的 element_token 失效）。
- 真要看图（布局、颜色、验证码）才单独截一次图。

## 例：并行查几样，再按结果做下一步

```json
{
  "steps": [
    {"parallel": [
      {"tool": "mcp_cua-driver__list_windows", "args": {"app": "Safari"}, "save_as": "safari"},
      {"tool": "mcp_cua-driver__list_windows", "args": {"app": "Mail"}, "save_as": "mail"},
      {"tool": "bash", "args": {"command": "date '+%H:%M'"}, "save_as": "now"}
    ], "on_error": "continue"},
    {"tool": "mcp_cua-driver__get_window_state",
     "args": {"pid": "${safari.windows.0.pid}", "window_id": "${safari.windows.0.window_id}",
              "query": "address", "include_screenshot": false},
     "when": "safari.windows.len() > 0", "save_as": "addr"}
  ],
  "return": ["addr", "now"],
  "max_chars": 800
}
```

（参数名、结果字段以 `search_tool` 查到的 cua-driver schema 和实际返回为准，这里只示意结构。）
