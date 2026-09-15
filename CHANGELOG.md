# Changelog

本文件记录用户可见的变化。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号用[语义化版本](https://semver.org/lang/zh-CN/)。

0.x 期间不承诺 `config.toml` 的键与 `dock.1` 协议的向后兼容：破坏性变更会写进对应版本，
并在升级说明里给出改法。

## [未发布]

### 新增

- **`/computer` 自己会装 cua-driver**：驾驶舱变成状态机（未挂载 / 未安装 / 缺授权 / 已禁用 /
  未连上 / 已连接），`i` 走 trycua 官方脚本装或重装，`p`（macOS）让 driver 自己拉起
  Accessibility / 屏幕录制授权对话框。两个动作都是**两步**：先列出要执行的每一步，`Enter` 才真的跑，
  `Esc` 只取消确认；进度一行行显示在驾驶舱里，装完自动重新探测 + 重载 MCP，不用重启 dock。
- **cua-driver 零配置接入**：Dock 启动时自己发现本机 driver（`DOCK_CUA_DRIVER` → `PATH` →
  `~/.local/bin` → macOS `/Applications/CuaDriver.app`），找得到就注入内置
  `[mcp_servers.cua-driver]`，不必再手写 `config.toml`；`DOCK_CUA_DRIVER=off` 彻底关掉。
  Dock 的 release **不带** driver 二进制。
- 桌面类关键词（click / screenshot / 桌面 …）搜不到工具且 driver 没连上时，`search_tool` 的 note
  会直接指向 `/computer` 的安装键。

### 变更

- `/computer` 的 `Ctrl+R` 重新探测本机 driver 并重载 MCP 配置；打开驾驶舱也会静默重探一次
  （在 dock 外面装好、授权好的 driver 这样接上）。
- `/mcps` 里禁用内置的 cua-driver 行时，Dock 把完整一行（`command` / `args` / `enabled = false`）
  落进用户 `config.toml`，此后该行归配置文件管。
- **`/usage` 的未命中按来源拆开**：会话总计折了子代理与压缩，而「上一轮」和每轮命中率
  走势只有主循环，两个口径并排摆着容易被读成「主循环每轮都在漏 token」。现在多一行
  `未命中来源: 主循环 N · 子代理 M · 压缩 K`，只有真有子代理 / 压缩时才出现。
- **压缩（auto-compact）开始记账**：它是一次整段历史、几乎零命中的满价请求，且之后那一轮
  必然全量重算。它隔离掉了 `"sessions"`，以前用量整笔掉进黑洞——`/usage` 里看不到开销，
  命中率掉格也无从归因。现在它进总计与 `模型调用`（标成「压缩 N」），但仍不是用户的一轮：
  不进 `numTurns`、不占走势格子。
- **不报 cache write 的 wire 上注明口径**：`chat/completions` 与 Responses 不单独上报
  `cache_creation`，这一轮新写进缓存的 token 全落在「未命中」里。overlay 现在在这一段标
  「含首次写入（这条 wire 不单列）」，不然会被当成和 Claude Code `/cost` 的 `input` 同一个口径。
- **`DOCK_CACHE_DEBUG`**：默认关的诊断日志。把每次请求与同一会话上一次逐条比对，第一处不同
  报下标与字节偏移，并把上游 read / write / miss 贴在同一条记录下，写进
  `$DOCK_HOME/scratch/cache-debug.log`（设成路径则写到该路径）。命中率掉下去时用来分辨
  「前缀真被改写了」还是「上游计数问题」。**日志含对话片段**，只在本机调试用。

### 修复

- **Stop 后不再把假文本留在历史里**：请求已发出但响应头还没回时按 Stop（插队发送也走这条），
  采样器以前返回 `"cancelled"` 占位文本，`finish_llm` 会把它填进本轮那条助手记录——历史里
  从此留着一句模型从没说过的话，`seal_incomplete_tool_calls` 不再弹出空槽位，cancel-rewind
  （把提示词还回输入框）也一起失效。现在取消返回空输出。

## [0.1.0] - 2026-09-14

首个公开版本。macOS（Apple Silicon / Intel）与 Linux（x86_64 / aarch64）有预编译二进制，
`install.sh` 一键安装；也可以从源码 `cargo run -p cordis-app`。

### 新增

- **一切皆插件**：内核 crate `cordis`（`Context`、`inject`、named service、waterfall、fiber
  生命周期），产品面由 `install_app` 挂成插件树；`agent/pre-step`、`agent/step-start`、
  `agent/turn-end`、`llm/stream`、`tools/execute`、`system-prompt/assemble` 六个扩展点。
- **Grok 外形 TUI**：主题、scrollback 卡片、prompt、overlay、底栏、快捷键；斜杠命令目录见
  `CLI.md`，模型工具见 `TOOLS.md`。
- **模型目录由 `config.toml` 声明**：一个端点可同时声明 `responses` / `chat_completions` /
  `messages` 三条协议并在运行时切；能力（`context_window`、`reasoning`、`supports_images`、
  单价）写在模型级；不写就按 128k 估、不发该参数。
- **工具与 MCP**：`search_tool` 走 BM25 索引、schema 去重与输出预算；MCP 是 fail-open ——
  没有 `[mcp_servers.*]` 时仍挂载，连不上不影响启动。
- **动态 Cordis 插件**：会话内定义 Package（预设或 Rhai）→ 运行 → 提升为磁盘插件，
  支持 `/` 命令、`tui.slots`、`agent/*` 扩展点；工作流见 `skills/cordis-plugin-development/`。
- **会话与恢复**：`~/.dock/sessions/<cwd>/` 落盘，`dock --resume [id]` 恢复。
- **浏览器 companion**：进程内 loopback 网关（默认不监听，`/pair` 开启），`dock.1` 协议 +
  `embed-sdk` 宿主页 SDK。
- **技能与工作流**：`skills/`、`.dock/skills/`、`~/.dock/skills/`、`.agents/skills/` 五层发现；
  内置技能编译期嵌入，启动物化到 `~/.dock/bundled/skills/`。

### 变更

- 对外二进制名定为 **`dock`**（此前是 `cordis-tui`）；新增 `dock --version`。
- 七个第一方 crate 的版本号统一由 `[workspace.package].version` 提供
  （各自改为 `version.workspace = true`），发版只需改一处。
- CI 只在 `main` push 与 PR 上跑门禁；发版另由 `.github/workflows/release.yml` 在 `v*` tag 上
  触发，先复述门禁并校验 tag 与 workspace 版本一致，再构建四平台产物。

### 已知限制

- 只发 macOS 与 Linux；Windows 未做适配验证。
- Linux 预编译产物动态链系统库，有两条运行下限：**glibc ≥ 2.39**（构建机是
  `ubuntu-24.04` / `ubuntu-24.04-arm`，两个架构同档）与 **OpenSSL 3**
  （`libssl.so.3` / `libcrypto.so.3`，来自 `cordis-spine` 的 reqwest `default-tls`）。
  实际可用范围约等于 Ubuntu 24.04+ / Debian 13+ / Fedora 40+；更老的发行版
  （Ubuntu 22.04、Debian 12、RHEL 9、Amazon Linux 2）请从源码构建。`install.sh` 装完会跑一次
  `dock --version`，跑不起来会直接报错退出，不会假装装好了。
- macOS 产物未做代码签名 / 公证，Gatekeeper 首次运行可能拦截（`install.sh` 会给出
  `xattr -d com.apple.quarantine` 提示）。
- 需要浏览器 companion 时，宿主页 SDK 要自己 build `embed-sdk/`（`npm ci && npm run build`），
  它不随二进制发布。
