# Package：脚本外形与目录

## 目录

插件目录名就是插件 id（`[a-z][a-z0-9-]{1,31}`）：

```
.dock/plugins/<id>/source.rhai    项目级（覆盖同名用户级）
.dock/plugins/<id>/plugin.toml    可选
.dock/plugins/<id>/panel.html     可选：web 面板（见 ui.md）
~/.dock/plugins/<id>/…            用户级，所有项目都加载
```

`plugin.toml` 每个键都可选；整个文件也可以没有（名字就用目录名）：

```toml
name = "便签"                 # 给人看的名字，默认目录名
purpose = "会话里的一张便签"    # 一句话用途，设置页和 /cordis 显示
enabled = true                # false = 启动时不装；cordis_plugin reload 会改回 true

[permissions]
# 这些 host 的 http_request 不再逐次确认（批准加载插件时会列给用户看）。
# 只写主机名：api.example.com 或 *.example.com（只配子域），不许光一个 *。
http = ["api.github.com"]
```

认不得的键不挡加载，但 reload 回执会提醒（`[permission]` 少个 s 这种错不会被静默吞掉）。
写好用 `cordis_plugin reload <id>` 装上；`source.rhai` 上限 1 MiB。

## 脚本外形

整个文件求值成一张 map：

```rhai
#{
    inject: ["tools"],          // 可省，省了就是 ["tools"]
    apply: |host| {
        // 在这里 register_* / provide
    }
}
```

- `apply` 必须是函数，`run` 时才执行。
- 顶层可以先写 `fn helper(x) { … }`，再写这张 map。
  顶层 `fn` **不捕获**外面的变量，要用 `host` 就当参数传进去。
- 顶层不能调 `http_request` / `host.secret` / 编解码函数：编译检查时的引擎没有它们。
  放进 `apply` 或 `execute` 里。
- reload 时先编译并求值一次顶层（拿到那张 map），再调 `apply`。

## `inject` 是启动闸门，不是授权

内核按名字解析 `inject`：名字挂上了 fiber 才启动，没挂上就一直 `pending`。
但能挂上 ≠ 脚本能用。脚本**用得上**的只有：

| 名字 | 解锁 |
| --- | --- |
| `tools` | `register_tool` / `call_tool` |
| `slash` | `register_slash` |
| `tui.slots` | `register_slot` / `open_slot` / `slot_changed` |
| `tool.views` | `register_tool_view` |
| `plugin.settings` | `register_settings` / `setting` |
| `status.items` | `set_status` / `clear_status` |
| 别的包 `host.provide` 的名字 | `host.get` 拿到一份拷贝 |

`inject: ["sessions"]` 能启动，但 `host.get("sessions")` 回 `()`，没有方法可调。
要用某个内置能力，就 `host.call_tool` 调它的模型工具（权限门照走）；要成败和解析好的 JSON
用 `host.call_tool_result`（`host-api.md`）。MCP 服务器的工具一样能调。
`cordis_inspect what: "services"` 把「够得着」和「只是挂着」分两段列。

## 状态放哪

可变状态放在 `apply` 里的变量，被 `execute` / `render` / `on_key` 闭包共享：

```rhai
apply: |host| {
    let store = #{ items: [] };
    host.register_tool(#{ …, execute: |args| { store.items.push(args.text); store.items.len() } });
}
```

- 闭包按引用共享，改了大家都看得见。
- `host.provide(name, map)` 只是把一份快照挂出去给别的包读，`host.get` 拿到的是拷贝，写不回去。
- 状态只活在这次运行里；reload / disable / 重启都会清掉。
  要跨重启，写文件或用设置卡（`ui.md`）。

## 预置工厂

`plugin.toml` 里写 `factory = "…"` 可以不用 Rhai，换成固定的 Rust 实现。只有一个
实用：`slash`——只加一条 `/命令`：

```toml
factory = "slash"
command = "standup"
kind = "prompt"          # prompt | overlay | slot | tool
text = "写站会：{args}"
```

`echo` / `note` / `hold` 只用来教学和回归，不写业务逻辑。其余一律 Rhai（默认）。
