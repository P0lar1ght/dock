# 浏览器（BUA）

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

浏览器工具不在 Dock 进程里。
它们是内置 MCP 服务 `browser` 暴露的工具。
服务就是 `dock` 自己：`dock mcp browser`（crate `cordis-browser`）。

## 内置 MCP `browser`

- **怎么接上**：`dock` 二进制启动时登记自己的路径。
- 登记后 `load_mcp_servers` 注入一条内置行 `[mcp_servers.browser]`。
- 这条行是 `<dock 绝对路径> mcp browser`，stdio，NDJSON 帧。
- 零配置；配置文件里的同名行整条覆盖它。
- `DOCK_BROWSER_MCP=off` 关掉内置行（测试与 CI 用）。
- 测试进程、嵌进别的宿主时不登记，所以不会凭空拉起子进程。
- 连不上照样 fail-open：`mcp-client` 保持 `Active`。
- 服务进程启动很轻：Chromium 等到第一次 `browser_open` 才拉起。

## 工具

- 公名 `mcp_browser__browser_*`，22 颗，本名与以前的进程内工具一致（`browser_js` 是新加的）：
  - `browser_js`（一次跑一段 Playwright 脚本，见下）
  - `browser_open` `browser_navigate` `browser_navigate_back`
  - `browser_snapshot` `browser_click` `browser_hover` `browser_type`
  - `browser_press_key` `browser_select_option` `browser_fill_form`
  - `browser_wait_for` `browser_drag` `browser_handle_dialog`
  - `browser_file_upload` `browser_resize` `browser_evaluate`
  - `browser_console_messages` `browser_network_requests`
  - `browser_screenshot` `browser_tabs` `browser_close`
- 和其它 MCP 一样：默认不进 sampler，经 `search_tool` / `use_tool`。
- 想让某个预设直接看见：`resident_tools: [mcp_browser__*]`（见 [agent-presets](agent-presets.md)）。
- MCP 工具绕过预设允许名单；预设 yml 里不再列 `browser_*`。
- `tools/list` 带 `annotations.readOnlyHint`（snapshot / screenshot / console / network / wait_for）。
- 失败回 `isError: true`；Dock 客户端据此标 `ToolResult.is_error`，不靠猜文本。
- `browser_screenshot` 存到 `$DOCK_HOME/browser/screenshots/`，同时回 MCP `image` 内容。

## JS 对话框与卡住的页

- 点击 / 按键 / 跑脚本弹出 `alert` / `confirm` / `prompt` / `beforeunload` 时，CDP 命令要等对话框关掉才返回。
  - 对话框一弹出，这次调用就先返回（不算失败）：写明弹的是什么，叫模型调 `browser_handle_dialog`。
  - 以前这一下会把调用一直挂着（实测 10 分钟没回），同组后面的调用全排在它后面。
- 对话框没关之前，除了 `browser_handle_dialog`（和 `browser_close`），本组别的调用立刻报错指路，不去碰被挡住的页。
- 人在面板里把对话框点掉也算关了（`Page.javascriptDialogClosed`）。
- `browser_handle_dialog` 处理对话框所在的那一页，不一定是活动页。
- 每次调用最多等 90 秒（`browser_wait_for` 是它自己的超时再加 15 秒），到点报错说清是哪一步卡住。

## browser_js（Playwright 脚本）

参考 Codex 的浏览器做法（`node_repl` 的 `js(title, code)`：模型写一段脚本、一次做好几步）。
Codex 那套是闭源插件，这里用开源的 playwright-core 自己实现。

- 参数：`code`（async 函数体；只有一个表达式时直接当返回值）、`title`（给人看的一句话）、`timeout_ms`（默认 60 秒，最长 300 秒）。
- 脚本里有：
  - `page`：本会话活动标签页的 Playwright Page。`context` / `browser` 也在。
  - `state`：本会话跨调用保留的对象（局部变量不保留）。
  - `log(...)`：收进输出。`snapshot(selector)`：ARIA 快照文本。`screenshot(opts)`：存 PNG，最后一张随结果回传。
- 实现（`cordis-browser/src/js.rs` + `src/js/runner.mjs`）：
  - 每个 `dock mcp browser` 进程一个常驻 Node 执行器，stdin / stdout 一行一个 JSON；死了下一次调用重起。
  - 执行器用 `chromium.connectOverCDP` 连同一个 Chromium（和 chromiumoxide 并存），按 target id 找本会话的活动页。
  - 本会话还没有页就先开一个空白页。跑完旧快照的 refs 作废。
- 依赖：Node 18+（`DOCK_NODE` 可指定）+ playwright-core。
  - 第一次用时 `npm install playwright-core@1.64.0` 到 `$DOCK_HOME/browser/js/`（只是驱动，不下载浏览器）。
  - 离线时用 `DOCK_PLAYWRIGHT_CORE` 指到已有的包目录。
- 对话框：Playwright 默认会自动关掉对话框，执行器给每页挂一个空监听让它留着；脚本触发对话框时提前返回，交给 `browser_handle_dialog`。
- 权限：和 `browser_evaluate`、bash 同级（询问 + 计划模式里挡）。
- 测试：`cargo test -p cordis-browser --test real_chrome -- --ignored browser_js`（要 Chrome、Node、npm）。

## 权限

- `mcp_browser__browser_evaluate`、`mcp_browser__browser_js` 与 bash 同级：`needs_permission` + `blocked_in_plan`。
- 其余 `mcp_browser__*` 不过门（和以前进程内的划分一致）。
- 子代理能力档位按本名归类：看页面的算检索，动页面 / 跑 JS 的算执行。

## 会话与进程

- **一个 Chromium，共用 profile**：`$DOCK_HOME/browser/user-data`，登录态所有会话共享。
- **按会话分标签页**：每个会话一组自己的标签页、快照 refs、对话框、network / console。
- `browser_tabs` 只列本会话的页；`browser_close` 只关本会话的页。
- 本会话的页自己开的新页（`target=_blank` 链接、`window.open`）也算本会话的：
  - 每次调用前和 Chrome 对一遍，收进来、插在打开它的页右边并成为活动页（和真浏览器一样）。
  - 所以点了新开标签页的链接之后，下一步就在新页上；`browser_tabs` 列得到它。
  - 别的会话的页开的不收。
- 最后一组关掉时，自己拉起的 Chromium 也关。
- **会话身份**：Dock 在 `tools/call` 的 `_meta["dock/sessionId"]` 里带。
  - 取发起调用那一页的落盘会话 id（GUI 的 threadId）。
  - 还没落盘的用页身份（`main` / `main#2`）；子代理用自己的身份，所以有自己的标签页。
  - 别的 MCP 客户端不带这个键，全部归到默认组。
- **连已有的 Chromium**：先读 user-data 下的 `DevToolsActivePort`。
  - 连得上就直接用（比如 TUI 和 GUI 各起了一个 Dock）；连上的一方不关别人的进程。
  - 连不上（文件过期）才自己拉起。
- 拉起时不带 `--enable-automation`、关掉 `AutomationControlled`（`navigator.webdriver` 为 false）；
  无头时 UA 换成同版本普通 Chrome（不写 `HeadlessChrome`）。
  - 以前两样都露着，Google 搜索一直弹人机验证，人在面板里也过不去。
  - 只管自己拉起的；连上已有的 Chromium 时沿用它启动时的参数。
- Chromium 被关掉或崩了：下一次调用把各组作废，提示先 `browser_open`。
- stdin 关闭（Dock 退出）：关掉全部标签页和自己拉起的 Chromium。

## 运行时名册与网关画面

- MCP 服务把「会话 → 标签页」写到 `$DOCK_HOME/browser/sessions/<pid>.json`。
  - 标签页变了（开、关、切）就整份重写，先写临时文件再改名；都关了就删掉。
  - 一个进程一份，多个 Dock 共用 Chromium 时互不覆盖。
  - 运行时状态，不是会话数据。
- 网关 `browser/view/*` 按它找到会话的活动标签页，另开一条 CDP 连接推画面（`cordis_browser::view`）。
  - 协议见 `cordis-gateway/README.md`「浏览器画面」。
  - 关视图只断自己的连接，不关页、不改 agent 那页的视口。
- 为什么不让 GUI 直连 CDP：Dock 可能在远程机器上，调试端口只在那台机器的回环上，也不该暴露。

## 显示（有头 / 无头）

- `[browser].headed`，默认无头；`DOCK_BROWSER_HEADED`（任意非空）覆盖为有头。
- 服务拉起 Chromium 时读；已开的 Chromium 不重启。
- 连上已有的 Chromium 时沿用它原来的模式。

## `/browser` 驾驶舱（`tool-browser`）

- named `"browser"`，只看内置 MCP 的状态：未挂载 / 未启用 / 已禁用 / 已连接 / 未连上。
- 按 `h` 切换有头 / 无头偏好；显示待批的 `mcp_browser__*` 调用。
- 不渲染网页，也不再缓存标签页与截图（那些状态在 MCP 服务进程里）。

## 细节（沿用）

- CDP 由 chromiumoxide 驱动；无 Node / Playwright。
- 快照是精简 a11y 树，refs `@eN` 给 click / type / hover / select / fill。
- 同域 iframe：`frame` / `frame_selector`（CSS 选 iframe）。
  - 跨域或找不到直接报错，不静默落回主文档。
  - framed snapshot 的 refs 只对该 frame 有效。
- chromiumoxide 的 `.arg` 不要带前导 `--`（库会再加一遍）。
- 有头启动用 `with_head().viewport(None)` + 初始窗口大小，避免默认 800×600 只画一角。
- CUA 像素点击不做；桌面操作走 cua-driver（见 [computer](computer.md)）。

## 测试

- `cargo test -p cordis-browser`：工具清单、协议（两代握手、isError、未知方法、坏行）。
- `cargo test -p cordis-app --test browser_mcp`：真起 `dock mcp browser`，经 `search_tool` / `use_tool`。
- 真 Chrome 冒烟（`#[ignore]`）：`cargo test -p cordis-browser --test real_chrome -- --ignored --test-threads=1`。
  - 覆盖按会话分页、第二个进程连上已有 Chromium、P0–P2、画面与用户输入。
- 网关画面端到端（`#[ignore]`）：`cargo test -p cordis-gateway --test gateway -- --ignored browser_view_streams`。
