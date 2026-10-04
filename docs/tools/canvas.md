# 画布

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-canvas`

- **ctx**：→ `"tools"`（不提供 named service）
- **模型工具**：`canvas_create` `canvas_edit` `canvas_data` `canvas_read`（按需，经 `search_tool` / `use_tool`；GUI 按 `use_tool` 的内层工具名认画布项）
- **源码**：`cordis-spine/src/tools/canvas/`；落盘引擎 `cordis-base/src/canvas.rs`

模型写一页自包含 HTML，Dock 桌面端放进会话旁的沙箱 iframe 里跑。
用来放看板、图表、报告、表格、对比、小工具、界面原型。

| 工具 | 作用 |
|---|---|
| `canvas_create { title, html, data? }` | 新建，得到 `canvas-<n>` 的 v1 |
| `canvas_edit { id, old_string, new_string, note, title? }` | 局部替换（恰好一处），出新版 |
| `canvas_edit { id, html, note }` | 整页重写，出新版 |
| `canvas_data { id, data }` | 只换数据，不出新版；页面经 `dock.onData` 就地重绘 |
| `canvas_read { id?, version? }` | 读回 HTML + 数据 + 版本列表；不给 id 列全部 |

### 落盘

```text
<会话目录>/canvas/<id>/
  meta.json    { id, title, createdMs, updatedMs, dataUpdatedMs, versions[{ n, note, createdMs, bytes }] }
  v1.html …    每版一份；回滚 = 把旧版拷成新版，历史不丢
  data.json    数据一份，不分版本
```

- 会话目录是 `$DOCK_HOME/sessions/<cwd-key>/<id>/`；不落盘的页（`/btw`）用不了画布。
- 子代理调用时写到父页（经按页挂的 `"planMode"` 找页，同计划文件）。
- HTML 每版上限 2 MB，数据 4 MB；`id` 只认 `canvas-<n>`。
- 同进程里读改写串在一把锁上（模型工具与网关 `canvas/setData` 会撞）。

### 页面里能用什么

- iframe 只开 `allow-scripts`：能跑脚本、能联网（CDN、fetch），碰不到用户文件和 Dock。
- 内置离线库：`<script src="dock:lib/chart.js"></script>`（Chart.js 4，全局 `Chart`），由桌面端内联。
- 桥 `window.dock`：
  - `dock.data`：当前数据（没有为 `null`）；
  - `dock.onData(fn)`：数据变了就调用，在里面重绘；
  - `dock.setData(obj)`：存下用户在页面里改的数据；
  - `dock.send(text)`：把一段话放进用户输入框（用户自己按发送）。
- 页面没接住的报错显示在画布上，带「让 Agent 修复」按钮（填进输入框，不自动发）。

### 能力分类与预设

- `canvas_read` 是读，其余三颗是写（只读子代理用不了）。
- 不进权限门 / 计划门：只写会话目录，不碰工作区。
- `code`、`cordis` 预设的允许名单里有这四颗。

### 客户端

- 网关 `canvas/*`：见 `cordis-gateway/README.md` 的「画布」。
- TUI 没有画布面板：工具结果里有画布 id，文件在会话目录。
- 桌面端从工具结果文字里读 `canvas-<n>`、`v<n>`、`「标题」`：改结果措辞时三样都要留着。
