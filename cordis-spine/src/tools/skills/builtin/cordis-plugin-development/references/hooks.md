# host.on：观察会话、在回合里插提醒

只有三个事件能挂，别的 waterfall 脚本够不着。不需要 `inject`。

| 事件 | 时机 | 回调收到 | 返回 |
| --- | --- | --- | --- |
| `session/event` | 会话日志追加之后，异步 | 一行文本，见下 | 忽略 |
| `agent/step-start` | 回合里**每次采样之前** | `#{ step, identity, main }` | 字符串 = 注入一条 `<system-reminder>`；`()` = 没意见 |
| `agent/turn-end` | 回合想结束时 | `#{ text, rounds, ended_with_text, queued_followups, identity, main }` | 字符串 = 注入提醒并让回合继续；`()` = 让它结束 |

## `session/event`

```rhai
host.on("session/event", |line| {
    if line.starts_with("turn-end\t") { store.turns += 1; }
});
```

`line` 的形状：`user\t…` / `assistant\t…` / `tool\tname` / `reminder\t…` /
`turn-end\tcompleted|cancelled|failed`（每个结束的回合一条）。

- 只观察，不是拦截。
- 回调里不要再写 `Sessions`。

## `agent/step-start`

- `step` 从 0 数，turn-end 续跑时继续累加；所以「每回合一次」的状态在 `step == 0` 时重置。
- `main` 为 `false` 是子代理的回合。

## `agent/turn-end`

- 用户已经排了下一条消息时，宿主**整个跳过**这个钩子（所以 `queued_followups` 在这里总是 `false`）。
- 子代理的回合也会调：提醒落在那个子代理自己的记录里。只管主会话就看 `main`。

## 两个拦截钩子的规矩

- 它们在回合里**同步**执行，会卡住采样直到返回；写短，别在里面 `host.call_tool`。
- 抛错 = 没意见（记日志，回合照常）。返回非字符串（数字、map）也当没意见，不会被转成文字塞给模型。
- 链由宿主往下传，脚本吞不掉；脚本也不能自己往会话里写，只能返回文字由循环写入。
- 内置的优先：`tool-todo`（10）、`tool-goal`（20）排在脚本（50）前面，同一轮都想说话时它们先。
