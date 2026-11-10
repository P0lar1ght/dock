# Plugin View Gallery

- node id: `hum`（本轮未变）
- label: Plugin View Gallery
- 文件：Hofmann · 页面「1 · 界面」
- 导出：2026-10-06（第七轮验收意见第一轮修改）

## 本轮改动

- 定下全图鉴节点规范，其它画板照此同步（#1/#6/#7/#13）。
- text 加一行小号（size s，12px）。
- kv：标签列固定 96px、值左对齐紧跟，不套卡片。
- progress：右侧显示「60%」，不套卡片；list 不套卡片。
- section 是唯一的卡片（白底 · 圆角 10 · 边框 #e5e5ea），加「卡片规则」注释；empty 去掉卡片边框。
- button 统一 h32 · 圆角 8 · 12px；补禁用态，并加执行中规则注释（同面板其它按钮禁用）。
- 加图标规则块：插件图标统一 lucide Puzzle；状态项图标按 tone（success CircleCheck / warning TriangleAlert / danger CircleX / accent Loader 转圈；default / muted 无图标）。

## 第二轮改动

- 导出：2026-10-06（第七轮验收意见第二轮修改）
- table 四行（Table Row 0–3）加 items-baseline，「实例」列数字和同行其它格按基线对齐。
