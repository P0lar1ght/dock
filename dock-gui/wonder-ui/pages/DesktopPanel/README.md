# DesktopPanel

桌面（CUA）面板：agent 经 cua-driver 操作桌面的每一步，和它当时截到的画面。

- `DesktopPanel.jsx`：`lake` Page - Desktop Panel
- `DesktopPanelStates.jsx`：`ian-bbc` Desktop Panel States
- `DesktopPanelLiveCursor.jsx`：`web` Page - Desktop Panel Live Cursor（实时画面 + agent 光标拖动）
- `DesktopAgentCursorStates.jsx`：`rfc` Desktop Agent Cursor States（八种动作 + 三种画面状态）
- 状态：未安装 / 连不上 / 还没操作 / 没有截图。
- 2026-09-30 由 Claude 画（用户授权），与浏览器、文件面板同一并排布局。
- 画面只来自 agent 自己的截图（dock.1 `item/image`），不是实时桌面流。
- 2026-10-02 Claude 补画 `web` / `rfc`（用户授权）：实时画面上的 agent 光标。
