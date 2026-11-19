# dock-sandbox

按命令的系统沙箱：一份策略，两个后端。
不含插件，不认识 Dock 的配置和会话；
接进 Dock 的是 `cordis-spine` 的 `sandbox` 插件。
产品面的说明见 [docs/SANDBOX.md](../docs/SANDBOX.md)。

| 后端 | 系统 | 机制 |
|---|---|---|
| `Seatbelt` | macOS | `/usr/bin/sandbox-exec` + 生成的 SBPL（底子拷自 Codex） |
| `Landlock` | Linux 5.19+ | Landlock（文件）+ seccomp（网络、ptrace、io_uring），子进程 exec 前装上 |

## 用法

```rust
use dock_sandbox::{availability, prepare, Access, Host, Policy, Settings};

let host = Host::detect(dock_home);
let policy = Policy::build(Access::WorkspaceWrite, &workspace, &Settings::default(), &host);
if availability().is_ready() {
    let (mut cmd, guard) = prepare(&policy, &argv)?.into_command();
    let status = cmd.current_dir(&workspace).status()?;
    if let Some(guard) = guard {
        let report = guard.restore(); // Linux：受保护路径被改了就还原
    }
}
```

- `Policy::allows_write` / `Policy::denies_read`：进程内的写 / 读工具用同一份边界。
- `availability()`：第一次调用真起一个沙箱里的 `true` 探测，进程内缓存。
- `prepare` 报错、或子进程里装规则失败（`spawn` 失败），命令都不会跑。
- `looks_like_denial`：失败的输出像不像被沙箱拦了（给模型一句提示用）。

## 测试

```sh
cargo test -p dock-sandbox                       # 单测 + 真起命令的 e2e
cargo run -p dock-sandbox --example sandboxed -- . bash -lc 'git status'
```

`tests/e2e.rs` 在后端不可用的机器上打印原因后跳过
（例如 Docker Desktop 的内核没有 Landlock）。
Linux 的 seccomp 规则有不依赖 Landlock 的单测（`linux::tests`）。

## 来源

`src/seatbelt/*.sbpl` 原样拷自 OpenAI Codex（Apache-2.0），文件头注明了出处与 commit。
受保护路径表、密钥表、环境变量表参考 Grok 的 `xai-grok-sandbox`。
