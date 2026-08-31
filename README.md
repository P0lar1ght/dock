# dock

Grok 外形的本地 Agent。**不 path-dep `grok-build/`。** 外面的参考树（`cordis/`、`deepseek-harness/`、`grok-build/`）不属于本仓库。

第一方库统一 `cordis-*`。其余是入口、注入、冻结副本。

| 目录 | 干什么 |
|---|---|
| [`cordis-rust/`](cordis-rust/) | 插件内核：`Context`、inject、named services（crate `cordis`） |
| [`cordis-spine/`](cordis-spine/) | Agent 循环、工具、MCP、会话 |
| [`cordis-tui/`](cordis-tui/) | 全屏终端 UI |
| [`cordis-app/`](cordis-app/) | 二进制入口 |
| [`cordis-render/`](cordis-render/) | 输出渲染：Markdown、Mermaid（后续同类往这里加） |
| [`embed/`](embed/) | 宿主页 JS 注入（`dock-embed.js`） |
| [`vendor/`](vendor/) | 冻结副本：[`mermaid/`](vendor/mermaid/) 布局栈、[`xai/`](vendor/xai/) Grok 拷贝 |
| [`skills/`](skills/) | Agent skills |
| [`assets/`](assets/) | 品牌图 |

插件规则：[AGENTS.md](AGENTS.md)。模型工具：[TOOLS.md](TOOLS.md)。斜杠 / TUI：[CLI.md](CLI.md)。

```bash
cargo run -p cordis-app
```

Model picker（`/model`，F2 Settings）读 `~/.dock/config.toml` 再读 `.dock/config.toml`。见 `config.toml.example`。`DOCK_MODEL` 覆盖 `[models].default`。

Stub LLM 会 echo。产品只在这棵树里改。
