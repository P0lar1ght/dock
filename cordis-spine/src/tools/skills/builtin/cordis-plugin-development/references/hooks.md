# host.on：观察会话、插提醒、拦流程

两档权限，**不是所有插件都能挂全部事件**：

| 档 | 事件 | 谁能用 |
| --- | --- | --- |
| 观察 + 插提醒 | `session/event`、`agent/step-start`、`agent/turn-end` | 任何插件 |
| 拦流程 | `agent/pre-step`、`system-prompt/assemble`、`tools/pre-execute`、`tools/execute`、`llm/stream` | **只给磁盘来源插件** |

「磁盘来源插件」= 插件目录装上 / 设置里启用 / 审批过的 `cordis_promote` 落盘。
会话里 `cordis_define` 的包**打不开**后五个：否则模型能给自己开闸门（拦自己后续的
工具调用）、能改自己的系统提示 —— 那是自我提权。挂了会报一句 `only available to
disk plugins`。

判据是**这个包是不是从磁盘加载的**，不是它有没有传 `source_path`：会话里 define
的包也能带 `source_path`（工具说明里还推荐这么写），拿它当判据等于把闸门开着。
要试这五个钩子，走 `cordis_promote` 落盘 —— 它会立刻从磁盘 autostart，不用重启
dock；下次启动也会自动加载。

## 通用三事件

不需要 `inject`。

| 事件 | 时机 | 回调收到 | 返回 |
| --- | --- | --- | --- |
| `session/event` | 会话日志追加之后，异步 | 一行文本，见下 | 忽略 |
| `agent/step-start` | 回合里**每次采样之前** | `#{ step, identity, main }` | 字符串 = 注入一条 `<system-reminder>`；`()` = 没意见 |
| `agent/turn-end` | 回合想结束时 | `#{ text, rounds, ended_with_text, queued_followups, identity, main }` | 字符串 = 注入提醒并让回合继续；`()` = 让它结束 |

### `session/event`

```rhai
host.on("session/event", |line| {
    if line.starts_with("turn-end\t") { store.turns += 1; }
});
```

`line` 的形状：`user\t…` / `assistant\t…` / `tool\tname` / `reminder\t…` /
`turn-end\tcompleted|cancelled|failed`（每个结束的回合一条）。

- 只观察，不是拦截。
- 回调里不要再写 `Sessions`。

### `agent/step-start`

- `step` 从 0 数，turn-end 续跑时继续累加；所以「每回合一次」的状态在 `step == 0` 时重置。
- `main` 为 `false` 是子代理的回合。

### `agent/turn-end`

- 用户已经排了下一条消息时，宿主**整个跳过**这个钩子（所以 `queued_followups` 在这里总是 `false`）。
- 子代理的回合也会调：提醒落在那个子代理自己的记录里。只管主会话就看 `main`。

## 五个流程钩子（磁盘插件）

### `agent/pre-step` —— 拒掉一整轮

载荷 `#{ user, identity, main }`。

```rhai
host.on("agent/pre-step", |s| {
    if s.user.contains("别动生产") { #{ deny: "这一轮不接生产相关的活" } } else { () }
});
```

返回 `#{ deny: "理由" }`（或一个裸字符串）= 这一轮不开始，理由写进会话的
system-reminder，模型看得见为什么没跑。`()` = 没意见。

注意：用户那条消息**已经记进会话了**，拒绝只是不往下走。

### `system-prompt/assemble` —— 往系统提示加一段

载荷 `#{ tools }`：当前在册的工具名（字典序，含按需工具与 MCP 工具）。

```rhai
host.on("system-prompt/assemble", |p| {
    "本项目的约定：改动前先跑 cargo test -p cordis-spine。"
});
```

返回字符串 = 加一段；`()` = 没意见。

- 段落进固定槽位（`ORDER_HOOKS`），**按插件 id 排序** → 提示词字节稳定，前缀缓存不作废。
- 同一个插件的段只写一次（第一次写赢）。
- 只能**加段**：不能改 base，不能删别人的段。

### `tools/pre-execute` —— 拦一次工具调用

载荷 `#{ called, name, arguments, identity, main }`（`called` 是模型写的名字，
`name` 是改道后真正要跑的那个）。

```rhai
host.on("tools/pre-execute", |c| {
    if c.name == "bash" { #{ deny: "这个项目不让跑 shell" } }
    else if c.name == "write_file" { #{ rewrite: #{ name: "write_file", arguments: c.arguments } } }
    else { () }
});
```

| 返回 | 效果 |
| --- | --- |
| `()` | 没意见 |
| `#{ deny: "理由" }` | 拒绝这次调用，理由回到模型 |
| `#{ rewrite: #{ name, arguments } }` | 改道到另一颗工具（`arguments` 可以是字符串或 map） |

规矩：

- **拒绝是单调的**：先拒的赢，后来的处理器不能把它翻案。拒绝之后再改道无效。
- 改道之后按**新名字**重过允许名单、计划门、权限门 —— 改道不能绕过门。
- 拒绝发生在权限门**之前**：脚本不让跑的东西，模型连问都不会问用户。

### `tools/execute` —— 事后改结果

载荷 `#{ name, call_id, content, is_error }`。

```rhai
host.on("tools/execute", |r| {
    if r.name == "bash" && r.content.len() > 4000 { r.content.sub_string(0, 4000) + "…（已截断）" }
    else { () }
});
```

返回字符串 = 替换模型看到的内容；`()` = 不动。

Rhai 没有 `slice`（字符串截取用 `sub_string(start, len)`）；写错函数名会在钩子里抛错，
而钩子抛错 = 当没意见（fail-open）——**钩子静默失效**，不会报错给你看。

这是**事后**：工具已经跑过了，副作用已经发生，这里只改模型看到的那份文本。

### `llm/stream` —— 改写采样输出

载荷 `#{ text, tool_calls }`（`tool_calls` 是工具名数组）。

```rhai
host.on("llm/stream", |o| {
    if o.text.contains("内部代号 XYZ") { o.text.replace("XYZ", "客户名") } else { () }
});
```

返回字符串 = 替换助手文本；`()` = 不动。

只能改**输出**。请求那一侧（系统提示、历史、工具表）不在这个钩子里 ——
要改请求，用 `system-prompt/assemble` 那条路。

## 全部钩子的规矩

- 它们在回合里**同步**执行，会卡住采样直到返回；写短，别在里面 `host.call_tool`。
- 抛错 = 没意见（记日志，回合照常）。返回类型不对（该字符串的返回了 map 等）也当没意见。
- 链由宿主往下传，脚本吞不掉；脚本也不能自己往会话里写，只能返回文字由循环写入。
- 内置的优先：`tool-todo`（10）、`tool-goal`（20）排在脚本（50）前面，同一轮都想说话时它们先。
- 停掉插件（`cordis_stop`）会连监听一起摘掉。

## 实测踩过的坑

以下是在真实 dock（完整应用树 + 磁盘插件 autoload）上验出来的，写脚本前先看一遍：

- **钩子抛错 = 静默当没意见**。函数名写错、类型不匹配，宿主只记一行 stderr，钩子
  当没发生 —— 表现就是「插件明明写了却不生效」。字符串截取用
  `sub_string(start, len)`，**Rhai 没有 `slice`**。
- **钩子作用于所有工具结果**，包括 `cordis_inspect` 这类诊断工具。调试时先按
  `r.name` 缩小范围，否则你自己的诊断输出会被替换掉，看不出问题在哪。
- **改磁盘源文件后，要重启 _current_ 那个包**：`cordis_inspect what:"permanent"` 看
  `current` 的包 id，`cordis_run` 用它重启才会重读磁盘文件。传别的包 id 是
  **回滚**到那个旧版本（promote 时的内存快照），不是重读 —— 很容易以为「改了没用」。
- **waterfall 链不能被掐断**：宿主插件的监听若不把控制权交给下一环，排在它后面的
  监听者（包括脚本钩子）全都拿不到这次结果 —— 表现同样是「钩子不生效」，而且不
  报错。
- **拒绝发生在权限门之前**：脚本拒掉的调用，用户连询问都看不到。改道之后按**新名字**
  重过允许名单、计划门、权限门，改道绕不过门。
- **子代理的回合也走这些钩子**：载荷里的 `main` 为 `false`。只想管主会话就判 `main`。
- **`agent/pre-step` 拒绝的是整轮**：用户那条消息已经记进会话了，拒绝只是不往下走，
  理由写进会话的 system-reminder，模型看得见为什么没跑。
