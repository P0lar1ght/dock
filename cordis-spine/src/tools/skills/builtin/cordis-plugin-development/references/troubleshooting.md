# 报错对照

reload 回执里已经带了原因和「下一步」；照它做。这张表是补充。

| 报错 / 现象 | 先查 |
| --- | --- |
| `没找到插件目录 <id>` | 目录名就是 id；先把 `source.rhai` 写进回执列出的那个路径 |
| `插件 id … 不合法` | 2–32 位，小写字母开头，只含 `a-z0-9-` |
| `rhai syntax: …` | Rhai 语法，见 `rhai-pitfalls.md`；按行号改，再 reload |
| `source must be a map` / `needs apply as a function` | 文件求值结果必须是 `#{ …, apply: \|host\| { … } }` |
| 顶层调用报 `Function not found` | 顶层调了运行期才有的函数（`http_request`、`host.secret`、编解码），挪进 `apply` |
| `源文件要在插件目录 … 里` | `source.rhai` 是指向目录外的软链；放真文件 |
| `source.rhai exceeds 1MiB` | 把大块数据挪出脚本（运行时 `host.read_bytes` 读），或拆成两个插件 |
| `has been registered` / already registered | 另一个**正在跑**的插件占着这个名字：换个名字，或 `cordis_plugin disable` 那个插件 |
| `已经装着同名插件 …` | 两层（项目 / 用户）有同 id 的目录且另一份在跑：先 disable 那一份（`scope` 指到它那层） |
| 回执「在等依赖」 | `inject` 的名字还没挂上（合法状态）。写了没有的名字就删掉再 reload |
| 回执「上一版仍在运行（已自动回滚）」 | 新版没起来，用户手上还是旧版；按错误改，再 reload |
| `Function not found: …` | 见 `rhai-pitfalls.md`；带 `(())` 说明参数是空值 |
| `密钥不存在：…` | 让用户在设置页或 `/cordis set` 填；插件里用 `try` 兜住并返回提示（`host-api.md`） |
| HTTP 403 / 401 | 多半是密钥无效或没开通权限，告诉用户，别改代码重试 |
| 请求被拦（内网 / 回环） | SSRF 规则，见 `http.md` |
| 每次请求都要用户确认 | 在 `plugin.toml` 的 `[permissions] http` 里声明 host，再 reload |
| 权限被拒 | 不要重复申请同一次 reload，除非用户要求 |
| 工具在 `search_tool` 里搜不到 | 插件没在跑（`cordis_inspect` 看状态），或名字写错 |
| 改了文件没生效 | reload 读的是那一刻的快照：改完要再 reload |
| 想写 JS / JSX / Wasm / 浏览器端 | Dock 插件只有 Rhai；重活或别的语言写成 MCP 服务器（`dock-config` 技能） |
