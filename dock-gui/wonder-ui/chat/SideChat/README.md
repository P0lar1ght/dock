# SideChat · 侧边聊天（第八轮）

来源：`docs/DESIGN-TASKS-8.md`。Wonder 文件 Hofmann · 页面「1 · 界面」，画板放在 x ≥ 23208 的新列（间距 60）。

侧边聊天 = 从当前会话分叉出的只读旁问：带主会话上下文、可追问、不打断主线；每个主会话最多一个，关面板即销毁。回主线两条路：「带回输入框」（只填主输入框不发送）和「写进主线」（≤2000 字结论，用户确认后写入主上下文，主会话出现可折叠的「来自侧边聊天的笔记」卡）。

坐标为 2026-10-09 整页整理排版后的位置（侧边聊天组起点 (80968, 0)）。

| 分组 | Label | Node id | 坐标 | 尺寸 | 文件夹 |
|---|---|---|---|---|---|
| 面板 | Side Chat Empty | `eme` | (80968, 0) | 420×796 | [SideChatEmpty](./SideChatEmpty/) |
| 面板 | Side Chat Answering | `die` | (81448, 0) | 420×796 | [SideChatAnswering](./SideChatAnswering/) |
| 面板 | Side Chat Idle | `jun` | (81928, 0) | 420×812 | [SideChatIdle](./SideChatIdle/) |
| 面板 | Side Chat Parent Waiting | `bad` | (82408, 0) | 420×812 | [SideChatParentWaiting](./SideChatParentWaiting/) |
| 面板 | Side Chat Permission | `iso` | (82888, 0) | 420×812 | [SideChatPermission](./SideChatPermission/) |
| 带回主线 | Side Chat Handback Drafting | `big` | (80968, 872) | 420×796 | [SideChatHandbackDrafting](./SideChatHandbackDrafting/) |
| 带回主线 | Side Chat Handback Preview | `led` | (81448, 872) | 420×796 | [SideChatHandbackPreview](./SideChatHandbackPreview/) |
| 带回主线 | Side Chat Handback Done | `cat` | (81928, 872) | 420×796 | [SideChatHandbackDone](./SideChatHandbackDone/) |
| 带回主线 | Side Chat Handback Error | `aln` | (82408, 872) | 420×796 | [SideChatHandbackError](./SideChatHandbackError/) |
| 主会话笔记卡 | Side Note Card Collapsed | `aal` | (80968, 2600) | 868×240 | [SideNoteCardCollapsed](./SideNoteCardCollapsed/) |
| 主会话笔记卡 | Side Note Card Expanded | `two` | (81896, 2600) | 868×355 | [SideNoteCardExpanded](./SideNoteCardExpanded/) |
| 入口与收尾 | Side Chat Entry | `col` | (80968, 1728) | 220×565 | [SideChatEntry](./SideChatEntry/) |
| 入口与收尾 | Side Chat Close Confirm | `sop` | (81248, 1728) | 420×812 | [SideChatCloseConfirm](./SideChatCloseConfirm/) |
| 入口与收尾 | Side Chat Parent Closed | `did` | (81728, 1728) | 420×812 | [SideChatParentClosed](./SideChatParentClosed/) |

## 与文档的偏离 / 待定

- 文档列了 14 张画板（任务口述为 16 张），按文档做 14 张。
- 侧聊输入框去掉了模型 / 推理 chip 和「请求批准」chip（侧聊只读、写操作一律询问）。
- 工具行按真实 ToolCard 渲染为「Read src/auth/login.ts」，不是字面 `read_file`。
- 「写进主线」按钮放在头栏「新窗口」右边、分隔线 + × 左边。
- 权限条用文档文案「侧边聊天想运行 …」，真实 Pending 组件文案是「允许 {tool}？」。
- Entry 照 ToolPanel.tsx 真实列表（无「审查」，有桌面 / 画布），不是设计稿 ShortcutsPane。
- Parent Closed 把输入框收起、副标题改「· 已结束」（文档只规定正文）。
