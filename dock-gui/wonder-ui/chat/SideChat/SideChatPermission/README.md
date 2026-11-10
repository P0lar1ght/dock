# Side Chat Permission

- Wonder 文件：Hofmann · 页面「1 · 界面」
- Label：`Side Chat Permission`
- Node id：`iso`
- 画布坐标：(82888, 0)（2026-10-09 整页整理排版后），尺寸 420×812
- 分组：面板（第八轮「侧边聊天」，DESIGN-TASKS-8.md）

## 说明

侧聊想跑有副作用的命令（npm install）：输入框上方权限条，只有「允许」「拒绝」，没有「始终允许」。

## 画板内注释

注释：侧聊自己要跑会改东西的命令时，输入框上方出权限条（照 his.cowy，缩到 420）：「侧边聊天想运行 npm install」+「允许」「拒绝」。侧聊里没有「始终允许」——会改东西的命令每次都问；只读工具（读文件、搜代码、只读命令）不问。等批准时输入框右侧是停止键。

文件：`SideChatPermission.png`（截图）· `SideChatPermission.jsx`（get_element_code 导出）
