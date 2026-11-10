# ComposerGoalContext

目标 /goal 与上下文 /context 的输入框界面，Claude 画，2026-10-04。

- 画板：`had` Composer Goal & Context（画布 x=14448, y=649，对话流 · 输入框组；2026-10-09 整页整理排版后）。
- 源码：`ComposerGoalContext.jsx`（`get_element_code had` 原样导出）。
- 实现：`components/GoalBar.tsx`、`components/ContextRing.tsx`、`Composer.tsx`。

目标（①–⑥）：
- 底栏「目标」胶囊，开着时蓝底蓝字，占位换成目标引导。
- 用户消息下的「设为目标」。
- 输入框顶上的目标条：进行中 / 展开 / 已暂停 / 已完成（4 秒自动收起）。

上下文（⑦⑧）：
- 模型胶囊左边的圆环：灰 → 离压缩阈值 10% 内橙 → 过阈值红。
- 点开弹层：按类别着色的甜甜圈 + 横条 + 阈值竖线；分「固定前缀」「会话」两组；行可展开明细。
- 底部「压缩上下文」= /compact。
