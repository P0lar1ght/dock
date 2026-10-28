# 界面：斜杠、面板、视图、工具卡、设置卡、状态栏

每一类都要在 `inject` 里声明对应名字（见 `package.md` 的表）。
注册的东西都归这次 Run 的 fiber，停包或 `update` 时一起撤掉。

## 斜杠 `host.register_slash` — `inject: "slash"`

```rhai
host.register_slash(#{ command: "memo", kind: "slot", text: "memo", description: "打开便签" });
```

| `kind` | `text` 是 | 效果 |
| --- | --- | --- |
| `prompt` | 模板，`{args}` 换成用户敲的参数 | 发给模型；`send: false` 只填进输入框 |
| `overlay` | 正文 | 只读弹层，`title` 可选 |
| `slot` | 面板 id | 打开那个面板 |
| `tool` | 活着的工具名 | 不经模型直接跑工具，结果进 Notice |

- 只能新增：保留名（`/help`、`/quit`、`/agents` …）会在插入**前**失败。
  想让脚本先失败再做别的，就把 `register_slash` 放在 `apply` 最前面。
- `apply` 后面抛错会回滚前面已做的 `provide` / `register_*`。
- `kind: "tool"` 的参数：空 → `{}`；JSON 对象原样；其它 → `{"args": …}`。

## 面板 `host.register_slot` — `inject: "tui.slots"`

```rhai
host.register_slot(#{
    id: "memo",                    // 1–32 字符，a-z 开头，之后 a-z0-9_-
    title: "便签",
    hud: true,                     // 首行显示在快捷键栏
    render: || { store.text },     // 纯文本
    on_key: |key| { if key == "esc" { "close" } else { () } },
    actions: [#{ id: "clear", label: "清空" }],
    view: || #{ type: "stack", children: [ … ] }
});
```

- `render` / `on_key` 在宿主上同步执行，要快。
- `on_key(key)` 的 key：`esc` / `enter` / `up` / `down` / `char:x`，
  或 `actions` / 视图按钮的 id。返回 `"close"` 关面板。没写 `on_key` 时 Esc 照样能关。
- `actions`：GUI 显示成按钮，点了调 `on_key(id)`；终端用户照旧按键。
- `host.open_slot(id)`：让终端打开这个面板。
- `host.slot_changed(id)`：正文变了，GUI 重新拉取（终端每帧都重画）。

### 视图树 `view`

有 `view` 时终端和 GUI 都画它，不画 `render()`。返回 `()` 表示这次用 `render()`。
节点类型：`text` / `markdown` / `kv` / `table` / `list` / `badge` / `progress` /
`button` / `link` / `section` / `row` / `stack` / `empty` 等，完整字段见仓库
`docs/PLUGIN-VIEWS.md`（dock.view.1）。`button` 的 `action` 会调 `on_key(action)`，
终端里用数字键 1–9。

```rhai
view: || #{ type: "stack", children: [
    #{ type: "kv", items: [#{ label: "条数", value: store.items.len() }] },
    #{ type: "button", label: "清空", action: "clear" }
] }
```

## 工具卡 `host.register_tool_view` — `inject: "tool.views"`

```rhai
host.register_tool_view("memo_set", |tc| #{ type: "text", text: "便签：" + tc.arguments.text });
```

- 能画任何工具的卡（自己的或内置的），终端和 GUI 都用。
- `tc` 是 `#{ name, arguments, output, failed }`；`arguments` 能解析时是 map。
  `call` 是 Rhai 保留字，别拿它当变量名。
- 返回 `()` = 这次用默认卡。只影响界面，不进模型上下文。

## 设置卡 `host.register_settings` — `inject: "plugin.settings"`

```rhai
host.register_settings(#{ title: "GitHub", fields: [
    #{ key: "repo", type: "string", label: "默认仓库", "default": "owner/name" },
    #{ key: "limit", type: "number", min: 1, max: 50, "default": 10 },
    #{ key: "github", type: "secret", label: "Token" }
] });
let limit = host.setting("limit");
```

- GUI 设置页和终端 `/cordis set <插件 id> <key> <值>` 按它生成表单并校验。
- `type`：`string` `text` `number`（`min` `max`）`boolean` `select`（`options`）`secret`。
- `default` 是 Rhai 关键字，**要加引号**写成 `"default"`。
- `secret` 字段存进 `secrets.json`、键名是 `key`，用 `host.secret(key)` 读，`host.setting` 读不到。
- `host.setting(key)`：已存的值 → 否则 `default` → 否则 `()`。
- 每个插件一张卡。

## 状态栏 `host.set_status` — `inject: "status.items"`

```rhai
host.set_status(#{ id: "memo-count", text: "3 条", tone: "accent", surface: "memo" });
host.clear_status("memo-count");
```

- 第一次调用登记，之后同 id 就是更新。
- `tone`：`default` `muted` `accent` `success` `warning` `danger`。
- `surface`：点击时打开的面板 id。`tooltip` 可选。
- `text` 要短，客户端只截断不改写。
