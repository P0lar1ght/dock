# 开发

给人类和 agent 的同一份操作手册。硬规则在根 [AGENTS.md](../AGENTS.md)；架构在 [ARCHITECTURE.md](ARCHITECTURE.md)。

## 环境

| 需要 | 版本 | 说明 |
|---|---|---|
| rustc / cargo | **1.88+** | 由根 `Cargo.toml` 的 `[workspace.package].rust-version = "1.88"` 声明，第一方 crate 用 `rust-version.workspace = true` 继承（`vendor/` 冻结副本不继承）。`README.md` 的 badge 与「快速开始」同步声明（原文：`cordis-gateway` 在 1.85 编不过）。用旧工具链编不过就升到 1.88+，不要改写法去迁就旧编译器。`Context::new()` 需要 tokio runtime |
| Node / npm | **>= 18** | 只给 `embed-sdk/`（`package.json` 的 `engines.node`） |
| 模型端点 | — | `~/.dock/config.toml` 或项目 `.dock/config.toml`，样例 `config.toml.example` |

没有可用模型时 spine 走 echo，TUI 仍能起。

## 第一次跑起来

```bash
git status -sb                 # 当前分支与未提交改动
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
| `DOCK_CACHE_DEBUG` | 默认关。`1` / `on` / `true` 写 `$DOCK_HOME/scratch/cache-debug.log`，设成路径则写到该路径。把每次 LLM 请求与同一会话上一次逐条比对，第一处不同报下标与字节偏移，并把上游 read / write / miss 贴在同一条记录下（**日志含对话片段**，只在本机调试用） |

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
cargo fmt --check -p cordis -p cordis-spine -p cordis-tui -p cordis-gateway -p cordis-app -p cordis-markdown -p xai-grok-mermaid
cargo clippy -p cordis-spine --all-targets --no-deps -- -D warnings
```

注意事项，都是当前仓库的真实状态：

- 第一方 crate 已 rustfmt-clean，CI 会跑上面的格式门禁。**不要对 `vendor/` 跑 rustfmt**（冻结副本，见 [vendor/AGENTS.md](../vendor/AGENTS.md)）。
- 第一方 crate 的 clippy 已清零，`-D warnings` 是 CI 门禁。命令要带 **`--no-deps`**：workspace 成员里有 `vendor/` 冻结副本，不带就会被一起 lint，然后被上游既有 warning 打红。新增代码要么真消掉 warning，要么在那一处 `#[allow(clippy::…)]` 并写清理由 —— 不要往 workspace 级 lint 配置里塞 allow。
- `too_many_arguments` / `large_enum_variant` / `result_large_err` 这类是设计取舍，仓库当前一律**逐处 allow + 理由注释**，不动签名；要抽结构体或 boxing 就单独提 PR。
- 改行为只跑对应 crate，不要动辄全量。不要为了跑测试切 `--release`。
- `install_fakes` 保持 echo（`cordis-spine/tests/round.rs` 期望 `echoed: hello`）；测试默认不配 `mcp_servers`，`mcp_client` 仍挂载并 fail-open，不要改成默认连接。
- `vendor/` 里的 crate 是冻结副本，测试不过就当已知边界上报，不要就地改（见 [vendor/AGENTS.md](../vendor/AGENTS.md)）。
- `cordis-spine` 的测试改进程级 env / cwd 必须走 `crate::test_env::scoped()`（单一进程锁 + drop 还原）。不要各模块自建 `static Mutex`，私锁之间不互斥，正是并行随机红的成因。

## 构建速度

等的从来不是测试本身（`cordis-spine` 693 个 lib 用例跑完 10 秒），是**重编与重链**。三条实测有效的：

**1. 给 clippy 单独的 target 目录。** clippy 走 `RUSTC_WORKSPACE_WRAPPER`，指纹里的 `rustc` 哈希和 `cargo test` 不是一个值——**在同一个 target 里 `clippy` 与 `test` 来回切，每切一次都把 200MB 的 rlib 重编一遍**。分开就是两份常热缓存：

```bash
CARGO_TARGET_DIR=target/clippy cargo clippy -p cordis-spine --all-targets --no-deps -- -D warnings
```

**2. 迭代时别跑全量。** 改哪块跑哪块，全量留到提交前一次：

```bash
cargo test -p cordis-spine --lib project_instructions   # 只跑这一个模块的单测
cargo test -p cordis-spine --test instructions          # 只链这一个集成二进制
```

`cordis-spine` 有 5 个集成测试文件 = 5 个上百 MB 的可执行文件，全量测要挨个链一遍。

**3. `[profile.dev] debug = "line-tables-only"`**（已在根 `Cargo.toml`）。调试信息原本占产物的绝大头。backtrace 的「文件:行」还在，测试和 panic 定位不受影响；要用 lldb 看局部变量就跑 `--profile dev-debug`。

**定期清。** `target/debug/incremental` 与 `deps` 会堆到几十上百 GB（本仓库见过 254 GiB），磁盘压力本身在拖慢构建。改 `[profile.*]` 会让**所有** crate 的指纹失效（依赖也算），那时正是 `cargo clean` 的时机；平时用 `cargo clean -p <crate>` 或 `cargo-sweep`。

## CI

`.github/workflows/ci.yml` 在 `main` 的 push 与所有 PR 上跑四步：

```bash
cargo fmt --check -p cordis -p cordis-spine -p cordis-tui -p cordis-gateway -p cordis-app -p cordis-markdown -p xai-grok-mermaid
cargo clippy --locked -p cordis -p cordis-spine -p cordis-tui -p cordis-gateway -p cordis-app -p cordis-markdown -p xai-grok-mermaid --all-targets --no-deps -- -D warnings
cargo test --locked -p cordis-gateway -p cordis-tui -p cordis-app
cargo test --locked -p cordis-spine
```

工具链用 rustup 的 stable，仓库下限由 `[workspace.package].rust-version` 兜底。代理环境要放行回环地址（见上文 `no_proxy`）。

改 workflow 后先本地校验：

```bash
actionlint .github/workflows/ci.yml
```

`env` 的 key 大小写不敏感，`no_proxy` 与 `NO_PROXY` 同时写会被判重复 key，workflow 整体解析失败。

CI 不跑的：

- 标了 `#[ignore]` 的用例：`browser` 里 4 个要真实 Chrome 的冒烟（`p0_` / `p1_` / `p2_` / `open_close_`）。页面文本与快照随 Chrome 版本、界面语言变化 —— 例如中文本地化下 `<input type=file>` 的标签是「选择文件」，而 `p1_` 的过滤器只匹配 ASCII `file`。要跑就本地跑：

  ```bash
  cargo test -p cordis-spine --lib -- --ignored           # 全跑，需要装 Chrome
  cargo test -p cordis-spine --lib -- p1_ --ignored       # 单个
  ```

- `embed-sdk` 的 js 检查：目前没有 lint / test 脚本，类型检查就是 `npm run build:types`。

spine 测试怎么隔离进程级 env：多个模块会临时改 `DOCK_HOME` 与当前目录做隔离，而这两个都是进程级全局。模块各持一把私有锁时锁与锁之间不互斥，表现为 `session_persist::tests` 与 `skills::tests` 随机红。仓库统一用 `cordis-spine/src/test_env.rs` 的 `scoped()`：

```rust
let _home = crate::test_env::scoped().home();                  // 临时 DOCK_HOME
let _cwd = crate::test_env::scoped().cwd(dir.path());          // 切 cwd
let _env = crate::test_env::scoped().set("CHROME_PATH", "/x"); // 设 / 删单个变量
```

一次 `scoped()` 只拿一把锁，所以不要嵌套调用（会自锁）。集成测试用不到 crate 私有模块，`tests/round.rs` 里自带同一把文件级锁。

## 发布

`.github/workflows/release.yml` 在 `v*` tag 上跑：先复述 CI 的四道门禁并校验 tag 与
`[workspace.package].version` 一致，再矩阵构建四个平台，最后把 `dock-<target>.tar.gz` 与
`<...>.sha256` 挂到同名 Release（`gh release create --generate-notes`）。`install.sh` 只认这套
产物名与 `tar` 里的 `dock` 二进制，改任意一边都要同步另一边。

| target | 构建机 |
|---|---|
| `aarch64-apple-darwin` | `macos-14` |
| `x86_64-apple-darwin` | `macos-14`（交叉编译，macOS 构建机只有 arm64） |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |

Windows 未做适配验证，不发。

两个 Linux 目标必须钉在同一档 Ubuntu：产物动态链 glibc，构建机的 glibc 就是运行下限
（24.04 是 2.39），一高一低会出现「同一台发行版 x86_64 能跑、arm64 报 `GLIBC_2.38 not
found`」。同理，`cordis-spine` 的 reqwest 走 `default-tls`（Linux 上即 OpenSSL），产物动态链
`libssl.so.3` / `libcrypto.so.3`，OpenSSL 1.1 的发行版跑不起来。要放宽这两条下限得换静态方案
（musl 目标，或 spine 改 `rustls-tls`），0.1.0 先按已知限制记在 `CHANGELOG.md`。

发版步骤：

1. 改根 `Cargo.toml` 的 `[workspace.package].version` —— 第一方 crate 全是
   `version.workspace = true`，一处生效；同步 `CHANGELOG.md`。
2. 本地跑默认回归集合与格式门禁。
3. `git tag v0.x.y && git push origin v0.x.y`，剩下的交给 workflow。

本地校验与干跑：

```bash
actionlint .github/workflows/release.yml
shellcheck install.sh

# 安装脚本干跑：把产物与 .sha256 放进一个目录，让它当 Release 用
mkdir -p /tmp/rel
tar -czf /tmp/rel/dock-aarch64-apple-darwin.tar.gz -C target/release dock
(cd /tmp/rel && shasum -a 256 dock-aarch64-apple-darwin.tar.gz > dock-aarch64-apple-darwin.tar.gz.sha256)
DOCK_BASE_URL=file:///tmp/rel DOCK_INSTALL_DIR=/tmp/dockbin sh install.sh
```

macOS 产物未签名 / 未公证，Gatekeeper 会拦首次运行；`install.sh` 检测到 quarantine 标记时打印
一次 `xattr -d com.apple.quarantine`，不替用户改（要彻底解决得上 Apple 开发者账号做签名 + 公证）。

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

`.agents/skills/` 与 `skills/` 都会被运行时扫描（前者 Agents scope，后者 Bundled）。同名时项目 `.dock/skills` > `.agents/skills` > `~/.dock/skills` > `skills/` > 内置 `$DOCK_HOME/bundled/skills/`（编译期嵌入，启动物化到缓存）。

## 调试

- `/context` 与顶栏 live-lookup `ContextBook.window()`，按段看提示词 token 占用。
- `/mcps` 里 Space 立即 dispose / 重连，用于验证 MCP fail-open 与隐藏工具注册。
- `/pair` 看回环网关状态与实际端口；`[::1]` 绑失败会打 stderr 并出现在 `/pair` 与 `initialize.connection.companion`，不是静默失败。
- 会话问题先看 `$DOCK_HOME/sessions/<cwd-key>/<id>/`（`meta.json` / `chat_history.jsonl`）。
- 命中率掉下去时开 `DOCK_CACHE_DEBUG=1`（见上面的环境变量表），看 `$DOCK_HOME/scratch/cache-debug.log`：它把每次请求与同一会话上一次逐条比对，第一处不同报下标与字节偏移，并把上游的 read / write / miss 贴在同一条记录下，用来分辨「前缀真被改写了」还是「上游计数问题」。
- 改工具表后用 `cargo test -p cordis-spine --test round -- install_app_registers` 核对。
- `cordis-gateway` 的测试全程打 loopback HTTP（`127.0.0.1` 与 `[::1]`）。环境里设了 HTTP 代理时，必须把回环地址放进 `no_proxy` / `NO_PROXY`，否则请求会被代理接管，症状是成片 502 与无响应体。

## 仓库流程 skills

- `.agents/skills/git-commit/SKILL.md` — 提交（Conventional Commits、只 stage 任务相关文件）
- `.agents/skills/create-pr/SKILL.md` — 开 PR
- `skills/cordis-plugin-development/SKILL.md` — 动态 Cordis 插件工作流

贡献流程见 [CONTRIBUTING.md](../CONTRIBUTING.md)；安全上报见 [SECURITY.md](../SECURITY.md)。
