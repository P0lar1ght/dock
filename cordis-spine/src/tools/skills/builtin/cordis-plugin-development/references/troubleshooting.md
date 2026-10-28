# 报错对照

| 报错 / 现象 | 先查 |
| --- | --- |
| `rhai syntax: …` | Rhai 语法，见 `rhai-pitfalls.md`；定义失败不发 id |
| `source must be a map` / `needs apply as a function` | 文件求值结果必须是 `#{ …, apply: \|host\| { … } }` |
| `rhai define: …` | 顶层调了运行期才有的函数（`http_request`、`host.secret`、编解码），挪进 `apply` |
| `source_path` 越界 / 带 `..` | 路径必须在 `.dock/plugins/<id>/` 或 `~/.dock/plugins/<id>/` 下 |
| 同时给了 `source` 和 `source_path` | 只给一个 |
| `plugin is required` / `plugin.pluginId is required` | `plugin` 是对象：`{ kind: "new", idPrefix }` 或 `{ kind: "existing", pluginId }` |
| `has been registered` / already registered | 另一个**正在跑**的包占着这个名字，`cordis_stop` 它 |
| `unknown factory` | `cordis_inspect what: "factories"` |
| `invalid-mode` / 提示用 `update` | 对照 `lifecycle.md` 的 mode 表 |
| fiber `[pending]` / `waiting for` | `inject` 的名字还没挂上（合法状态）。`[active]` 就是在跑，有问题看 `hostError` |
| `no dynamic plugin "…" in this process` | id 被丢了、重启丢了，或盘上源码坏了没加载上；见 `lifecycle.md` 最后一条 |
| `Function not found: …` | 见 `rhai-pitfalls.md`；带 `(())` 说明参数是空值 |
| `密钥不存在：…` | 让用户在设置页或 `/cordis set` 填；插件里用 `try` 兜住并返回提示（`host-api.md`） |
| HTTP 403 / 401 | 多半是密钥无效或没开通权限，告诉用户，别改代码重试 |
| 请求被拦（内网 / 回环） | SSRF 规则，见 `http.md` |
| 权限被拒 | 不要重复申请同一次 `cordis_run`，除非用户要求 |
| 工具在 `search_tool` 里搜不到 | 包没在跑（`cordis_inspect what: "temporary"`），或名字写错 |
| 重启后没了 | 会话插件只在内存里：`cordis_promote` 或直接写在 `.dock/plugins/<id>/` |
| 想写 JS / JSX / Wasm / 浏览器端 | Dock 不支持，用 `factory: "rhai"` 或静态插件 |
