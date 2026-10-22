# cordis-apps

Dock 内置的 MCP Apps 服务（规范 `io.modelcontextprotocol/ui`）：stdio MCP，工具挂 `ui://` 界面。不含插件。

| App | 子命令 | 文档 |
|---|---|---|
| 白板（Excalidraw） | `dock mcp whiteboard` | [docs/tools/whiteboard.md](../docs/tools/whiteboard.md) |

- `stdio`：共用的 stdio 服务（两代握手、`tools/*`、`resources/*`）；新 App 实现 `stdio::App`。
- 界面源码在 `web/<app>/`，构建成单文件 HTML 放进 `assets/`，`include_str!` 编进二进制——Rust 构建与 CI 不需要 Node。
  改界面后在 `web/<app>/` 跑 `npm ci && npm run build`，连同 `assets/` 一起提交。
