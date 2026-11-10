# 会话轨迹（示例插件 · web 面板）

按轮次看一次会话的完整轨迹，参照 DSH 的「轨迹检查记录表」。一颗 Rhai 磁盘插件，
GUI 里是插件自带的 web 面板（`panel.html`，`docs/PLUGIN-VIEWS.md`「web 面板」）。

- 顶部：选会话（默认跟随 GUI 当前会话）、搜索（内容 / 工具名 / 参数 / 结果）、全部折叠。
- 计时总览：轮次 / 模型 / 工具 / 上下文四条轨道；灰色是等首个 token（含直接调工具前的思考），
  蓝色是输出，橙色是工具，绿色竖标是这一轮开头注入的上下文（系统提示 / 注入块）。拖动选时间段，只看那段里有活动的记录。
- 记录表：按轮次分组（轮次 id、步数、工具次数、耗时），你说的 / 思考 / 回复 / 工具 / 审批 / 压缩 / 系统提示 / 注入；
  工具缩进在它那一步下面；每行右侧是等待（⏳）和耗时。点轮次头折叠。
- 检查器：点一行打开。工具看参数 / 结果（JSON 自动排版）/ 计时；系统提示 / 注入看全文（截断时标原字数）；
  文字看全文 / 计时。
- 实时：当前会话的事件经桥推来就追加；收不到推送时每 3 秒重拉一次（右上角圆点灰色）。

只读：经桥读 `thread/list`、`thread/history` 和实时事件，不改 Agent 的任何行为，也不能联网。
终端里没有 web，只显示一行提示。

## 装上

```bash
mkdir -p ~/.dock/plugins
cp -R examples/plugins/trajectory ~/.dock/plugins/trajectory
```

重启 Dock，在 GUI 右侧面板的分屏菜单「插件面板」组里打开「会话轨迹」；可以弹出成独立窗、置顶。

## 改它

`panel.html` 是纯 HTML / JS（无构建）：`build()` 把事件流折成轮次和记录，`drawOverview()` /
`drawLedger()` / `drawInspector()` 各画一块。改完在 GUI 里关掉面板再开就是新的。
回归：`cargo test -p cordis-spine --test dynamic -- example_trajectory_plugin`。

## 还没有的（相对 DSH）

- **harness 系统注入**：dock.1 已新增 `item/system_prompt` / `item/system_reminder` 两条事件，
  面板能看见每轮的系统提示与注入块（规约副本、长期记忆索引、MCP 已连接、todo / 计划模式这些）。
  但**只在 dock.1 客户端里可见**：TUI 读的是 spine 的会话日志（不走 dock.1），仍按设计不画
  这些注入块（`cordis-tui/src/scrollback/mod.rs`）；dock-core 的 reducer 不认识这两个 method，
  会静默忽略。
- token 用量、请求级的 TTFT / 解码时间：Dock 还没记每次请求的用量和计时（这里的「等待」是事件时间算的近似）；
- 虚拟滚动：只画最近 40 轮，更早的点「显示更早」；
- 时间轴缩放。
