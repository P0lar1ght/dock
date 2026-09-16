# 浏览器（BUA）

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-browser`

- **ctx**：`"browser"` + `"tools"`
- **模型工具**：`browser_open` `browser_navigate` `browser_navigate_back` `browser_snapshot` `browser_click` `browser_hover` `browser_type` `browser_press_key` `browser_select_option` `browser_fill_form` `browser_wait_for` `browser_drag` `browser_handle_dialog` `browser_file_upload` `browser_resize` `browser_evaluate` `browser_console_messages` `browser_network_requests` `browser_screenshot` `browser_tabs` `browser_close`（按需）

**BUA P2**：in-process **chromiumoxide** CDP。P1 之外增加 `browser_evaluate`（**权限门同 bash**：`needs_permission` + `blocked_in_plan`，经 `tools/execute` / `use_tool` 命中）、只读截断的 `browser_console_messages` / `browser_network_requests`（会话连接时挂 Network/Runtime 监听，保留最近 N 条）、以及 **同域 iframe**：`browser_snapshot` / `browser_evaluate` / `browser_click` 可选 `frame`/`frame_selector`（CSS 选 iframe）；跨域或找不到则明确报错。无 Node/Playwright；CUA 像素点击仍不做。`register_deferred`：不进 sampler / `specs_for_model`。Fiber dispose 关掉 Chromium。`browser_screenshot` 写路径给 `/browser`，并经 `ToolResult.images` 进多模态（见「工具结果图」）。`/browser` 驾驶舱列 P0–P2 + 最近 evaluate/network，并可 **`h` 切换有头/无头**（`[browser].headed`，默认无头；`DOCK_BROWSER_HEADED` 任意非空覆盖；切换后需 close/open 才作用于已开会话。有头 launch：`with_head().viewport(None)` + 初始 `window_size`，避免默认 800×600 Emulation 只画一角；Chromium CLI `.arg` 勿带前导 `--`（库会再加））。`code` / `cordis`（+ general-purpose）允许名单含这些 `browser_*`；`minimal` / `warden` 主代理不含

- **默认**：`browser_snapshot` / `browser_evaluate` / click·type 等操作在**主文档**（current main frame）。
- **进入同域 iframe**：在 `browser_snapshot` / `browser_evaluate`（及 click 的文档对称参数）上传可选 `frame` 或 `frame_selector`（CSS，指向 `<iframe>`/`<frame>`）。实现用主文档 `querySelector` 探测 + `Page.getFrameTree` 匹配 CDP `FrameId`，再对 evaluate 设 `Runtime.evaluate` 的 `contextId`，对 snapshot 传 `Accessibility.getFullAXTree.frameId`。
- **跨域 / 找不到**：探测读 `contentDocument` 失败或树里匹配不到时，**直接失败并返回明确错误**（例如 `cross-origin iframe (CDP cannot enter)` / `no element matching frame_selector`）。不做 OOPIF 像素点击，也不静默落到主文档。
- **refs**：framed snapshot 产生的 `@eN` 只对该 frame 有效；click 前应使用同一 `frame_selector` 拍到的 snapshot。
