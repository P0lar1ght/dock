# Computer / CUA

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `tool-computer`

- **ctx**：`"computer"`
- **模型工具**：—（桌面能力全部经 `cua-driver` MCP，见下）

**CUA C0**：薄驾驶舱 named `"computer"`。live-lookup `"mcp"` 看 cua-driver 是否就绪；`/computer` 是 TUI CATALOG builtin（`Overlay::Computer`）。挂在 `mcp-client` 之后，fiber dispose 注销。

控本机桌面走 **trycua [`cua-driver`](https://github.com/trycua/cua)** MCP，**不**自研键鼠、**不** Docker / cua 云沙箱、**不** path-dep 进 spine。

- **接入（零配置）**：Dock 启动时自己找本机 driver —— `DOCK_CUA_DRIVER`（绝对路径）→ `PATH` → `~/.local/bin/cua-driver` → macOS `/Applications/CuaDriver.app/Contents/MacOS/cua-driver`。**找得到就注入一条内置 `[mcp_servers.cua-driver]`**（`args = ["mcp"]`、`enabled = true`、绝对路径当 command），不用写 `config.toml`；找不到就当没这条，`/mcps` 不会多出一条死行。配置文件里的同名行**整条覆盖**内置行（连 command 一起），`DOCK_CUA_DRIVER=off` 彻底关掉内置行（测试与 CI 用这个）。公名 `mcp_cua-driver__{tool}`（服务器键名必须是 `cua-driver`），与其它 MCP 一样 **不进** sampler / `specs_for_model`，经 `search_tool` / `use_tool`。
- **不打包 driver**：Dock 的 release **不带** driver 二进制。macOS 上它是 trycua 签名的 app（`com.trycua.driver`，69MB），Accessibility / 屏幕录制授权绑那份签名身份 —— 拷进 Dock 的产物里重签会让授权失效，也等于重分发别人的公证产物。`/computer` 按 `i` 走的是**官方安装脚本**（`https://cua.ai/driver/install.sh`，下载后 `bash <tmp> --no-modify-path`），不改用户的 shell rc；装完自动重探 + `Mcp::reload`，不用重启 dock。
- **搜不到桌面工具时**：`search_tool` 的空结果 note 会点名 —— 桌面类关键词命中且 cua-driver 没连上时，明确告诉模型「去 `/computer` 按 i 装」，免得它只回一句没有这个能力。
- **stdio 帧**：Dock MCP stdio **默认 NDJSON**（每行一条 JSON-RPC，对齐 cua-driver 0.24+；0.28.1 实测仍是 NDJSON，0.28.0 的「modern stdio MCP」没有改默认帧）。LSP `Content-Length` 仅显式 `framing = "content-length"`。可选 `framing = "auto"`：先 CL 探测，`-32700`/parse 则 **kill+respawn** NDJSON（不在同一 stdin 硬切）。无需 Python 桥。
- **元素寻址（`element_token`，优先于坐标）**：`get_window_state` 走一遍 AX 树，同时给出 `structuredContent.elements`（每项 `element_index` / `element_token` / `role` / `label` / `value` / `actions` / `frame` / `parent_index` / `depth`）与向后兼容的 `tree_markdown`。8 个工具接受 `element_token`：`click` `double_click` `right_click` `scroll` `press_key` `type_text` `set_value` `hotkey`。token 格式 `s{snapshot_id:08x}:{element_index}`，**按 (pid, window_id) 作用域，下一次同窗快照即替换**——driver 的不变式是「每轮、每个 (pid, window_id) 先 `get_window_state` 再做元素动作」，过期 token 明确报 `element_token is stale`。走 token 的好处 driver 自己写在 `click` 描述里：对后台 / 最小化 / 隐藏 / 不在当前 Space 的窗口有效，不移光标、不抢焦点，且能回报点的是什么（role + label）。只有 canvas / video / WebGL / 自绘表面（不进 AX 树）才退回 `x, y`。AX 树不可靠时返回 `degraded_reason: ax_window_unresolved`，那种情况按像素点。
- **截图会吃图片配额**：`get_window_state` **默认同时返回截图**，而 `tool_images.rs` 的 `MAX_TOOL_IMAGES = 5` 是每轮硬上限。纯重新索引时传 `include_screenshot: false`（便宜路径，只要树）；只要预览不要树则 `include_accessibility_tree: false`（AX walk 是贵的那半，最长 20s）。两个都 false 是错误。大树（Electron / Obsidian 10k+ 元素）用 `max_elements` / `max_depth` / `query` 收口；缺省是 ≤2000 元素、深度 ≤25。`capture_mode` 已废弃且被忽略。
- **权限 / 计划门**：所有 `mcp_cua-driver__*` 与 `bash` 同级（`needs_permission` + `blocked_in_plan`）。`use_tool` 内层 `execute` 会命中该门。**权限摘要**：`mcp_cua-driver__*`（及一般长 JSON MCP）走结构化摘要（action + role/label，长 token 脱敏），不再用裸 `format!("{} {}", name, args[..120])` 把 CUA 目标挤掉。
- **Allowlist**：MCP extras 仍按现规则 **穿过** Agent preset allowlist；但 `code` / `cordis`（含 general-purpose）须保留 `search_tool` / `use_tool`。`minimal` / `warden` 主代理不含这两项则调不到 cua-driver。
- **勿混 BUA**：`cua-driver` 自带的 `browser_*` MCP 工具 ≠ Dock chromiumoxide `browser_*`。网页自动化优先 Dock BUA；桌面键鼠 / 开应用走 cua-driver。
- **Linux 坑**（写进安装说明）：需要 **X11 或 XWayland**（原生 Wayland 仍预览）；`DISPLAY` / `XAUTHORITY`；`at-spi2-core`（+ 必要时 toolkit-accessibility）否则 AT-SPI / `get_window_state` 弱；把 `~/.local/bin` 放进 `PATH`，或用 `cua-driver mcp-config` 给出的绝对 command；telemetry 默开，可 `cua-driver telemetry disable`。
- **TUI**：`/computer` 驾驶舱带状态机（未挂载 / 未安装 / 缺授权 / 已禁用 / 未连上 / 已连接）与两个动作键：`i` 安装或重装 driver、`p`（macOS）跑 `permissions grant`，`Ctrl+R` 重新探测并重载 MCP 配置。两个动作都是**两步**：先在确认块里列出要执行什么，Enter 才跑；进度一行行进驾驶舱。状态、文案、动作全在 named `"computer"` 里，TUI 只渲染 + 路由按键。不嵌真桌面。
- **冒烟**：装好后 `/mcps` 见 `cua-driver` → `search_tool` 查桌面工具 → `use_tool`（先过权限门）完成截图或点按一类动作。

**本机边界（BUA 在 Linux + X11 冒烟；工具面按 macOS `cua-driver` 0.28.1 实测，56 个工具）**

- `doctor` 应见 `display server: X11` + `X11 connection: connected`。若 `[warn] AT-SPI: accessibility bus not reachable`：装 `at-spi2-core`，确保用户会话有 D-Bus；GNOME 可再开 `gsettings set org.gnome.desktop.interface toolkit-accessibility true`。AT-SPI 弱时 `get_window_state` / a11y 树不可靠，点按仍可能走几何。
- 验收常用 MCP 名（公名前缀 `mcp_cua-driver__`）：`list_apps` / `list_windows` / `launch_app`、`click` / `double_click` / `right_click` / `drag` / `scroll`、`type_text` / `press_key` / `hotkey`、`get_accessibility_tree` / `get_desktop_state` / `get_screen_size`、`bring_to_front` / `invoke_menu`。driver 另暴露 `browser_*`——**不要**当 Dock BUA 用。
- 无图形会话 / 纯 SSH 无 `DISPLAY`：`doctor` 会挂；CI 不要默认跑 cua-driver 实机。本机可用既有 X11/Xvfb，但 AT-SPI 仍要会话总线。
- **办公链 S3（完整 CUA 重测）**：开 `mousepad` → `bring_to_front` → `type_text` → **`hotkey` `ctrl+s`（`delivery_mode=foreground`）** 落盘 `/tmp/...`；`invoke_menu` 仅 AT-SPI 绿时可选；再用 `verify_state` + 读文件确认。固定步骤见验收台 `dock-cua-accept/S3_REPRO.md`。包已装仍 warn 时勿只靠菜单。
- **开应用 / S8**：`list_apps` 可能漏 `mousepad` 等 —— `launch_app` **优先** `launch_path=/usr/bin/mousepad`（或绝对路径）。S8 Thunar 选中/树验证依赖 AT-SPI；弱则只证目录打开（几何/截图），勿强求 a11y 选中态。
- 安装脚本：`https://cua.ai/driver/install.sh` → 常落到 `~/.local/bin/cua-driver`；`mcp-config` 的 JSON `command` 可直接抄进 Dock。

安装（Linux 示例）：

```bash
# 官方安装（二进制进 ~/.cua-driver，并 symlink 到 ~/.local/bin）
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
# Debian/Ubuntu 建议再装 AT-SPI：
#   sudo apt-get install -y at-spi2-core
export PATH="$HOME/.local/bin:$PATH"
cua-driver --version
cua-driver doctor          # 查 DISPLAY / X11 / AT-SPI（macOS 报 TCC / 安装路径）
cua-driver list-tools      # 当前版本真实工具面（macOS 0.28.1 是 56 个）
cua-driver describe <tool> # 单个工具的完整描述 + input_schema
cua-driver mcp-config      # 打印推荐 command/args（可抄进 config.toml）
# 可选：cua-driver telemetry disable

# 升级（原地替换，macOS 的 TCC 授权不丢；升完要重启守护进程）
cua-driver check-update
cua-driver update --apply
cua-driver stop && open -n -g -a CuaDriver --args serve   # macOS
cua-driver permissions status                             # 确认 Accessibility / Screen Recording 还在

# 可选：官方 skill pack（**不要** vendor 进本仓——它带 `version:` 标注，会随 driver 漂移）
# 从 GitHub Release 拉版本化副本，并 symlink 进各 agent 的 skills/ 目录，升级自动跟随
cua-driver skills install
cua-driver skills          # 查看本地包与各 agent 的链接状态
```

`~/.dock/config.toml`（或项目 `.dock/config.toml`）样例——**只有要覆盖内置行时才需要写**（换命令、换 args、固定绝对路径，或 `enabled = false` 关掉）：

```toml
[mcp_servers.cua-driver]
command = "cua-driver"
args = ["mcp"]
enabled = true
# 默认已是 ndjson；旧 CL 服务器才写：
# framing = "content-length"
# 若 PATH 没有，可改成 mcp-config 给出的绝对路径，例如：
# command = "/home/YOU/.cua-driver/packages/releases/…/cua-driver"
```

不写这段也能用：装好 driver 后 `/computer` 会自己发现它。`/mcps` 里给内置行按 Space 禁用时，Dock 会把完整的一行落进用户 `config.toml`（带 `enabled = false`），之后就归配置文件管。
