# SidebarSessionMenu

侧栏会话行右键菜单。Wonder 里原本没有，Claude 照 SessionMoreMenu（dubs）画，2026-09-26。

- 画板：`ber` Sidebar Session Context Menu（画布 x=8248, y=0，主壳组件组；2026-10-09 整页整理排版后）。
- 样式与 `dubs` 一致：白底、`#e5e5ea` 边、rounded-lg、p-1.5、项 px-2 py-1.5 text-xs。
- 分三组：打开 / 右侧分屏（⌘ 点击）/ 单独面板 / 在新窗口打开；改名 / 复制 ID / 访达；断开（记录保留）/ 删除…。
- 「断开」只在会话开着时出现。
- 右键的那一行：hover 底色 + 1px 蓝色描边（`rgba(10,132,255,0.4)`）。
- 实现里「单独面板」图标用会话头同款 SquareSplitHorizontal。
- 源码没导出成 jsx：结构就是上面几条 + `dubs` 的项样式，要原样 JSX 时 `get_element_code ber`。
