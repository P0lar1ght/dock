# cordis-tui

Grok pager 界面，拆成 Cordis 插件。Harness 只 `plugin(tui())`。全屏接管和 Grok 一样：raw mode + **stderr 备用屏**，启动后占满窗口，不再露出 shell 里刚输入的命令。

| 插件 | ctx key | 做什么 |
|---|---|---|
| `theme` | `theme` | GrokNight 调色板 |
| `tui.scrollback` | `tui.scrollback` | Grok block：`❯`/`$`/`↻` 用户、markdown 助手、折叠 `◆` 工具（点击展开，整卡可点）；外壳（缩进 / 换行宽度 / 截断脚注 / 折叠符）统一在 `scrollback/card.rs`，各卡片只管自己的内容判定。整份 transcript 按 `layout_key`（宽度 / `events_rev` / 折叠态 / **配色**）缓存，运行中卡片的扫光与耗时走 `paint_live_chrome`，不进缓存 |
| `tui.prompt` | `tui.prompt` | 随内容长高的 `┃` + `╭─╮` composer；粘贴 / 历史上翻；`/` 弹出 slash 下拉 |
| `tui.statusBar` | `tui.statusBar` | 顶栏 cwd / turn / idle |
| `tui.welcome` | `tui.welcome` | braille logo + 菜单（空 session）；F3 resume 时切到 fullscreen picker |
| `tui` | — | 事件循环；inject `session` + `session.port` |

发消息时 live lookup `session.port`，不把 `Arc` 关进闭包。

Slash / 快捷键：`/new` `Ctrl+W` 归档后清空；`/resume` / F3 会话 picker（可搜索）；`/history` 提示词历史；`/find` 搜索 scrollback；`/copy` 复制上一条助手回复；`/help` 或 `Ctrl+.` 快捷键速查；`/quit` 退出。Composer 支持左右光标、`Shift+Enter` 换行、`@` 文件补全。Mermaid 块可点 `[Copy Source]`（affordance 行不换行：面板放不下时按 `[Copy Image Path]` → `[Copy Source]` 的顺序省按钮，画出的列位与点击判定共用同一张表）。

```bash
cargo run -p cordis-app
```
