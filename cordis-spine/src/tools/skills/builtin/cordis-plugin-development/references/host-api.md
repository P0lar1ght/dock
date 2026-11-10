# host：工具、数据、密钥、日志

签名以 `cordis_inspect what: "builtins"` 为准。
界面相关（斜杠、面板、设置卡、状态栏）见 `ui.md`；`host.on` 见 `hooks.md`；
HTTP 与编解码见 `http.md`。

| 调用 | 要 `inject` | 说明 |
| --- | --- | --- |
| `host.register_tool(#{ name, description, parameters, execute, view? })` | `tools` | 注册一颗模型工具，见下 |
| `host.call_tool(name, args)` | `tools` | 调一颗活着的工具；`args` 是 map 或 JSON 字符串。bash 类工具照样弹权限。在 `execute` 里调时**以调用方身份**跑：那一页 / 那个子代理的审批档位、计划模式、只读场景、停止都算数。工具名不存在时回 `Error: unknown tool `x``（不会再把参数原样回显成成功） |
| `host.call_tool_ex(name, args)` | `tools` | 同 `call_tool`（同样的身份、门、未知工具报错），但回结构化结果 `#{ ok, is_error, text, images, tool }`：`is_error` 是工具自己的判定，`images` 是图片张数，`tool` 是实际跑的那颗。要分支判成败时用它，别去猜 `Error:` 前缀 |
| `host.list_tools()` | `tools` | 调用方能调的工具 `[#{ name, description }]`（按调用方的允许名单、能力档位筛；含 MCP 与插件工具；描述截到 200 字） |
| `host.tool_schema(name)` | `tools` | 一颗工具的 `#{ name, description, parameters }`，`parameters` 是 JSON Schema map；不认得或调用方调不了 → `()`。跑之前先查名字、核对必填参数用它 |
| `host.cancelled()` | — | 调用方这一轮被用户停止了吗。多步循环每步前查一下；脚本也会在下一个检查点被中止 |
| `host.provide(name, map)` | — | 把一张 map 挂成命名服务（RhaiBag）。重名失败；停包时撤掉 |
| `host.get(name)` | — | bag → map 的拷贝；活着的非 bag 服务 → `true`；没有 → `()` |
| `host.secret(name)` | — | 读密钥，见下 |
| `host.log(msg)` / `print(msg)` | — | 写日志，前缀 `[cordis:{pluginId}]` |

## `register_tool`

```rhai
host.register_tool(#{
    name: "memo_set",
    description: "Set the memo text",
    parameters: #{
        type: "object",
        properties: #{ text: #{ type: "string" } },
        required: ["text"]
    },
    execute: |args| { store.text = args.text; "ok" }
});
```

- `parameters` 是 **JSON Schema 的 map**，不是 JSON 字符串。
  根 `type` 为 `"object"`；`required` 里的名字必须在 `properties` 里。省略 = 空对象。
- `execute: |args|` 收到的是参数 map，返回字符串（别的类型会被转成字符串）。
  可选参数没传时取到的是 `()`，用 `args.x ?? 默认值`。
- 也可以返回 `#{ is_error: true, content: "…" }`（`content` 也可写成 `text`）：
  明确告诉模型这次失败了（同 MCP 的 `isError`），不必在文字前面拼 `Error:`。
  `is_error: false` 同理表示成功。缺这两个键的 map 照旧按 JSON 文本返回。
- 抛错 = 工具结果是一条 `Error: …`（标记为失败）。给用户能照着做的提示时，**返回**字符串或上面的 map，别抛。
- `view: |tc| #{ … }`：自定义这颗工具在终端和 GUI 的卡片，同 `register_tool_view`（见 `ui.md`）。

### 插件工具的可见性

- 不进采样工具表：模型用 `search_tool` 找、`use_tool` 调；
  挂上卸下都不改工具表，不打掉前缀缓存。
- 绕过预设的工具允许名单（能力档位照查）。
- 同一轮要验证就用 `cordis_call`（`name` + `arguments`），不要等下一轮或斜杠。
- 停包时一起撤掉；没有 `unregister_tool`，fiber 卸载就是撤销。

## `host.secret(name)`

- 查找顺序：`$DOCK_HOME/secrets.json`（扁平 `name → string`，`0600`）
  → 环境变量 `DOCK_SECRET_<NAME>`（大写，非字母数字换成 `_`）。
- 第一次用某个名字会弹 `secret {name}` 权限确认（不显示值）。
- 缺失或被拒都会**抛错**，不会返回空串。
- 名字：`[a-zA-Z][a-zA-Z0-9_-]{0,63}`。
- 只在运行期有：放进 `apply` / `execute`，不要写在顶层。
- 想让用户在设置页填，就声明设置卡里的 `secret` 字段（`ui.md`），
  它存在同一个 `secrets.json`、键名就是字段 `key`。

缺密钥时给出可操作的提示，而不是让模型反复撞同一个错：

```rhai
let token = ();
try { token = host.secret("github"); } catch (e) { }
if token == () {
    return "未配置 github 密钥：在设置页本插件的卡片里填，或 `/cordis set <插件 id> github <值>`";
}
```

## 边界

- 求值器不是安全边界，信任级别与 `bash` 相同；同步步骤有操作数上限。
- 脚本不能 `ctx.plugin`、不能加载磁盘模块、不能 `eval`。
- 不要在 `apply` 外面做进程级副作用。
