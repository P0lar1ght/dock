# 插件视图（dock.view.1）

插件（含 Rhai 动态插件）往界面加东西时，交的是一棵**声明式视图树**（JSON）。
TUI 和 GUI 各自把它画出来；插件不写 ratatui，也不往 GUI 里塞代码。

跟踪：#181 第 3、4 阶段。

解析：Rust 在 `cordis-base/src/view.rs`；dock.1 客户端用 `dock-core/src/plugins.ts`
（同一套规范化，外加面板 / 状态项 / 工具卡的结果解析）。

## 用在哪

四个扩展点共用这一套节点：

| 扩展点 | 插件交什么 | TUI | GUI |
|---|---|---|---|
| 插件面板 | 面板的视图树 | `Overlay::Slot` | 右侧面板「插件」 |
| 工具卡 | 按工具名登记的渲染函数 | 卡片展开区 | 工具卡展开区 |
| 设置卡 | 配置项 schema（见下） | `/cordis` 详情 | 设置 › 插件 › 详情 |
| 状态项 | 一个小状态（文字 + 色调） | 底栏右侧 | 见设计稿 |

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
  （只认受信 ticket）。右上角「视图 | 原始」切换规划中，等 GUI 实现。
- 暂不覆盖：子代理转录里的工具卡（`tool/view` 只查线程自己的会话日志）。

## 设置卡的 schema

插件声明配置项，界面按它生成表单；值按插件存，插件用 `host.setting(key)` 读。

- named service `"plugin.settings"`，一颗插件一份，包停了消失；
- 普通字段存 `$DOCK_HOME/plugin-settings.json`（`{ 插件 id: { key: 值 } }`），没写过读到 `default`；
- 密钥字段存密钥库 `secrets.json`，名字就是字段 `key`，插件用 `host.secret(key)` 读；
- Rhai：`host.register_settings(#{ title, fields })`；`default` 是 Rhai 关键字，写 `"default": 10`；
- 写入按 schema 校验，有一个字段不合法整组不写；
- 终端：`/cordis` 列出当前值，`/cordis set <插件> <key> <值>` 改一项；
- GUI：设置 › 插件，`plugin/settings/list|get|set`（只认受信 ticket）。

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
- GUI：`status/list` + 推送 `status/changed`（只认受信 ticket）。

## 以后可以加（还没做）

- `image`：图片节点（TUI 显示 alt）。
- 表单节点：面板里直接填参数再点按钮。
- 工具卡上的按钮（现在工具卡只读）。
- 节点 `id` 与视图局部刷新：按 `id` 推增量，而不是整棵重拉。
- 对话流里的插件消息卡（不是工具结果，而是插件主动插一张卡）。
