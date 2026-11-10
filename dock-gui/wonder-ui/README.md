# wonder-ui — 设计稿源码（只读参考）

来源：Wonder 文件 Hofmann / 页 Dock GUI。
经 Wonder MCP `get_element_code` 导出。
原样 JSX，带 `data-node-id` / `data-node-label`。
不参与构建，不要 import，不要手改。
设计稿更新后整目录重新导出。

## 目录

- 按功能分目录，一个组件 / 画板一个子目录：
- `shell/`：侧栏、窄栏、会话头、分屏、终端、连接条、搜索、通知、快捷面板。
- `chat/`：对话流、输入框、权限条、提问 / 计划 / MCP 确认卡。
- `preset/`：新建预设、预设编辑器（PresetEditor*）、AI 起草（PresetAi*）。
- Wonder 画布分三页（免费版上限）：
- 「界面」页（原 Dock GUI）↔ `shell/` `chat/` `pages/`。
- 「工具卡」页 ↔ `tools/`。
- 「预设」页 ↔ `preset/`。
- 跨页剪切粘贴会保留画板 id（2026-09-25 实测，三页共 124 张）。
- MCP 只能读当前打开的页：查别的页要先请用户切过去。
- `tools/<Tool>/`：工具卡，折叠态 / 展开态。
  - 2026-10-11：Hofmann「1 · 界面」工具卡组（x=19268 起）55 张按 Dock 真实工具重做，每个目录有 `*.jsx` + `*.png` + README（label / node id / 真实工具 / 坐标）；旧稿移到各目录 `legacy-2026-09-24/`。
  - `tools/ReadImage/`：Read 读图片（折叠缩略图 / 展开大图 + 元信息 / 失败 / 点击放大预览）。
- `pages/<Name>/`：阶段 2 的整页参考（工作台、报告、画布、浏览器面板）。
- `reference/`：截图。
- 每个目录的 `README.md` 写来源节点 id。

## 基准画板

- `his`：展开侧栏主壳（组件默认取这里）。
- `bar`：折叠窄栏。
- `nor`：新对话选预设。
- `rpg`：输入框浮起态。
- `job`：品牌行预设下拉。
- `bos`：自定义预设与图标库。
- `cid`：双会话分屏。
- `emil` `moyo` `tway` `monk` `oban`：预设编辑器各分区。
- `paty` `slur` `rone` `sero` `tamp`：内置 / 未保存 / 校验 / 损坏 / 搜索无结果。
- 以上存在 `preset/PresetEditor*`。
- `turr` `sab-zoo` `pom-feb`：AI 起草的描述 / 生成中 / 失败。
- `ady-cod` `ppc-usc` `hub-liz`：AI 草稿结果 / 润色对比 / 推荐工具。
- 以上存在 `preset/PresetAi*`。

## 设计调整（2026-09-24，已经用户确认）

- 原稿不动，改动做在独立小画板上。
- `chat/ComposerV2`：底栏换成真实控件。
- `chat/ComposerRunning`：运行中停止键。
- `shell/SessionHeaderRunning`：运行中指示。
- `shell/SidebarSessionsState`：会话行三态。
- `shell/BrandDefaultPresetMenu`：新对话默认预设。
- `tools/Bash/failed.jsx`：工具卡失败态。
- 有 v2 的以 v2 为准，旧目录保留作对照。
- `chat/TranscriptCodexStyle`：对话流改 Codex 式（2026-09-25，用户截图）。
- `shell/NotificationMenu`：铃铛通知菜单与空状态（2026-09-25，用户确认：刚完成算通知）。
- `chat/QuestionCard`、`PlanCard`、`ElicitCard`：Gemini 稿（2026-09-25）。
- 同批还有 SidebarSessionsWaiting、ConnectionBanner、TranscriptLoading、SidebarRailBell、
  Images、ToolCardStates、UserTurnQueued、SessionMoreMenu、SessionSearch。

## 重新导出

- `get_element_code` 单次上限 200k 字符，超出截断。
- 超过约 50k 字符才存文件，否则进对话上下文。
- 按 70k–190k 分批；不够就捎带一个大画板。
- 存档：`scripts/wonder-save.py <结果> id=目录:说明`。

## 旧文件

- `DockShellLightGlass.*`：早期整壳导出，已被上面取代。

## 注意

设计稿是静态的，没有动效。
动效以 `../docs/MOTION.md` 为准。
