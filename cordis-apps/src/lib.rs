//! Dock 内置的 MCP Apps 服务：stdio MCP，工具挂 `ui://` 界面（规范 io.modelcontextprotocol/ui）。
//! 由 `dock mcp <app>` 跑起来，Dock 按内置行自己拉起（同 `dock mcp browser`）。不含插件。
//!
//! - [`whiteboard`]：Excalidraw 白板，按项目存，Agent 和用户在同一张上改。

pub mod stdio;
pub mod whiteboard;
