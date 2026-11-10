# Status Items Composer

- node id: `uni`（本轮未变）
- label: Status Items Composer
- 文件：Hofmann · 页面「1 · 界面」
- 导出：2026-10-06（第七轮验收意见第一轮修改）

## 本轮改动

- 画在真实输入框那一行（#11）：复制 ComposerGoalContext（had）的目标条（进行中）+ Composer 工具栏（+ / 请求批准 / 计划 / 目标 | 上下文环 / 模型 / 麦克风 / 发送）。
- 状态项插在左组与右组之间、靠右：部署中 60%（accent Loader）、CI 通过（success CircleCheck）、2 个告警（warning TriangleAlert）（#6）。
- tooltip 往上弹（插件名 + 一句说明），右缘对齐胶囊，不盖住发送按钮。画板加宽到 880。

## 第二轮改动

- 导出：2026-10-06（第七轮验收意见第二轮修改）
- 注释补两句：工具栏放不下时先把状态项收进「+N」，至少留一个在外面；被截断的那项，tooltip 第一行显示完整原文，下面才是插件名 + 一句说明（没截断的项不重复原文）。
