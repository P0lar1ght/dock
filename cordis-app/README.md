# cordis-app

Dock 的二进制入口 crate。产品名 **`dock`**（`[[bin]] name = "dock"`），crate 名保持 `cordis-*` 前缀。

`main.rs` 只做一件事：建 `Context`、`install_app` 挂上整棵插件树、按依赖顺序 `.wait().await`。本身几乎没有业务逻辑——装配关系都在这里。

## 启动

```bash
cargo run -p cordis-app                # 起 TUI
dock --resume                          # 恢复本 cwd 最近一次会话
dock --resume <id>                     # 恢复指定会话
```

会话存档在 `$DOCK_HOME/sessions/<cwd>/`（默认 `~/.dock/sessions/`）。

## 插件树装配顺序

`main` 里的顺序是依赖顺序，不能随意调：

```rust
install_app(&root).await?;             // spine：工具表、MCP、会话、预设
sessions.attach_disk();                // 挂上会话存档目录
sessions.seed_preset_if_unset(...)     // 新存档打上当前 preset id

root.plugin(system_prompt(), ())?      // 基础 system prompt（context/"context"）
root.plugin(agent_loop(), ())?         // Agent 循环
root.plugin(session_actor(), ())?      // 会话 actor（UI 与 loop 之间的队列）
root.plugin(gateway(), ())?            // 回环网关：挂载但不监听
root.plugin(cron_driver(), ())?        // 定时任务驱动
root.plugin(tabs(), tab_mount())?      // 分页：必须在 TUI 前，第一帧就问当前页
root.plugin(tui(), ())?                // 全屏 TUI（最后）
```

- `--resume` 时 `resume_and_apply_preset` 会同时恢复会话并应用归档时记下的 preset（失败只 `eprintln!`，不阻断启动）
- gateway 由 `cordis-gateway` 提供；TUI `/pair` 才触发监听，见 `cordis-gateway/README.md`

## 对外导出

| 项 | 干什么 |
|---|---|
| `system_prompt` | 基础 `<system>` 内容，注册到 `"context"`（`ContextBook::set_base`）。换 preset 不换它：persona 说"是谁"，这段说"怎么干活" |
| `session_actor` / `SessionHandle` | 会话 actor；`SessionHandle` 暴露 `submit` / `compact` / `cancel` / `promote` / `queued_prompts` 等 |
| `tab_mount` | 建页工厂，注入 `cordis_tui::tabs()`。第 1 页是根上下文，这里造第 2 页起每一页 |
| `cron_driver` | 定时任务（`cron` 工具）的驱动循环 |
| `SESSION` / `Error` / `Result` | named service key 与错误类型 |

## 旁问页（aside）

`tab.rs` 里旁问页是**只读预设**：只给 `read_file` / `grep` / `list_dir` / `glob`，不给写工具，也不给 `search_tool` / `use_tool`。

理由在源码注释里：旁问与主线共用同一工作目录且并发跑，主线正在改文件时让旁问也能写就是制造竞态；MCP 侧有 cua-driver 这类能操作桌面的工具，插话不该有这个本事。**改这个白名单前先想清楚。**

## 相关文档

- 插件树与不变式：`docs/ARCHITECTURE.md`
- 命令 / 斜杠 / 快捷键：`CLI.md`
- 网关：`cordis-gateway/README.md`
- 开发环境与测试：`docs/DEVELOPMENT.md`
