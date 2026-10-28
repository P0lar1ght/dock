# Package：脚本外形与目录

## 目录

插件目录名就是稳定的 `pluginId`（`[a-z][a-z0-9-]{1,31}`）：

```
.dock/plugins/<id>/plugin.toml    项目级（覆盖同名用户级）
.dock/plugins/<id>/source.rhai
~/.dock/plugins/<id>/…            用户级，所有项目都加载
```

`plugin.toml`：

```toml
name = "便签"
purpose = "会话里的一张便签"
factory = "rhai"
enabled = true      # false = 启动时不自动跑
```

启动时自动加载，不弹权限确认（工作区信任）。`/cordis` 列出磁盘层和会话层。

## `cordis_define` 字段

- `plugin`：`{ kind: "new", idPrefix }`（3–6 个小写字母，宿主发 `prefix-N`）
  或 `{ kind: "existing", pluginId }`（给已有插件追加一个 Package）。
- `name`、`purpose`：给人看的名字和用途。
- `factory`：自定义行为一律 `"rhai"`。
- `source_path` **或** `source`，二选一：

| 输入 | 什么时候用 | 上限 | 存法 |
| --- | --- | --- | --- |
| `source_path` | 默认 | 1 MiB | 只记路径，`run` / `update` 时重读文件 |
| `source` | 一两行的小样 | 128 KiB | 存在 Package 里 |

`source_path` 必须落在 `.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/` 下，
不能带 `..`。项目根是调用工具的那个会话的工作目录。

`define` 会编译并求值一次顶层（拿到那张 map），**不调 `apply`**、不弹确认、
不动 `currentPackageId`。语法错不会发 id。

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
- 顶层不能调 `http_request` / `host.secret` / 编解码函数：define 期的引擎没有它们。
  放进 `apply` 或 `execute` 里。

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
要用某个内置能力，就 `host.call_tool` 调它的模型工具（权限门照走）。
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
- 状态只活在这次 Run 里；`update` / `stop` / 重启都会清掉。
  要跨重启，写文件或用设置卡（`ui.md`）。

## 预置工厂

`echo` / `note` / `hold` / `slash` 是固定的 Rust 实现，用来教学和回归，不写业务逻辑：

- `echo`：挂 `dynEcho`，注册 `dyn_echo` 工具。
- `note`：挂一个内存字符串 `dynNote`。
- `hold`：`inject` 一个没人提供的名字，演示 `pending`。
- `slash`：加一条 `/命令`；`command` / `kind` / `text` 写在 `cordis_define` 参数里。

拿不准有哪些就 `cordis_inspect what: "factories"`，别编。
