# host：工具、数据、密钥、日志

签名以 `cordis_inspect what: "builtins"` 为准。
界面相关（斜杠、面板、设置卡、状态栏）见 `ui.md`；`host.on` 见 `hooks.md`；
HTTP 与编解码见 `http.md`。

| 调用 | 要 `inject` | 说明 |
| --- | --- | --- |
| `host.register_tool(#{ name, description, parameters, execute, view? })` | `tools` | 注册一颗模型工具，见下 |
| `host.call_tool(name, args)` | `tools` | 调一颗活着的工具；`args` 是 map 或 JSON 字符串。在发起这次调用的那一页上执行，bash 类工具照样弹权限 |
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
- 抛错 = 工具结果是一条 `Error: …`。给用户能照着做的提示时，**返回**字符串，别抛。
- `view: |tc| #{ … }`：自定义这颗工具在终端和 GUI 的卡片，同 `register_tool_view`（见 `ui.md`）。

### 插件工具的可见性

- 不进采样工具表：模型用 `search_tool` 找、`use_tool` 调；
  挂上卸下都不改工具表，不打掉前缀缓存。
- 绕过预设的工具允许名单（能力档位照查）。
- reload 之后同一轮就能验证：`use_tool { tool_name: "memo_set", tool_input: { text: "x" } }`。
- reload 换版 / disable 时一起撤掉；没有 `unregister_tool`。

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

## 主动找 agent：`host.followup`

```rhai
host.followup("CI 红了：main 上 build #812 失败，看一下日志");   // 返回 "woke" 或 "queued"
host.followup(identity, "…");                                    // 指定送到哪一页（钩子载荷里有 identity）
```

- 送进那一页的信箱（和后台任务跑完通知同一个）：那一页**在跑**就在下一步带上；
  **闲着**的主会话页就叫醒它开一轮——不用用户再发话。
- 一页 5 秒内最多叫醒一次，多出来的排着等下一轮（返回 `"queued"`）。
- 不传 identity：发起这次脚本的那一页；定时器里调就是这颗插件最近打过交道的那一页。
- 模型看到的是「插件 X 送来的消息（不是用户发的消息）」。

## 定时器：`host.every` / `host.after`

```rhai
host.every(60, || { let r = http_request(#{ url: host.setting("status_url") }); … });
host.after(10, || { host.followup("十秒到了"); });
```

- `every` 最少 5 秒，`after` 最少 1 秒；都支持小数秒，最多 2592000 秒（30 天，覆盖日 / 周级任务并避免无意义的超长等待）。超限返回脚本错误，不占定时器名额。一次运行最多 16 个。插件停了 / reload 换版就取消。
- 回调不在任何一轮里：要让 agent 做事就 `host.followup`，要让用户看见就 `host.card` / `set_status`。

## 问模型一句：`host.llm`

```rhai
let verdict = host.llm(#{ system: "只回 SAFE 或 RISKY", prompt: "这条命令危险吗：" + cmd });
```

- 用**这一页**的模型，不带工具，不进会话日志，不计入 `/usage`。请求失败会抛错。
- 阻塞调用方：在钩子里调就卡住这一轮，写短。适合复审、分类、摘要、起标题。

## 跨重启的小存储：`host.load` / `host.save` / `host.forget`

```rhai
let n = (host.load("runs") ?? 0) + 1;
host.save("runs", n);
```

- 值是 JSON 能表示的（字符串、数字、布尔、数组、map）。整份 ≤ 1 MiB。
- 存在 `$DOCK_HOME/plugin-state/<id>-<目录指纹>.json`：不同项目里的同名插件各存各的；
  不写进项目目录（不会被提交）。
- reload、重启都在；删插件目录不会删它。

## 边界

- 求值器不是安全边界，信任级别与 `bash` 相同；同步步骤有操作数上限。
- 脚本不能 `ctx.plugin`、不能加载磁盘模块、不能 `eval`。
- 不要在 `apply` 外面做进程级副作用。
