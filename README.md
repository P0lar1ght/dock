<p align="center">
  <img src="assets/logo/mark.png" width="112" alt="Dock mark">
</p>
<h1 align="center">Dock</h1>
<p align="center">
  <b>通用的本地 Agent · 终端、桌面、浏览器同源</b>
</p>
<p align="center">
  <img src="assets/logo/lockup.png" width="360" alt="Dock">
</p>
<p align="center">
  <img src="https://img.shields.io/badge/rust-1.94+-dea584?logo=rust&logoColor=white" alt="Rust 1.94+">
  <img src="https://img.shields.io/badge/runtime-cordis-132238" alt="Cordis runtime">
  <img src="https://img.shields.io/badge/gateway-loopback-73dbff" alt="Loopback gateway">
  <img src="https://img.shields.io/badge/license-Apache_2.0-yellow" alt="Apache 2.0">
  <img src="https://github.com/P0lar1ght/dock/actions/workflows/ci.yml/badge.svg" alt="CI">
</p>
<p align="center">
  <a href="#安装">安装</a> •
  <a href="#快速开始">快速开始</a> •
  <a href="#特性">特性</a> •
  <a href="#架构概览">架构</a> •
  <a href="#桌面端">桌面端</a> •
  <a href="#浏览器-companion">浏览器</a> •
  <a href="#仓库">仓库</a> •
  <a href="#文档">文档</a>
</p>

<p align="center">
  <a href="assets/promo/dock-promo.mp4">
    <img src="assets/promo/dock-promo.gif" width="880" alt="Dock 桌面端演示：一句话做出销售看板；记住周报格式并在记忆关系图里连起来，下个会话先翻记忆再写周报；内置浏览器查 GitHub；经 cua-driver 在 Numbers 里填备注；Agent 预设与工具配置；插件在工具执行前拦下强推">
  </a>
  <br>
  <sub>50 秒演示（桌面端真实界面）：一句话做出看板 → 记忆：说一次就记住、连成关系图、下次先翻记忆 → 浏览器 → 桌面 CUA → Agent 预设 → 插件。<a href="assets/promo/dock-promo.mp4">高清 MP4</a></sub>
</p>

---

## 简介

**Dock 是一个通用的智能 Agent —— 不只写代码。** 研究、写作、自动化、数据分析，或者只是把一件麻烦事办完。

它跑在你自己机器上，用你自己的模型和密钥；真正让它不一样的是下面三点：

- **通用的** —— 工具面横跨文件、终端、网页、浏览器、本机桌面（CUA）与画布；MCP 把外部服务接进来，技能与工作流把成串的活交给它自己编排。
- **能自己把事做完** —— 不是单轮问答。计划模式先想再动，子代理并行分工，目标模式认领目标并推进到完成，记忆跨会话记住你是谁、做过什么。
- **能改的都在文件里** —— 模型、人设、工具权限、快捷键、主题、MCP 服务器落在能读、能 diff 的配置和 YAML 里。要改 agent **行为本身**（拦工具、设完成标准、加面板）再写插件。

同一套内核，三个前端：终端里的全屏 TUI、桌面上的 Tauri GUI、以及一行 `<script>` 就能嵌进任意网页的浏览器端。桌面端不是另一套实现，它拉起的是同一个 `dock` 二进制（`dock serve`），走同一套协议、共享同一份会话历史。

模型端点你自己配（`responses` / `chat_completions` / `messages` 三条协议都支持）。密钥走环境变量或 `$DOCK_HOME/secrets.json`；也可以 `/auth` 接 ChatGPT 或 Grok 的订阅额度（凭据只存在本机 `$DOCK_HOME/credentials/`）。Dock 自己不做账号与计费。会话历史在 `$DOCK_HOME`。

实现跑在 [Cordis](https://github.com/cordiverse/cordis) 上（crate `cordis`，Rust，不是上游 JS `cordis/` 的 fork）。设计见论文 [_A Programming Paradigm for Spatiotemporal Composability_](https://github.com/cordiverse/paper)。

---

## 特性

### 🖥️ 终端与桌面，同一个 Dock

- 终端 TUI：全屏、思考可折叠、工具卡片先输入后输出，计划 / 目标 / 子代理各有面板，斜杠命令带补全
- 桌面 GUI（Tauri 2 + React）：侧栏会话树、每一轮改动的文件清单与逐行 diff、子代理独立面板、审查视图、浏览器面板
- 桌面端启动即拉起一个无头 `dock serve`，走 dock.1 协议，ticket 只经 stdout 交接
- 浏览器嵌入：一行 `<script>` 注入 `dock-embed.js`，任意页面变成对话入口；配对走一次性 ticket + 回环
- 三端共享同一份会话历史：终端建的会话，桌面端接着聊，反之亦然

### 🎛 高度自定义

- 模型目录是文件不是菜单：`config.toml` 描述端点、协议、上下文窗口、推理档、单价；密钥走 `env_key` 或 `secrets.json`，不落明文
- 一个端点多条协议：`api_backends` 声明 `responses` / `chat_completions` / `messages`，`/protocol` 现场切换，不必为每条协议重复配一个模型
- Agent 人设是 YAML 目录：`.dock/presets/<id>/agent.yml` 就是一个人格，包含它能用的工具白名单；`agents/<type>.yml` 定义子代理角色
- 规约分层：工作区根的 `AGENTS.md`、用户级 `~/.dock/AGENTS.md`、关于你的 `~/.dock/USER.md` —— 追加不覆盖，内容变了才重注
- 快捷键、主题、权限档位、推理强度都能改：`/settings` 里改，或直接改配置
- 跨会话记忆默认关；开了以后按全局 / 工作区分层存储，FTS + 可选向量混合检索

### 🔌 插件：改 agent 行为，不是万能接口

插件包在模型外面，每一轮、每一次工具调用都会经过它。MCP 做不到这件事——MCP 要等模型来调。

| 要的 | 用什么 | 别这样做 |
|---|---|---|
| 「以后 / 每次 / 不许 / 必须」变成保证 | 插件钩子 | 写进记忆或提示词（模型可能不照做） |
| 已有 Claude Code / Codex 的 hooks | 用户级 `~/.dock/config.toml` 的 `[hooks]` 打开，`/hooks` 查看 | 再包一层插件 |
| 读写文件、跑命令、抓网页 | 现成工具 | 包一层 HTTP/工具壳 |
| 一套可复用的做事说明 | 技能（`SKILL.md`） | — |
| 多阶段、多代理编排 | 工作流（Rhai） | — |
| 换人设、工具名单、子代理名册 | 预设 YAML | — |
| 给别的客户端也用的外部能力 | MCP 服务器 | — |

钩子能做的：

- `tools/pre-execute`：改写、改道、拒绝，或要用户确认（自动批准下也问）
- `agent/turn-end`：没满足完成标准就不许收尾
- `agent/pre-step` / `agent/step-start`：开轮前拒绝，或每步采样前注入提醒
- `tools/execute`：改已经发生的工具结果（例如裁输出）
- `system-prompt/assemble`：追加一段系统提示，不是整份替换
- `host.card` / `host.every`：一轮下面挂卡片；定时检查外部状态并叫醒 agent

另外可以登记工具、斜杠、面板、状态项、设置卡。目录是项目 `.dock/plugins/<id>/`（覆盖用户级 `~/.dock/plugins/<id>/`），`cordis_plugin reload` 热装；新版跑不起来时上一版照跑。写法见技能 `cordis-plugin-development`。

界面边界：视图树（面板、工具卡、状态项、设置卡）TUI 和桌面端都会画；**web 面板**（插件自带 HTML）只在桌面端，终端退回普通视图。

改道逃不出预设工具集：`tools/pre-execute` 之后权限门读的是**改写后的名字**。

### 🛠️ 工具与 MCP

- 工作区七件套：`list_dir` `read_file` `grep` `search_replace` `bash` `glob` `write_file`
- MCP：stdio 与 Streamable HTTP 都接。工具登记进同一张 `"tools"` 表，但经 `search_tool` / `use_tool` 渐进披露，**不进**模型采样工具表
- 内置浏览器 MCP（chromiumoxide CDP，按会话分标签页，22 颗工具）与内置白板 MCP App（Excalidraw，你和 Agent 同画一张图）
- MCP Apps：`io.modelcontextprotocol/ui` —— 服务器可以带自己的界面；服务器还能反过来向用户提问（elicitation），表单与链接模式都走 Dock 的弹窗
- 桌面驾驶：cua-driver 接管本机键鼠，与 `bash` 同级权限门
- 另有：计划、提问、画布、后台任务、调度、子代理、目标、记忆、LSP、工作流、压缩

### 🧠 会话编排

- 子代理：`task` 派活，`send_message` 插话，`interrupt_agent` 打断；对话独立落盘，关掉再回来能接着聊
- 目标：模型自己认领目标并推进，直到 `update_goal(completed)`；可设时长与请求数预算
- 调度：`/loop` 定时提问，任务落盘、跨重启继续跑
- 工作流：Rhai 脚本编排多阶段多代理。内置 `/code-review`（一个审查员通读 + 一个核实员逐条核对）、`/deep-research`
- 多会话同屏：`/dashboard` 并排盯多个会话，peek 面板直接给选中的会话派活
- 插话分叉：`/btw` 开一条只读旁支问一句，答案不进主线上下文

### 🔒 权限与计划门

- 危险动作默认问一句。Shift+Tab 在「询问 → 计划 → 始终允许」之间循环；计划是独立模式，不是第三档权限
- 计划态挡住 bash 与写文件，退出要你批
- 桌面端给每一轮改动记账：文件清单、逐行 diff、撤销回到该轮改完的样子（该轮之后文件没被再改过才生效）。终端 `/undo` 只撤回还没模型输出的那条用户消息

---

## 架构概览

终端里的 `dock` 画 TUI；桌面端拉起无头的 `dock serve`，和宿主页一样只连回环 Gateway（协议 `dock.1`），不另起一套循环。会话历史都落在 `$DOCK_HOME`，所以哪一端建的会话，另一端都能接着聊。

`cordis-app` 起一个运行时：先 `install_app` 挂会话、模型、工具、权限、MCP 等能力，再挂系统提示基座、agent 循环、会话队列、网关、定时驱动和界面。TUI 的 `/pair` 才让网关开始监听（默认 `127.0.0.1:18991`，占用换端口）；`dock serve`（桌面 GUI 的子进程）挂载即听，ticket 只经 stdout 交给父进程。细节、挂载顺序与磁盘布局见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

```mermaid
flowchart TB
  person(["人"])
  term["终端"]
  gui["桌面 GUI"]
  host["宿主页 · dock-embed.js"]

  subgraph app["dock 进程（终端 / dock serve）"]
    direction TB
    tui["TUI"]
    gw["gateway · 回环"]
    actor["session_actor · 队列"]
    aloop["agent-loop"]
    tools["工具 + MCP"]
  end

  person --> term --> tui
  person --> gui -->|"dock serve · stdout ticket"| gw
  person --> host -->|"loopback HTTP / WS"| gw

  tui --> actor
  gw --> actor
  tick["调度 · 到点提交"] --> actor
  actor -->|"跑一轮"| aloop
  aloop --> tools
  aloop --> api["模型 API"]
  tools --> mcp["MCP stdio / HTTP"]
  actor --> disk["$DOCK_HOME · 项目 .dock"]

  classDef surface fill:#e8f2ff,stroke:#3b82c4,color:#0b1f33;
  classDef core fill:#fff3e0,stroke:#d98a2b,color:#3a2a10;
  classDef ext fill:#eef0f2,stroke:#8a9199,color:#2a2f36;
  class tui,gw surface;
  class actor,aloop,tick core;
  class api,mcp,disk ext;
```

约定、工具名单、斜杠分别见 [AGENTS.md](AGENTS.md)、[TOOLS.md](TOOLS.md)、[CLI.md](CLI.md)。

### 一轮怎么跑

按键和网关请求都变成会话命令，交给 `session_actor` 排队；actor 每次取一条跑 agent 循环。默认没有步数上限，连着原样重复同一步才会被停下。

```mermaid
flowchart LR
  start(["用户 prompt / 续跑"]) --> pre["开轮"]
  pre --> asm["组装系统提示"]
  asm --> step["每步提醒"]
  step --> stream["采样"]
  stream -->|"工具调用"| gate["插件拦一层"]
  gate --> perm["计划门 / 权限门"]
  perm --> exec["执行"]
  exec -->|"结果回灌"| step
  stream -->|"文本"| endw["完成标准"]
  endw -->|"没过，续跑"| step
  endw -->|"过了"| done(["回合结束"])

  classDef hub fill:#fff3e0,stroke:#d98a2b,color:#3a2a10;
  class stream,exec hub;
```

系统提示每轮组装一次，不随工具轮次重跑；每一步采样前可以再注入提醒。出工具调用先给插件改写 / 改道 / 拒绝，再过计划门与权限门，结果回灌后进入下一步；出文本后走完成标准，没过就续跑下一步。计划 / 目标 / 工作区 `AGENTS.md` 走历史里的 `<system-reminder>`，不进系统提示。`/context` 按段看占用。

---

## 安装

macOS（Apple Silicon）与 Linux（x86_64 / aarch64）有预编译二进制，不需要 Rust 工具链（Intel Mac 请自行从源码构建）：

```bash
curl -fsSL https://raw.githubusercontent.com/P0lar1ght/dock/main/install.sh | sh
```

脚本从 [GitHub Release](https://github.com/P0lar1ght/dock/releases) 取对应平台的
`dock-<target>.tar.gz`，校验 sha256 后装进 `~/.local/bin`（不写系统目录、不需要 sudo）。

| 变量 | 作用 |
|---|---|
| `DOCK_VERSION` | 指定版本，如 `v0.1.0`；默认 latest |
| `DOCK_INSTALL_DIR` | 安装目录，默认 `~/.local/bin` |

变量要写在 `sh` 前面，写在 `curl` 前面只会传给 `curl`、脚本收不到：

```bash
curl -fsSL https://raw.githubusercontent.com/P0lar1ght/dock/main/install.sh | DOCK_VERSION=v0.1.0 sh
```

装完脚本会跑一次 `dock --version` 自检，跑不起来直接报错退出；随后默认执行 `dock init`
（幂等，不覆盖已有 `config.toml`；可用 `DOCK_SKIP_INIT=1` 跳过）。TTY 下分「下载 /
安装 / 初始化」三阶段进度，管道非 TTY 降级为朴素输出。macOS 产物未做代码签名 / 公证，
首次运行若被 Gatekeeper 拦下，按脚本提示执行一次
`xattr -d com.apple.quarantine "$(command -v dock)"` 即可。

装好后建议：

```bash
dock doctor          # PATH / 配置 / 模型目录 / quarantine·glibc
dock upgrade --check # 是否有新版本（不安装）
# dock upgrade       # 原子升级；有进行中会话需 --force
```

Linux 产物动态链系统库，要求 **glibc ≥ 2.39** 与 **OpenSSL 3**（约等于 Ubuntu 24.04+ /
Debian 13+ / Fedora 40+）；更老的发行版与 Windows 暂无可用产物，请见下方「快速开始」从源码构建。

---

## 快速开始

需要 **Rust 1.94+**（版本下限说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)）和本机 API key，或在配置里写好端点。

```bash
# 从源码
cargo run -p cordis-app
cargo run -p cordis-app -- --resume          # 恢复本 cwd 最近一次会话
cargo run -p cordis-app -- --resume <id>     # 恢复指定会话

# 已安装的二进制
dock
dock --resume
```

首次启动会全屏接管终端。浏览器 companion 在 TUI 里 `/pair` → Enter 开启回环网关后再打开宿主页。模型目录读 `~/.dock/config.toml`，再读项目 `.dock/config.toml`（后者覆盖）。样例：[`config.toml.example`](config.toml.example)。

最小配置长这样——密钥从环境变量取，不写进文件：

```toml
[models]
default = "my-model"

[model."my-model"]
name = "我的模型"
api_base_url = "https://<your-endpoint>/v1"
api_backends = ["responses", "chat_completions"]
api_model = "<上游真实模型名>"
env_key = "MY_API_KEY"
context_window = 200000
```

| 变量 | 作用 |
|---|---|
| `DOCK_HOME` | 用户配置目录，默认 `~/.dock`（含 `sessions/<cwd>/` 对话 jsonl） |
| `DOCK_MODEL` | 覆盖 `[models].default` |
| `DOCK_API_KEY` / `DOCK_API_BASE` | 覆盖当前模型的 key / base URL |
| `DOCK_GATEWAY_BIND` | 回环网关首选地址，默认 `127.0.0.1:18991`；`/pair` 开启时占用则换下一个端口 |

没有可用模型时不会连上游，`/model` 会提示去写 `config.toml`。不要把 `.dock/config.toml`、API key、`.env` 提交进仓库。

测试：

```bash
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway -p dock-memory -p cordis-browser -p cordis-apps
cargo test -p cordis
cargo test -p cordis-spine --test round -- install_app_registers
```

核对工具表是否还对，跑上面这条 `install_app_registers`。

---

## 桌面端

`dock-gui/` 是 Dock 的 Tauri 2 + React 桌面端，与内核同仓库、独立工程。它启动时拉起一个无头 `dock serve`，ticket 经 stdout 交给 Tauri，前端经回环 WebSocket 走 dock.1 —— 一个 Dock 进程，跨目录列会话，每个会话用自己的 cwd。

需要本机 **Rust 1.94+**、**Node ≥ 20.19**（Vite 8 的下限）与 Tauri 的系统依赖。先在仓库根构建二进制：

```bash
cargo build -p cordis-app

cd dock-gui
npm install
npm run tauri dev      # 起窗口
npm run build          # tsc + vite
```

开发构建默认用 `../target/debug/dock`，可用 `DOCK_BIN` 指定别的可执行文件、`DOCK_GUI_CWD` 指定启动目录。

界面能力：侧栏会话树（跨项目）、子代理独立页与逐条对话、每轮改动卡（文件清单 + 逐行 diff + 撤销）、审查面板（工作区 git diff 或单轮改动，批注落行）、浏览器面板（页面渲染 + 截图）、白板（MCP App）、预设编辑器、模型与密钥设置、排程页。设计稿在 `dock-gui/wonder-ui/`（只读），进度在 `dock-gui/docs/PROGRESS.md`。

> 桌面端目前**从源码构建**，发布产物暂不含 GUI 二进制。

---

## 浏览器 companion

同一进程里的 gateway 只绑 loopback，**默认不监听**。在 TUI `/pair` 里开启后再打开宿主页。首选端口占用时自动换下一个（overlay 显示实际地址）。

构建 `embed-sdk/` 需要 Node >= 18（`embed-sdk/package.json` 的 `engines.node`）。

```bash
cd embed-sdk
npm ci
npm run build
```

打开 [`embed-sdk/examples/inject/index.html`](embed-sdk/examples/inject/index.html)，在 TUI 里 `/pair`（或首次连接弹出的 overlay）批准该 Origin。

```html
<script
  src="/vendor/dock/dock-embed.js"
  data-dock-auto
  data-application="example-app"
  data-gateway-url="http://127.0.0.1:18991"
  data-skin="dudu">
</script>
```

协议与属性见 [`embed-sdk/README.md`](embed-sdk/README.md)。TUI overlay 类命令（`/cd` `/settings` `/pair` …）会回「请在 Dock 终端使用」。

---

## 仓库

第一方 crate 多为 `cordis-*`；`dock-memory`、`dock-core`、`dock-gui` 与二进制 `dock` 用产品名。其余是入口、注入与冻结副本。

```
dock/
├── cordis-rust/          # 运行时内核 crate `cordis`：Context、inject、named services
├── cordis-base/          # 底座：wire 类型、config.toml、纯引擎（不挂能力）
├── cordis-spine/         # Agent 循环、工具、MCP、会话、预设
├── cordis-tui/           # 全屏终端 UI
├── cordis-gateway/       # 回环 HTTP/WS：配对与 dock.1 投影
├── cordis-app/           # 二进制入口
├── cordis-browser/       # 内置浏览器 MCP 服务（chromiumoxide CDP）
├── cordis-apps/          # 内置 MCP Apps 服务（白板：Excalidraw）
├── dock-memory/          # 跨会话记忆引擎（默认关）
├── dock-render/          # Markdown / Mermaid（crate 名暂仍 cordis-markdown / xai-grok-mermaid）
├── dock-core/            # dock.1 协议核心（TS）：类型化线程事件 + reducer，无 DOM
├── dock-gui/             # 桌面 GUI（Tauri 2 + React）
├── embed-sdk/            # 宿主页 JS 注入（dock-embed.js）
├── examples/             # 插件示例：activity / trajectory
├── vendor/               # 冻结副本（mermaid 栈在 dock-render/third_party/）
├── skills/               # 通用技能（mcp-builder 等），不进二进制
├── .agents/skills/       # 仓库流程 skills（Agents scope）
├── docs/                 # 架构与开发细节
├── assets/               # 品牌图、宣传片（promo/）
├── CHANGELOG.md          # 用户可见变更
├── LICENSE               # Apache-2.0（第一方 crate）
├── NOTICE                # 版权归属 + vendor/ 第三方授权清单
└── config.toml.example   # 用户 / 项目模型目录样例
```

内置技能（`cordis-plugin-development`、`dock-guide` 等）在 `cordis-spine/src/tools/skills/builtin/`，启动时物化到 `$DOCK_HOME/bundled/skills/`。

---

## 文档

| 文档 | 说明 |
|---|---|
| [AGENTS.md](AGENTS.md) | 根政策：命令、边界、提交约定；给写代码的 agent / 人（`CLAUDE.md` 指向它） |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 目录地图、不变式、磁盘布局 |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | 环境、命令、测试、调试、落点 |
| [docs/REMOTE.md](docs/REMOTE.md) | 远程访问：设备令牌、反向代理 |
| [docs/HOOKS.md](docs/HOOKS.md) | command hook（Dock / Claude Code / Codex 格式） |
| [docs/PLUGIN-VIEWS.md](docs/PLUGIN-VIEWS.md) | 插件视图树：跨端渲染协议 |
| [docs/CHATGPT-SIWC.md](docs/CHATGPT-SIWC.md) | ChatGPT 订阅通道 |
| [docs/GROK-SUBSCRIPTION.md](docs/GROK-SUBSCRIPTION.md) | Grok 订阅通道 |
| [TOOLS.md](TOOLS.md) | 模型工具索引、缺口、明确不做（单颗细节在 [docs/tools/](docs/tools/)）|
| [CLI.md](CLI.md) | 斜杠、快捷键、overlay、底栏 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 人类贡献流程 |
| [SECURITY.md](SECURITY.md) | 漏洞上报与安全边界 |
| [CHANGELOG.md](CHANGELOG.md) | 用户可见变更（Keep a Changelog 格式） |
| [embed-sdk/README.md](embed-sdk/README.md) | 浏览器 SDK 属性、事件、配对 |
| [dock-gui/README.md](dock-gui/README.md) | 桌面端结构与开发 |
| [cordis-spine/src/tools/skills/builtin/cordis-plugin-development/SKILL.md](cordis-spine/src/tools/skills/builtin/cordis-plugin-development/SKILL.md) | 写插件：能力边界、流程、host API（内置技能） |
| [.agents/skills/](.agents/skills/) | 仓库流程 skills：git-commit、create-pr |

Crate README 管该包的 API。产品面的权威清单：`TOOLS.md`（模型工具）、`CLI.md`（斜杠 / 快捷键 / overlay）；实现细节见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

---

## 致谢与溯源

Dock 的实现过程中参考了这些开源项目：

- [xai-org/grok-build](https://github.com/xai-org/grok-build)（Apache-2.0）—— 终端 Agent 的工具实现、采样协议与渲染栈有大量参考与拷贝
- [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（MIT）—— 插件模型与 host API 契约
- [cordiverse/cordis](https://github.com/cordiverse/cordis)（MIT）—— 组合式运行时，见论文 [_A Programming Paradigm for Spatiotemporal Composability_](https://github.com/cordiverse/paper)

第三方拷贝清单见 [NOTICE](NOTICE)，冻结副本规则见 [vendor/README.md](vendor/README.md)。

工作只在这棵树里：不改、不 path-dep 外部的 `grok-build/`、`deepseek-harness/`、上游 `cordis/`；要源码就复制进 `vendor/xai/` 或对应 crate。

---

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=P0lar1ght/dock&type=Date)](https://star-history.com/#P0lar1ght/dock&Date)
