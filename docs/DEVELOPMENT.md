# 开发

给人类和 agent 的同一份操作手册。硬规则在根 [AGENTS.md](../AGENTS.md)；架构在 [ARCHITECTURE.md](ARCHITECTURE.md)。

## 环境

| 需要 | 版本 | 说明 |
|---|---|---|
| rustc / cargo | **1.88+**（README 声明，未实测） | 依据是 `README.md` 的 badge 与「快速开始」（原文：`cordis-gateway` 在 1.85 编不过）。本仓没有 `rust-toolchain`、`Cargo.toml` 没有 `rust-version`、也没有 CI 兜底；本机只有 1.95，无法实测 1.85。用旧工具链编不过就升到 1.88+，不要改写法去迁就旧编译器。`Context::new()` 需要 tokio runtime |
| Node / npm | **>= 18** | 只给 `embed-sdk/`（`package.json` 的 `engines.node`） |
| 模型端点 | — | `~/.dock/config.toml` 或项目 `.dock/config.toml`，样例 `config.toml.example` |

没有可用模型时 spine 走 echo，TUI 仍能起。

## 第一次跑起来

```bash
git status -sb                 # 先确认工作区干净，别把别人的改动带进来
cargo run -p cordis-app        # 全屏接管终端
cargo run -p cordis-app -- --resume        # 恢复本 cwd 最近一次会话
cargo run -p cordis-app -- --resume <id>   # 指定会话
cargo run -p cordis-app -- --help
```

配置读取顺序：`~/.dock/config.toml` → 项目 `.dock/config.toml`（后者覆盖）。对话落盘在 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` + `chat_history.jsonl`）。

| 环境变量 | 作用 |
|---|---|
| `DOCK_HOME` | 用户配置目录，默认 `~/.dock` |
| `DOCK_MODEL` | 覆盖 `[models].default` |
| `DOCK_API_KEY` / `DOCK_API_BASE` | 覆盖当前模型的 key / base URL |
| `DOCK_GATEWAY_BIND` | 回环网关首选地址，默认 `127.0.0.1:18991`；占用则换下一个端口 |

不要把 `.dock/config.toml`、API key、`.env` 提交进仓库。

## 命令

```bash
# 构建与运行
cargo build -p cordis-app
cargo run -p cordis-app

# 测试：默认回归集合
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
cargo test -p cordis                      # 内核（目录 cordis-rust/，包名 cordis）
cargo test -p cordis-markdown             # markdown 渲染

# 单文件 / 单测
cargo test -p cordis-tui --test dispatch
cargo test -p cordis-spine --test round -- install_app_registers
cargo test -p cordis-spine --test dynamic -- <test_name>
cargo test -p cordis-spine --test subagents
cargo test -p cordis-gateway --test gateway

# lint / 格式
cargo clippy -p cordis-spine --all-targets
rustfmt --edition 2021 <改过的文件>
```

注意事项，都是当前仓库的真实状态：

- 仓库存量 clippy warning 与 `cargo fmt --all --check` diff 都存在（复现：`cargo clippy -p cordis-spine --all-targets`、`cargo fmt --all --check`）。**只对改过的文件跑 rustfmt**，不要 `cargo fmt --all` / `cargo clippy --fix` 全仓，那会制造无关 diff。
- 改行为只跑对应 crate，不要动辄全量。不要为了跑测试切 `--release`。
- `install_fakes` 保持 echo（`cordis-spine/tests/round.rs` 期望 `echoed: hello`）；测试默认不配 `mcp_servers`，`mcp_client` 仍挂载并 fail-open，不要改成默认连接。
- `vendor/` 里的 crate 是冻结副本，测试不过就当已知边界上报，不要就地改（见 [vendor/AGENTS.md](../vendor/AGENTS.md)）。

## 浏览器 SDK

```bash
cd embed-sdk
npm ci                 # 有 package-lock.json，用 ci 而不是 install
npm run build          # clean + tsc -b + vite build + standalone bundle
npm run dev:host       # 宿主页调试，127.0.0.1:19080
npm run build:types    # 只出 .d.ts
npm run build:bundle   # 只出 bundle
npm run pack:skin      # 打包 pet skin
```

产物 `embed-sdk/dist/dock-embed.js`。宿主页在 TUI `/pair` 开启回环网关后再打开。协议是 `dock.1`，细节见 [embed-sdk/README.md](../embed-sdk/README.md)。

## 在哪改

| 要做的事 | 落点 |
|---|---|
| 加 / 改模型工具 | `cordis-spine/src/` 的工具粒 + 同步 `TOOLS.md` |
| 加 / 改斜杠、快捷键、overlay | `cordis-tui/` + 同步 `CLI.md`；斜杠目录迭代 `cordis_tui::slash_catalog()` |
| 加 named service / waterfall 拦截 | `cordis-spine/`，`inject` 后在调用点 live-lookup |
| 动挂载顺序 | `install_app`（`cordis-spine`）或 `cordis-app` 的 `main` |
| 回环网关 / `dock.1` | `cordis-gateway/` |
| 宿主页注入 | `embed-sdk/` |
| 动态插件（会话内起一颗 Cordis 包） | skill `skills/cordis-plugin-development/SKILL.md` |
| Agent 预设 YAML | 内置在 `cordis-spine/presets/`；用户 / 项目覆盖在 `~/.dock/presets/`、`.dock/presets/` |

`.agents/skills/` 与 `skills/` 都会被运行时扫描（前者 Agents scope，后者 Bundled）。同名时项目 `.dock/skills` > `.agents/skills` > `~/.dock/skills` > `skills/`。

## 调试

- `/context` 与顶栏 live-lookup `ContextBook.window()`，按段看提示词 token 占用。
- `/mcps` 里 Space 立即 dispose / 重连，用于验证 MCP fail-open 与隐藏工具注册。
- `/pair` 看回环网关状态与实际端口；`[::1]` 绑失败会打 stderr 并出现在 `/pair` 与 `initialize.connection.companion`，不是静默失败。
- 会话问题先看 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` / `chat_history.jsonl`）。
- 改工具表后用 `cargo test -p cordis-spine --test round -- install_app_registers` 核对。

## 仓库流程 skills

- `.agents/skills/git-commit/SKILL.md` — 提交（Conventional Commits、只 stage 任务相关文件）
- `.agents/skills/create-pr/SKILL.md` — 开 PR
- `skills/cordis-plugin-development/SKILL.md` — 动态 Cordis 插件工作流

贡献流程见 [CONTRIBUTING.md](../CONTRIBUTING.md)；安全上报见 [SECURITY.md](../SECURITY.md)。
