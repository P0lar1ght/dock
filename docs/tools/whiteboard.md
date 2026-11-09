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
| `whiteboard_read {}` | 每个图形一行：id、类型、标签、位置、尺寸、颜色、箭头两端；另给 `structuredContent.shapes` 与可复用的 `patches`（可直接拷进 edit 的 update/add）。`overview` 只给总数与画板范围，`ids` 只给这几个，`related` 给某个图形和它的容器/标签/箭头两端/指向它的箭头/同组 |
| `whiteboard_edit { clear?, delete?, update?, add? }` | 先清空，再删、改、加；任何一条不合法整次不改 |
| `whiteboard_group { ids, ungroup? }` | 把 ≥2 个图形合成一组，或 `ungroup: true` 取消分组 |
| `whiteboard_align { ids, align }` | 对齐：`left` / `right` / `top` / `bottom` / `center_x` / `center_y` |
| `whiteboard_style { ids, stroke?, fill?, strokeStyle? }` | 批量改描边/填充/线型 |
| `whiteboard_layer { ids, to }` | 调 z 序：`front` / `back` / `forward` / `backward`（只对已画成的完整元素） |
| `whiteboard_fit_text { ids }` | 按文字估算把框调到够用的大小（菱形 height 只抬下限） |
| `whiteboard_distribute { ids, direction }` | ≥3 个图形两端固定、中间均分；`direction` ∈ `horizontal` / `vertical` |
| `whiteboard_layout { ids, columns, gap_x?, gap_y? }` | 按网格重排现有图形（保留各自身高宽；默认间距 60/80） |
| `whiteboard_place { id, ref, where, gap? }` | 把已有图形放到参照旁；`where` ∈ `left` / `right` / `above` / `below`（默认 gap 40） |
| `whiteboard_find { queries, limit? }` | 按文字（整段相等、前缀、包含）、类型、颜色、距离排序，不是向量语义。`near` 必须是板上已有的 id，参照自己不算；找不到就整次失败。每个查询先收齐再排序截断 |
| `whiteboard_duplicate { ids, count?, texts?, place?, dx?, dy? }` | 复制给出的图形；两端都在这批 id 里的箭头一并复制（只连一端的留在原件上）。`place` 按选区真实外框平移。副本用新的 z 序键。回新 id：先是请求的，再是自动带上的箭头（标签不在里面） |
| `whiteboard_graph { nodes, edges?, layout?, … }` | 用 nodes+edges 画流程图，自动算坐标；`layout` ∈ `layered` / `row` / `column` / `grid`（默认 layered）；新图排在现有内容右边 |

- 图形 `type`：`rectangle` `ellipse` `diamond` `text` `arrow` `line`。
- 字段（写在工具 description 里；schema 的 `items` 只声明 `object` + `additionalProperties: true`，避免嵌套 properties 触发模型把数组包成 `{"item":...}`）：
  `id?` `x?` `y?` `width?` `height?` `text?`（框里的标签 / 箭头上的字 / 文字内容）`stroke?` `fill?`（`#rrggbb`）
  `strokeStyle?`（solid / dashed / dotted）`from?` `to?`（箭头两端的图形 id）`fontSize?`。
- `add` / `update` / `delete` / `ids` 必须是扁平数组。若误传恰好单键 `{"item": [...]}`（甚至套多层），服务端会自动剥掉并在结果里写「已自动去掉 item 包装层」；
  单个元素被拍平成对象（`"add": {"type": "rectangle", ...}`）、单个 id 写成字符串，服务端也当数组收下。
- 坐标（`x` / `y` / `width` / `height`）写数字或数字字符串都行；写成别的会直接报错，不会默默挪到默认位置。
- 数量与开关（`columns` / `count` / `limit` / `gap` / `dx` / `dy` / `clear` / `overview` …）数字和布尔写成字符串也认（`"2"`、`"true"`）。
- 工具失败时结果里带一句「这次收到：xxx=number ids=array」：一眼看出哪个参数没到服务端。
- 图形对象里写了不认识的字段（比如把 `text` 写成 `label`），结果里会回一句「忽略了不认识的字段：…」，并列出可用字段名。
- 工具结果只讲这次改了什么（新增 / 修改 / 删除的 id 与新坐标）；全量清单用 `whiteboard_read` 拿。
- 没给位置：自动找一块空地（现有内容右边，横着排开，不叠在一起）。箭头给了两端、没给位置：从一端的边连到另一端的边。
- 删掉的图形留墓碑；清空后可以用同样的 id 重画。
- 挪了位置的图形：连在上面的箭头退回骨架重新连，**箭头 id 保持不变**（旧箭头留墓碑：界面把旧版本存回来时按版本比较，旧的赢不了，不会出现两个同 id 的箭头）。
- 只改了尺寸（`fit_text`）不重连箭头；只有位置变了才重连。
- `graph` 的节点默认 200×80（可给 `width`/`height`），横排、竖排、网格和分层都按各自真实宽高占位。分层顺着前进边取最长路，回边照样画但不改层。边的默认 id 是 `e-<from>-<to>`，同 from/to 的第二条边自动加序号。
- 间距类参数（`gap` / `gap_x` / `gap_y`）不能为负。

### 界面工具（面板开着时）

白板界面自己登记的工具（规范 `appCapabilities.tools`），只在右侧面板那份里给（内联那份是只读快照）。
面板开着时打开它的会话经 `use_tool` 调；面板关了就没了。源码 `cordis-apps/web/whiteboard/src/viewTools.ts`。

| 工具 | 作用 |
|---|---|
| `app_whiteboard__get_selection {}` | 用户当前选中的图形：id、类型、标签、位置尺寸 |
| `app_whiteboard__get_viewport {}` | 用户看着的画布区域、缩放，和视野里的图形 |
| `app_whiteboard__focus { ids, select? }` | 把用户的视图移到这些图形（可顺带选中）；只动视图，不改内容 |
| `app_whiteboard__snapshot { ids?, mode? }` | 截 PNG 看实际效果：`mode` = `thumbnail`（默认，长边最多 1600）/ `full`（原尺寸，看细节）；不给 `ids` 截全部 |

### 只给界面用的工具

`whiteboard_load { sinceRev? }`、`whiteboard_save { elements, consumed, appState, files }`（`visibility: ["app"]`，不进工具表）。

### 落盘

```text
$DOCK_HOME/apps/whiteboard/<项目目录名>-<哈希>/scene.json   （旁边一个 scene.lock）
  { type: "dock-whiteboard", rev, elements: [Excalidraw 元素…], pending: [骨架…], appState, files }
```

- 项目来自调用的 `_meta["dock/cwd"]`；没带的归 `default`。
- 目录名 = 会话目录那套名字（`cordis_base::config::cwd_dirname`）+ 完整路径的 8 位哈希：那套名字把非 ASCII、`/`
  都折成 `-`（`…/SunGods-中台` 和 `…/SunGods` 会撞），会话目录有 `meta.cwd` 分得开，白板没有。
- 读改写整段拿 `scene.lock` 的排他锁：终端和桌面端各拉起一个白板服务时互不吞改动，`rev` 不重号。
- 文件坏了（解析不了）：原文件改名成 `scene.json.corrupt-<毫秒>` 留着，从空白板开始，工具结果里说一句；
  读不了（权限、IO）直接报错，不当空白板写回去。
- `pending`：Agent 加的图形先存成 Excalidraw 骨架；界面打开时画成完整元素（箭头路径、标签排版）存回来。
  没开过界面时 `whiteboard_read` 标「待画」。
- 合并按元素 `version`（大的赢，同版本 `versionNonce` 小的赢，同 Excalidraw）；每次有变化 `rev + 1`。
- 整个文件上限 24 MB（含粘贴的图片）；写临时文件（带进程号）再改名。
- 图片（`files`）每次存都回收：只留还在的图片元素、一天内删掉的图片元素引用的。
- 界面存回来的元素至少要有 `id`、`type`、数字 `version`，缺了的不收。

### 界面

- 内联只看；面板里完整编辑。用户改的按版本存回去，并经 `ui/update-model-context` 把「改了什么、选中了什么」
  交给模型（下一步的 system-reminder，只留最新一份）。
- Agent 改了（收到 tool-result，或轮询发现 `rev` 变了）就重载、合并进画布。
- 资源 CSP 只在 `resourceDomains` 放开 `https://esm.sh`（字体走 `@font-face`，不开 `connect-src`）：
  Excalidraw 的手写字体从那里取，离线退回系统字体。
- 改界面：`cd cordis-apps/web/whiteboard && npm ci && npm run build`（产物拷进 `cordis-apps/assets/`，随源码提交）。
  单文件打包时 mermaid（文字转图表）和多语言换成空模块，只留简体中文。
