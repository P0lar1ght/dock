# 命令沙箱

`bash` / `monitor` 跑的命令可以放进系统沙箱：
能写的只有工作区和几处临时 / 缓存目录，
读不到密钥，改不了 `.git/hooks`、`.git/config`、`.dock`。
沙箱里的命令不再弹权限框。
要出沙箱的命令照常问你。

默认关。`/sandbox` 开当前页，
`$DOCK_HOME/config.toml` 写 `[sandbox] enabled = true` 让新开的页默认开。

引擎在 crate `dock-sandbox`（不含插件），
接进 Dock 的是 spine 的 `sandbox` 插件（`cordis-spine/src/host/sandbox.rs`）。

## 开着时什么变了

| 场景 | 沙箱关 | 沙箱开 |
|---|---|---|
| `bash` / `monitor`（询问档） | 每条弹框 | 不弹，在沙箱里跑 |
| 模型传 `"sandbox": false` | 同上 | 弹框，写明「不进沙箱运行」；批准后在沙箱外跑 |
| `write_file` / `search_replace` 写工作区内 | 弹框 | 不弹 |
| 写工作区外、或写 `.git/hooks` 等受保护路径 | 弹框 | 弹框 |
| `read_file` / `grep` / `glob` / `list_dir` 读 `~/.ssh` 等 | 不弹 | 弹框 |
| 计划模式、只读子代理、`/btw` 里的命令 | 只读命令直接跑，别的必须你点头 | 全部在**只读沙箱**里跑（只能写临时目录、不联网），不弹 |
| 自动批准档 | 全放行 | 沙箱里照样跑；出沙箱也自动放行 |

命令像是被沙箱拦下时（退出码非零、输出里有
`Operation not permitted` 之类），结果末尾会接一句
`[沙箱] …`，告诉模型可以加 `"sandbox": false` 重跑。

后端不可用时（见下），开着也不起作用：
命令照常走权限门，不会不加防护地自动跑。
`/sandbox status` 会说原因。

## 边界

**可写**

- 工作区（这一页的会话 cwd）。
- 临时目录：`/tmp`、`/var/tmp`、`$TMPDIR`。
- `$DOCK_HOME/scratch`（workflow 的报告写在这里）。
- `$CARGO_TARGET_DIR`（设了的话；几个工作树共用构建目录时它在工作区外）。
- 依赖缓存（`build_caches = true`，默认）：
  `~/.cargo/registry`、`~/.cargo/git`、cargo 的锁文件、`~/.npm`、`~/.cache`、
  `~/Library/Caches`、`~/.pnpm-store`、`~/go/pkg/mod`、`~/.gradle/caches`、`~/.m2/repository`。
- `[sandbox] writable_roots` 里加的目录。

**可写目录里也只读**

- 工作区的 `.git/hooks`、`.git/config`（`.git` 存在时），
  子模块的 hooks / config，工作树的 `.git` 指针文件。
- 工作区的 `.dock`（不存在也保护，第一次创建要过权限门）。
- `$DOCK_HOME` 下除 `scratch` 以外的一切。
- 家目录下 shell 启动文件、`~/.gitconfig`、`~/.cargo/config.toml`、
  `~/.local/bin`、`~/Library/LaunchAgents` 等。

这些都是沙箱**外**的程序之后会照着执行的东西：
git 跑 hooks、照 config 跑过滤器，
Dock 下次启动照 `.dock/config.toml` 拉起 MCP 服务器。
它们只在家目录落进可写目录时（在 `~` 里开 Dock）才真起作用。

**读不到**（读写都拒）

- `~/.ssh`、`~/.aws`、`~/.azure`、`~/.config/gcloud`、`~/.gnupg`、`~/.kube`、
  钥匙串、Chrome / Firefox 的配置目录。
- `~/.netrc`、`~/.pypirc`、`~/.docker/config.json`、`~/.config/gh/hosts.yml`、
  `~/.git-credentials`、cargo 的 credentials。
- `$DOCK_HOME` 的 `secrets.json`、`mcp_credentials.json`、`credentials/`、
  `devices.json`、`browser/user-data`（内置浏览器的登录态）。
- `[sandbox] deny_read` 里加的路径。

没收 `~/.npmrc`：国内常在里面配镜像源，读不到 `npm install` 会直接报错。
里面存了 token 的，自己加进 `deny_read`。

**连不上的 socket**：Docker / containerd / podman 的控制 socket、
Linux 的会话总线（`$XDG_RUNTIME_DIR/bus`）。
连上它们就能让沙箱外的守护进程替你跑东西
（`docker run -v /:/host`、`systemd-run --user`）。

**环境变量**：名字带 `KEY` / `SECRET` / `TOKEN` / `PASSWORD` / `CREDENTIAL`、
`*_PAT`、`*ASKPASS*`、`DYLD_*` / `LD_PRELOAD` / `LD_LIBRARY_PATH`、
`SSH_AUTH_SOCK`、`DBUS_SESSION_BUS_ADDRESS` 不传给命令。
要留的写进 `keep_env`。
命令里能读到 `DOCK_SANDBOX=seatbelt|landlock`。

**网络**：默认放行（`network = false` 关掉）。
只读沙箱一律不联网。

## 后端

| 系统 | 机制 | 需要 |
|---|---|---|
| macOS | Seatbelt：`/usr/bin/sandbox-exec` + 生成的 SBPL | 系统自带 |
| Linux | Landlock（文件）+ seccomp（网络、ptrace、io_uring） | Linux 5.19+ 且启用了 Landlock；不需要装任何东西，不需要 user namespace |
| 其它 | — | 不可用 |

第一次用到时真起一个沙箱里的 `true` 探测一次，结果在进程内缓存。

**macOS**：底子是 Codex 的 `seatbelt_base_policy.sbpl`（默认全拒），
原样拷在 `dock-sandbox/src/seatbelt/`。
放开读、按可写根放开写、按需放开网络，
受保护 / 拒读 / socket 的拒绝规则排在**最后**——
Seatbelt 以最后一条匹配为准（本机实测过）。
路径一律经 `-DNAME=路径` 传进去，不拼进规则文本。
可写根本身和受保护路径的祖先目录不许删、不许改名
（防 `mv .git g && 改 g/hooks && mv g .git`）。

**Linux**：规则在 Dock 里建好，
子进程 fork 之后、exec 之前只做两次系统调用装上，不分配内存。
Landlock 只能「放行」，所以：

- 拒读靠展开：沿着每个拒读路径的祖先往下，把兄弟一个个放行
  （`dock-sandbox/src/tree.rs`）。处处能列目录名，读不到内容。
- `.git/hooks` 这类可写目录里的只读路径内核挡不住
  （放行工作区就放行了下面的一切，收回整个 `.git` 又会让 `git add` 写不了锁文件）。
  改为命令前拍快照、跑完（含后台任务跑完）比对，被改了就还原并在结果里报告
  （`dock-sandbox/src/guard.rs`）。

## 已知限制

`/sandbox status` 会列出这台机器上的限制。

- **Linux：受保护路径是事后还原**。
  命令还在跑的时候，沙箱外的进程（你自己终端里的 `git commit`）
  可能先一步用上被改过的 hook；命令留下的孙进程在还原之后再改也拦不住。
- **Linux：读得到同一用户其它进程的环境变量**（`/proc/<pid>/environ`），
  包括 Dock 自己的，里面可能有模型密钥。macOS 上读不到。
- **Linux 7.1 以下：挡不住连控制 socket**（Landlock ABI 9 才管 Unix socket）。
- **Linux 6.2 以下：挡不住截断沙箱外、你本来有权限写的文件**（Landlock ABI 3）。
- **Linux 5.19 以下：不可用**。ABI 1 下跨目录改名一律被拒，git 会坏，宁可退回权限门。
- **只看工作区顶层的 `.git`**：工作区里再 `git clone` 出来的嵌套仓库，
  它的 hooks / config 不受保护（Grok 也是这个取舍）。`core.hooksPath` 指向别处的不跟。
- **`~/.ssh` 读不到、`SSH_AUTH_SOCK` 不传**：
  沙箱里 `git push` / `git pull` 走 ssh 会失败，要出沙箱。
  `commit.gpgsign = true` 的仓库，沙箱里 `git commit` 签不了名。
- **进程内的读工具**只按目标路径判断：
  对着 `~` 跑 `grep` 会扫进 `~/.ssh`，不会弹框。
- **MCP 服务器、LSP、内置浏览器、cua-driver 不在沙箱里**：
  它们是你配置 / 启用的程序，不是模型临时写的命令。
- **每条命令一个沙箱**：后一条命令发不了信号给前一条起的进程
  （`kill` 前面 `is_background` 起的服务会被拒），用 `kill_task`。
  macOS 实测如此；Linux 要 6.12+（Landlock 的信号隔离），更早的内核不拦。

## 配置

只认 `$DOCK_HOME/config.toml`（项目 `.dock/config.toml` 里的 `[sandbox]` 不认：
那是仓库作者写的，不能替你关掉或放宽沙箱）。
每次跑命令现读，改了不用重启。

```toml
[sandbox]
enabled = false          # 新开的页默认开不开；/sandbox 按页切
network = true           # 沙箱里的命令能不能联网
build_caches = true      # 依赖缓存可不可写
writable_roots = []      # 额外可写：绝对路径或 ~/ 开头
deny_read = []           # 额外读不到：绝对路径或 ~/ 开头
keep_env = []            # 默认会去掉的环境变量里要留下的（精确的变量名）
```

## 手动试

```sh
cargo run -p dock-sandbox --example sandboxed -- . bash -lc 'git status && cargo build'
cargo run -p dock-sandbox --example sandboxed -- --read-only --offline . bash -lc 'ls'
```

## 参考

- Codex：`codex-rs/sandboxing/src/seatbelt.rs`（SBPL 结构、可写根里的只读子路径、
  拦截识别 `is_likely_sandbox_denied`），`codex-rs/linux-sandbox`（seccomp 网络规则）。
- Grok：`xai-grok-sandbox`（按命令包装、包不上就不跑、受保护路径表、
  密钥表、环境变量表、规则排在最后）。
