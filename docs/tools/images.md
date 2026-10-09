# 工具结果图（多模态）

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

- `ToolResult` 可带 `images`（落盘 `$DOCK_HOME/tool-images/`，cap **20** 张 / 单张体积上限）。`read_file` PDF `format=image` 按页塞多张，所以从早期的 5 提到 20（对齐每呼最多 20 页）。`browser_screenshot` 与 CUA 截图类 MCP（经 `format_call_result` / `promote_cua_fields`）在路径之外把像素塞进下一轮采样；**同轮多个 tool 先齐结果，再统一挂图**（避免 Chat 线 HTTP 400）。
- 路径字符串仍给 `/browser` / `/computer` 驾驶舱；TUI **不**嵌真图。
- 读盘限 `$DOCK_HOME`。`register_deferred` / 权限门不因产图回退。
- 纯文本模型（`supports_images = false`）：图片字节一律不进请求体，三条 wire 都一样。
  - 工具结果：正文里的 `Image content included inline` 换成一句说明（图没送到、别猜）。
  - 用户发的图：同样拿掉，正文后补同一句说明（`tool_images::text_only_note`）。
  - 不说明的话，模型会照着占位符编一段它没看见的画面。
- MCP `structuredContent` / `structured_content`：**非图片字段**（如 CUA `list_windows` 的 `window_id`/`bounds`）序列化追加进工具结果文本（默认截断约 8KiB）；`png_base64` 等大图字段仍只走 `ToolResult.images`，不进文本。不影响 deferred / 权限门（#26）。
