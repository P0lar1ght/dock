# dock-gui

Dock 的桌面 GUI（Tauri 2 + React + Tailwind v4）。
已接真 Dock（`dock serve` + dock.1）。进度见 `docs/PROGRESS.md`。

## 与 Dock 的关系

- 独立工程，`src-tauri/` 自带 `[workspace]`。
- 启动时拉起一个 `dock serve`（headless）。
- ticket 经 Dock 的 stdout 交给 Tauri。
- 前端经 loopback WS 走 dock.1。
- 一个 Dock 进程，跨目录列会话。
- 每个会话用自己的 cwd。

## 目录

- `wonder-ui/`：设计稿源码，按组件分目录，只读。
- `docs/`：进度、设计对照、动效规范、设计任务书规则（`DESIGN-BRIEF.md`）。
- `src-tauri/src/dock.rs`：拉起并看管 Dock。
- `src/dock/client.ts`：dock.1 客户端。
- `src/dock/project.ts`：事件 → 界面条目。
- `src/dock/store.ts`：`useDock()` 全部状态。
- `src/components/`：界面组件。

## 开发

```bash
cd dock-gui
npm install
npm run tauri dev      # 起窗口
npm run build          # tsc + vite
```

先在仓库根 `cargo build -p cordis-app`。
开发构建默认用 `../target/debug/dock`。

## 环境变量

- `DOCK_BIN`：Dock 可执行文件路径。
- `DOCK_GUI_CWD`：Dock 启动目录（开发用）。
- `DOCK_GUI_ORIGIN`：覆盖 webview Origin。
- `DOCK_HOME`：Dock 数据目录（测试用临时目录）。

## 快捷键

- `⌘B`：折叠侧栏。
- `` ^` ``：开关底部终端。
- 全部快捷键：设置 › 快捷键（定义在 `src/dock/shortcuts.ts`）。
