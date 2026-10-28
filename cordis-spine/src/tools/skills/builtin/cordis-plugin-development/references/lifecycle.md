# 版本、运行、修复、落盘

## 指针

- **Plugin**：稳定 id。新建时 `idPrefix`（3–6 个小写字母）+ 宿主发号 `prefix-N`，
  计数单调，丢掉的 id 不回收。磁盘插件的 id 就是目录名。
- **Package**：不可改的版本（`packageId`）。改任何东西 → 新 Package，从不覆盖。
- `currentPackageId`：最后一次**成功**的版本（不代表正在跑）。
- `nextPackageId`：正在切换或上次失败的目标。

## `cordis_run` 用哪个 `mode`

| 当前状态 | 目标 | `mode` |
| --- | --- | --- |
| 没有 current | 任意 Package | `run` |
| 有 current | 同一个 Package | `run`（重启） |
| 有 current | 另一个 Package | `update` |
| update 失败了 | `nextPackageId` | `update`（重试） |
| update 失败了 | `currentPackageId` | `run`（回滚） |

- `update` 是**替换**：先整个卸掉上一次 Run 的 fiber（所有 bag、工具、斜杠、面板），
  再在干净的 fiber 上起新 Package，不是叠加。
- `cordis_run` 走和 `bash` 相同的权限确认；技术性失败后授权仍然有效。
- 用户拒绝：除非用户再要求，不要重新申请同一次激活。

## 副作用归谁

- 每个 `provide` / `register_*` 归**整个 host fiber**，不归某一次调用。
- `apply` 抛错：已登记的东西先挂上，内核在报错返回**之前**卸掉 fiber，所以不会泄漏。
  失败后 `currentPackageId` 不变，这时 `cordis_stop` 什么也不做——已经清干净了。
- 没有 `unregister_*`；停掉 fiber 就是撤销。

## 修复循环

`cordis_run` 或 `cordis_call` 报错后：

1. `cordis_inspect` 带 `pluginId` + `packageId`：看源码路径和 `hostError`。
2. 报「已注册」：另一个**正在跑**的包占着这个名字，先 `cordis_stop` 它。
3. 改 `source.rhai`（或给一段新的小 `source`）。
4. `cordis_define`：`plugin: { kind: "existing", pluginId: <同一个> }`，同一个 `source_path`。
5. `cordis_run` 新的 `packageId`：已有 current 用 `update`，否则 `run`。
6. `cordis_call` 再验一次。

不要对同一个 `packageId` 反复 `run`（它不可变，结果一样）；
不要为同一个用途另开一个 Plugin。

## 用户写了 `@pluginId`

1. `cordis_inspect` 带那个 `pluginId`（+ `packageId`）。系统提醒里只有身份，没有源码。
2. `cordis_define` 用 `kind: "existing"` 和原 id。
3. 按上表 `run` 或 `update`。

id 不在了（删了或重启丢了）就直说，不要造一个同名的替身。

## 停、丢、落盘

| 操作 | 定义和版本 | 盘上文件 |
| --- | --- | --- |
| `cordis_stop` | 保留，之后可再 `run` / `update` | 不动 |
| `cordis_stop` + `drop: true` | 从内存里删掉 | 不动 |
| `cordis_promote` | 写盘并从盘上自启 | 写 `.dock/plugins/<id>/` |

- 会话里的插件只活在进程内存，重启就没了。要留就 `cordis_promote`，
  或一开始就写在 `.dock/plugins/<id>/` 下（下次启动自动加载，不用 promote）。
- `cordis_promote`：`scope` 默认 `project`（`{cwd}/.dock/plugins/<id>/`），`user` 写 `~/.dock/plugins/<id>/`。
  默认 id 去掉 `-N` 后缀（`echo-1` → `echo`）。
  盘上 id 与会话 id 不同时，会话那份会被停掉以免工具重复，不再需要就 `cordis_stop drop: true`。
- 即使 Package 是 `source_path` 的，promote 也会把内容写成 `source.rhai`。
- 不想让盘上插件自启：`plugin.toml` 里 `enabled = false`，或删掉目录（删目录要先问用户）。
- 盘上插件源码坏了时启动加载会失败、进程里没有这个 id；
  `kind: "existing"` 会报 `no dynamic plugin`。修好文件后让用户重启 Dock 重新加载，
  别另开一个名字相近的插件。
