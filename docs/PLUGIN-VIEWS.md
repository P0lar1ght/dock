# 插件视图（dock.view.1）

插件（含 Rhai 动态插件）往界面加东西时，交的是一棵**声明式视图树**（JSON）。
TUI 和 GUI 各自把它画出来；插件不写 ratatui，也不往 GUI 里塞代码。

跟踪：#181 第 3、4 阶段。

解析：Rust 在 `cordis-base/src/view.rs`；dock.1 客户端用 `dock-core/src/plugins.ts`
（同一套规范化，外加面板 / 状态项 / 工具卡的结果解析）。

完整示例：`examples/plugins/activity/`（Agent 活动：面板 + 状态项 + 设置卡）、
`examples/plugins/trajectory/`（会话轨迹：web 面板）。

## 用在哪

四个扩展点共用这一套节点：

| 扩展点 | 插件交什么 | TUI | GUI |
|---|---|---|---|
| 插件面板 | 面板的视图树（或 web 面板，见下） | 注册浮层 `slot` | 右侧面板（分屏菜单「插件面板」组），可弹出成独立窗 |
| 工具卡 | 按工具名登记的渲染函数 | 卡片展开区 | 工具卡展开区 |
| 设置卡 | 配置项 schema（见下） | `/cordis` 详情 | 设置 › 插件与技能，插件行「配置」就地展开 |
| 状态项 | 一个小状态（文字 + 色调） | 底栏右侧 | 输入框工具栏右组最前 |

GUI 插件面板弹出成独立窗（拖出窗口，或头栏「在新窗口打开」）：

- 不跟会话：一个面板全局一扇，适合挂在副屏上看进度、当遥控器；
- 头栏有「置顶」；插件自己关面板时窗口跟着关；
- 点状态项时面板已经在独立窗里：把那扇窗提到前面，不在主窗口再开一份。

## 节点

每个节点：`{ "type": "...", ... }`。

色调 `tone`：`default` `muted` `accent` `success` `warning` `danger`。

布局：

- `stack { children, gap? }`：竖排。`gap`：`s` `m` `l`。
- `row { children, align? }`：横排。`align`：`start` `between`。
  TUI 放不下就换行。
- `section { title, children, collapsed? }`：带标题的一组，可折叠。

文本：

- `text { text, tone?, weight?, size? }`。
  - `weight`：`normal` `bold`。
  - `size`：`s` `m` `l`。
- `markdown { text }`：和对话里的 markdown 同一套渲染。
- `code { text, lang? }`：等宽块，GUI 带复制按钮。

数据：

- `kv { items: [{ label, value, tone? }] }`：两列键值。
- `table { columns: [string], rows: [[string]] }`：
  - TUI 列宽按内容截断；
  - GUI 超过 20 行出现「展开」。
- `list { items: [{ title, subtitle?, badge?, action? }] }`：
  - `badge`：`{ text, tone }`；
  - `action`：点这一行触发的动作 id。

状态：

- `badge { text, tone }`：胶囊。
- `progress { value?, label? }`：
  - `value` 是 0–1；
  - 没有 `value` = 不确定进度（转圈 / 扫光）。

交互：

终端里只给看得见的动作编号（折叠段里的不编号），按数字键 1–9 点。

- `button { label, action, style? }`：
  - `style`：`primary` `secondary` `danger`；
  - `action` 是动作 id，回到插件（面板走 `on_key(action)`）。
- `link { label, url }`：
  - 只认 `http` / `https` / `mailto`，别的协议画成灰字「已拦下不安全的链接」；
  - GUI 用系统浏览器打开；
  - TUI 显示 url，可复制。

其它：

- `divider {}`：分隔线。
- `empty { title, text?, action?, label? }`：空状态卡（`label` 是按钮字）。

## 兼容与限额

- 不认识的 `type`：画一行灰字「不支持的视图：<type>」，不报错。
  新节点只加不改，旧客户端照样能用。
- 深度最多 8 层，节点最多 500 个；`kv` / `table` / `list` 的每个条目也算一个。
  超出部分丢弃并在末尾画一行提示。
- 单个字符串最长 20 000 字符，超出截断。
- 视图是纯数据：不能带脚本、样式、HTML。

## 工具卡

named service `"tool.views"`：按工具名登记渲染函数，拿这一次调用的参数、输出、成败，回一棵视图树。

- 回 `None` / Rhai 回 `()` = 这次不给视图，用通用卡片；
- 可以给任何工具登记（插件自己的或内置的），同名只能登记一个；
- 只给人看，不进模型历史；
- Rhai：`host.register_tool_view(name, |tc| #{ ... })`，或写在 `register_tool` 的 `view` 上；
  `tc` 是 `#{ name, arguments, output, failed }`（`call` 是 Rhai 保留字）；
- 渲染函数只看 `tc`：要是纯函数，不调工具、不读会话（`host.call_tool` 之类），
  每次重画都可能再跑一遍；
- 经 `use_tool` 调的按需工具也认：按里面那颗的名字和参数找视图；
- 终端：展开工具卡时头照旧，正文换成视图（在会话日志的锁外画）；
- GUI：`tool/views` 列出有视图的工具；展开时 `tool/view { threadId, itemId }` 按需取
  （只认受信连接），右上角「视图 | 原始」切换；有视图的工具头部图标是拼图。
- 暂不覆盖：子代理转录里的工具卡（`tool/view` 只查线程自己的会话日志）。

## 设置卡的 schema

插件声明配置项，界面按它生成表单；值按插件存，插件用 `host.setting(key)` 读。

- named service `"plugin.settings"`，一颗插件一份，包停了消失；
- 普通字段存 `$DOCK_HOME/plugin-settings.json`（`{ 插件 id: { key: 值 } }`），没写过读到 `default`；
- 密钥字段存密钥库 `secrets.json`，名字就是字段 `key`，插件用 `host.secret(key)` 读；
- Rhai：`host.register_settings(#{ title, fields })`；`default` 是 Rhai 关键字，写 `"default": 10`；
- 写入按 schema 校验，有一个字段不合法整组不写；
- 终端：`/cordis` 列出当前值，`/cordis set <插件> <key> <值>` 改一项；
- GUI：设置 › 插件，`plugin/settings/list|get|set`（只认受信连接）。

```json
{
  "title": "部署助手",
  "fields": [
    { "key": "region", "type": "select", "label": "区域",
      "options": ["cn", "us"], "default": "cn" },
    { "key": "token", "type": "secret", "label": "访问令牌" },
    { "key": "verbose", "type": "boolean", "label": "详细日志" }
  ]
}
```

字段 `type`：

- `string`：单行文本；
- `text`：多行文本；
- `number`：可带 `min` `max`；
- `boolean`：开关；
- `select`：配 `options`；
- `secret`：只进不出，界面拿不到原值（同密钥页）。

每个字段可选 `description`、`default`、`required`。

## 状态项

`{ id, text, tone?, tooltip?, surface? }`，named service `"status.items"`：

- `text` 尽量短（一个词或一个数）；界面只截断，不改写；
- `surface`：点它打开哪个插件面板；
- Rhai：`host.set_status(#{ ... })` 第一次登记、之后同 id 改内容，`host.clear_status(id)` 去掉；
  包停了自动消失；
- 旧的 slot `hud: true` 也在列表里（文字是正文第一行，`surface` 指向它）；
- 终端：快捷键条那一行右侧「● 文字」，放不下的折成「+N」；
- GUI：`status/list` + 推送 `status/changed`（只认受信连接）。
  - 最多摆 3 个，按工具栏剩余宽度少摆，多的收进「+N」；连「+N」都放不下就先藏起来；
  - 图标由色调决定（success / warning / danger / accent），default / muted 不带图标。

## web 面板

视图树画不了的交互（时间轴拖选、虚拟滚动、搜索框、图表）走 web 面板：插件自带 HTML / JS，
GUI 放进沙箱 iframe；终端照旧画 `view()` / `render()`。

声明（只有磁盘插件，HTML 放在插件目录里）：

```rhai
host.register_slot(#{ id: "trajectory", title: "会话轨迹", render: || { "在 GUI 里看" }, web: "panel.html" });
```

- 路径相对插件目录；绝对路径、`..`、经软链跑出目录都会让 `apply` 失败。
- 每次打开现读文件（改了 HTML 重开面板就是新的），上限 2 MiB。
- 网关：`surface/list` / `get` 带 `web`；`surface/web { id }` → `{ html }`，只认受信连接。

安全（GUI）：

- iframe 只给 `allow-scripts`：不同源，碰不到 GUI 的 DOM、存储、Tauri IPC；
- 文档里注入 CSP：不许联网（`fetch` / WebSocket / 外链资源都不行），脚本样式只能内联；
- 和 Dock 只经 `postMessage` 桥说话，桥只放行下面几个方法。

桥（页面里的 `window.dock`）：

| 调用 | 回 |
|---|---|
| `dock.call("threads.list")` | `thread/list { scope: "all" }` 的结果 |
| `dock.call("threads.history", { threadId })` | `thread/history` 的结果（完整事件流，`events[].timestamp` 毫秒） |
| `dock.call("threads.watch", { threadId })` | `{ live }`；之后这个会话的实时事件经 `dock.on("event", fn)` 推来 |
| `dock.call("panel.action", { action })` | 本面板的 `surface/action`，`{ closed }` |

- `live: false`：主窗口只转发 GUI 已打开的会话，收不到就自己隔几秒重拉历史；弹出的独立窗自己订阅。
- `dock.activeThread`：主窗口当前会话 id（独立窗里是 `null`），变了推 `dock.on("activeThread", fn)`。
- 设计 token 以 CSS 变量注入（`--dock-ink`、`--dock-accent`、`--dock-font`、`--dock-mono`…），和 GUI 同一套。

示例：`examples/plugins/trajectory/`（会话轨迹）。

## 以后可以加（还没做）

- `image`：图片节点（TUI 显示 alt）。
- 表单节点：面板里直接填参数再点按钮。
- 工具卡上的按钮（现在工具卡只读）。
- 节点 `id` 与视图局部刷新：按 `id` 推增量，而不是整棵重拉。
- 对话流里的插件消息卡（不是工具结果，而是插件主动插一张卡）。
- 表格格子的色调（现在格子是纯文字，「健康 / 降级」这类状态列没法上色）。
- 工具归属：工具卡知道是哪颗插件的工具，插件没加载时能提示「插件 X 未加载」。
