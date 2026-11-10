# Side Chat Close Confirm

- Wonder 文件：Hofmann · 页面「1 · 界面」
- Label：`Side Chat Close Confirm`
- Node id：`sop`
- 画布坐标：(81248, 1728)（2026-10-09 整页整理排版后），尺寸 420×812
- 分组：入口与收尾（第八轮「侧边聊天」，DESIGN-TASKS-8.md）

## 说明

侧聊在答时点 ×：× 下方小弹层「结束这段侧边聊天？」+「内容不会保存。」+「取消」「结束」（危险色）。

## 画板内注释

注释：关闭时侧聊还在跑（如图：正在答）或有未写入的预览，点 × 先弹这个小弹层；都没有就直接关。弹层右缘对齐 ×，样式照 quag 弹层；「结束」用危险色（同 Session Delete Confirm），点了销毁侧聊并关面板；「取消」/ Esc / 点外面收起。

文件：`SideChatCloseConfirm.png`（截图）· `SideChatCloseConfirm.jsx`（get_element_code 导出）
