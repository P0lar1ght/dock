# BrowserPanel

浏览器面板：看会话正在用的 Chromium 标签页，能接手操作。

- `BrowserPanelLive.jsx`：`turm` Page - Browser Panel Agent Live
- `BrowserPanelStates.jsx`：`alo-ali` Browser Panel States
- 状态：接手操作 / 连接中 / 未打开网页 / 浏览器已退出。
- 2026-09-30 由 Claude 画（用户授权），与 Canvas 的 `sert` 同一并排布局。
- 画面里的 SkyFare 是假网站，只为演示截屏流。
- `BrowserPanelTabs.jsx`：`das` Page - Browser Panel Tabs（标签栏主稿）
- `BrowserTabStripStates.jsx`：`mba` Browser Tab Strip States
- 标签栏四态：只有一页（也能关）/ 很多页 / 页面新开了标签页 / 你来切换新开关闭。
- 2026-10-02 由 Claude 画（用户授权）：头栏换成标签栏，状态和接手挪到地址栏右边。
- 和 agent 共用当前页（用户拍板）：面板切到哪页，agent 接着就在哪页操作。
