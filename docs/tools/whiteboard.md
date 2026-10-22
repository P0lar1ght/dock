# 白板

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## 内置 MCP App `whiteboard`

- **进程**：`dock mcp whiteboard`（crate `cordis-apps`），Dock 按内置行 `[mcp_servers.whiteboard]` 自己拉起（同 `browser`）；
  `DOCK_WHITEBOARD_MCP=off` 不注入。配置文件里同名行整条盖掉内置行。
- **模型工具**（按需，经 `search_tool` / `use_tool`）：`mcp_whiteboard__whiteboard_read`、`mcp_whiteboard__whiteboard_edit`
- **界面**：`ui://whiteboard/board.html`（Excalidraw），挂在 `whiteboard_edit` 上；规范见 [mcp](mcp.md)「MCP Apps」。
- **源码**：服务 `cordis-apps/src/whiteboard/`；界面 `cordis-apps/web/whiteboard/`，构建产物 `cordis-apps/assets/whiteboard.html`

项目白板：同一个项目（工作目录）的会话共用一张，Agent 和用户在同一张上改。
桌面端里 Agent 一调 `whiteboard_edit`，工具卡下面出现白板（只看）；点「在面板中编辑」进右侧常驻面板，
之后这个项目里的调用都送进那个面板。终端里没有界面，工具照样能用。

| 工具 | 作用 |
|---|---|
| `whiteboard_read {}` | 每个图形一行：id、类型、标签、位置、尺寸、颜色、箭头两端；另给 `structuredContent.shapes` |
| `whiteboard_edit { clear?, delete?, update?, add? }` | 先清空，再删、改、加；任何一条不合法整次不改 |

- 图形 `type`：`rectangle` `ellipse` `diamond` `text` `arrow` `line`。
- 字段：`id?` `x?` `y?` `width?` `height?` `text?`（框里的标签 / 箭头上的字 / 文字内容）`stroke?` `fill?`（`#rrggbb`）
  `strokeStyle?`（solid / dashed / dotted）`from?` `to?`（箭头两端的图形 id）`fontSize?`。
- 没给位置：排在现有内容右边一列。箭头给了两端、没给位置：从一端的边连到另一端的边。
- 删掉的图形留墓碑；清空后可以用同样的 id 重画。
- 挪了位置的图形：连在上面的箭头换成新 id 重新连（`whiteboard_read` 看新 id）。

### 只给界面用的工具

`whiteboard_load { sinceRev? }`、`whiteboard_save { elements, consumed, appState, files }`（`visibility: ["app"]`，不进工具表）。

### 落盘

```text
$DOCK_HOME/apps/whiteboard/<项目目录名>/scene.json
  { type: "dock-whiteboard", rev, elements: [Excalidraw 元素…], pending: [骨架…], appState, files }
```

- 项目目录名同会话目录（`cordis_base::config::cwd_dirname`），项目来自调用的 `_meta["dock/cwd"]`；没带的归 `default`。
- `pending`：Agent 加的图形先存成 Excalidraw 骨架；界面打开时画成完整元素（箭头路径、标签排版）存回来。
  没开过界面时 `whiteboard_read` 标「待画」。
- 合并按元素 `version`（大的赢，同版本 `versionNonce` 小的赢，同 Excalidraw）；每次有变化 `rev + 1`。
- 整个文件上限 24 MB（含粘贴的图片）；写临时文件再改名。

### 界面

- 内联只看；面板里完整编辑。用户改的按版本存回去，并经 `ui/update-model-context` 把「改了什么、选中了什么」
  交给模型（下一步的 system-reminder，只留最新一份）。
- Agent 改了（收到 tool-result，或轮询发现 `rev` 变了）就重载、合并进画布。
- 资源 CSP 只放开 `https://esm.sh`：Excalidraw 的手写字体从那里取，离线退回系统字体。
- 改界面：`cd cordis-apps/web/whiteboard && npm ci && npm run build`（产物拷进 `cordis-apps/assets/`，随源码提交）。
  单文件打包时 mermaid（文字转图表）和多语言换成空模块，只留简体中文。
